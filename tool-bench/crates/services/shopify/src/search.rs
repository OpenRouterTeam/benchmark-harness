//! The subset of Shopify's search syntax (`query:` argument) used by the Admin clients:
//! `field:value`, `field:*wild*card*`, `field:>=10`, quoted values, `-field:value` / `NOT`,
//! implicit `AND`, `OR`, and free-text terms.

/// How a field's values are compared with a term.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Whole-value, case-insensitive equality (ids, emails, statuses, tags).
    Exact,
    /// Equality with the whole value or with any word of it (titles, names).
    Words,
}

/// The values a document exposes for one search field. `None` from the lookup means the
/// field is unknown to this resource; Shopify ignores such terms.
pub struct FieldValues {
    pub values: Vec<String>,
    pub mode: Mode,
}

impl FieldValues {
    pub fn exact(values: Vec<String>) -> Option<Self> {
        Some(Self {
            values,
            mode: Mode::Exact,
        })
    }

    pub fn words(values: Vec<String>) -> Option<Self> {
        Some(Self {
            values,
            mode: Mode::Words,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Term {
    field: Option<String>,
    value: String,
    negate: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Query {
    /// OR of AND-groups.
    groups: Vec<Vec<Term>>,
}

pub fn parse(query: &str) -> Query {
    let tokens = tokenize(query);
    let mut groups = vec![Vec::new()];
    let mut negate_next = false;
    let mut i = 0;
    while i < tokens.len() {
        let (raw, quoted) = &tokens[i];
        i += 1;
        if !quoted {
            match raw.as_str() {
                "OR" => {
                    groups.push(Vec::new());
                    continue;
                }
                "AND" => continue,
                "NOT" => {
                    negate_next = true;
                    continue;
                }
                _ => {}
            }
        }
        let mut text = raw.trim_matches(|c| c == '(' || c == ')').to_owned();
        if text.is_empty() {
            continue;
        }
        let mut negate = std::mem::take(&mut negate_next);
        if !quoted && text.len() > 1 && text.starts_with('-') {
            negate = !negate;
            text.remove(0);
        }
        let term = match (!quoted).then(|| text.split_once(':')).flatten() {
            Some((field, value)) if !field.is_empty() => {
                let mut value = value.trim_matches(['"', '\'']).to_owned();
                // `title:*Linen Shirt*` arrives as two whitespace-separated tokens.
                if value.starts_with('*') && !value.ends_with('*') && value.len() > 1 {
                    let mut j = i;
                    while j < tokens.len() {
                        let next = tokens[j].0.trim_end_matches(')');
                        value.push(' ');
                        value.push_str(next);
                        j += 1;
                        if next.ends_with('*') {
                            i = j;
                            break;
                        }
                    }
                    if !value.ends_with('*') {
                        // No closing wildcard: keep only the first token.
                        value = value.split(' ').next().unwrap_or_default().to_owned();
                    }
                }
                let field = field.to_ascii_lowercase();
                // `tag_not:x` is `-tag:x`.
                let (field, negate) = match field.strip_suffix("_not") {
                    Some(base) if base == "tag" => (base.to_owned(), !negate),
                    _ => (field, negate),
                };
                Term {
                    field: Some(field),
                    value,
                    negate,
                }
            }
            _ => Term {
                field: None,
                value: text,
                negate,
            },
        };
        groups.last_mut().expect("at least one group").push(term);
    }
    groups.retain(|g| !g.is_empty());
    Query { groups }
}

/// Splits on whitespace, keeping quoted runs (`"Linen Shirt"`, `title:"a b"`) together.
fn tokenize(query: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut quoted = false;
    for c in query.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => current.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                quoted = quoted || current.is_empty();
            }
            None if c.is_whitespace() => {
                if !current.is_empty() {
                    out.push((std::mem::take(&mut current), quoted));
                }
                quoted = false;
            }
            None => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push((current, quoted));
    }
    out
}

impl Query {
    /// `field` returns the document's values for a named field; `text` are the values that
    /// free-text terms are matched against (substring, case-insensitive).
    pub fn matches(&self, field: &dyn Fn(&str) -> Option<FieldValues>, text: &[String]) -> bool {
        if self.groups.is_empty() {
            return true;
        }
        self.groups.iter().any(|group| {
            group.iter().all(|term| {
                let hit = match &term.field {
                    None => {
                        let needle = term.value.to_lowercase();
                        if needle.contains('*') {
                            text.iter().any(|t| glob(&needle, &t.to_lowercase()))
                        } else {
                            text.iter().any(|t| t.to_lowercase().contains(&needle))
                        }
                    }
                    Some(name) => match field(name) {
                        None => return true,
                        Some(values) => values
                            .values
                            .iter()
                            .any(|v| value_matches(&term.value, v, values.mode)),
                    },
                };
                hit != term.negate
            })
        })
    }
}

fn value_matches(pattern: &str, candidate: &str, mode: Mode) -> bool {
    for op in [">=", "<=", ">", "<"] {
        if let Some(rest) = pattern.strip_prefix(op) {
            return compare(op, rest, candidate);
        }
    }
    let p = pattern.to_lowercase();
    let c = candidate.to_lowercase();
    if p.contains('*') {
        return glob(&p, &c);
    }
    if p == c {
        return true;
    }
    mode == Mode::Words
        && c.split(|ch: char| !ch.is_alphanumeric())
            .any(|word| !word.is_empty() && word == p)
}

fn compare(op: &str, bound: &str, candidate: &str) -> bool {
    let ord = match (bound.parse::<f64>(), candidate.parse::<f64>()) {
        (Ok(b), Ok(c)) => c.partial_cmp(&b),
        _ => Some(candidate.cmp(bound)),
    };
    let Some(ord) = ord else {
        return false;
    };
    match op {
        ">=" => ord.is_ge(),
        "<=" => ord.is_le(),
        ">" => ord.is_gt(),
        _ => ord.is_lt(),
    }
}

/// `*` matches any run of characters; everything else literally.
fn glob(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    let first = parts[0];
    if !rest.starts_with(first) {
        return false;
    }
    rest = &rest[first.len()..];
    let last = parts[parts.len() - 1];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(i) => rest = &rest[i + part.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(name: &str) -> Option<FieldValues> {
        match name {
            "title" => FieldValues::words(vec!["Harbor Canvas Tote".into()]),
            "status" => FieldValues::exact(vec!["active".into()]),
            "tag" => FieldValues::exact(vec!["summer".into(), "bags".into()]),
            "inventory_total" => FieldValues::exact(vec!["42".into()]),
            _ => None,
        }
    }

    fn hit(q: &str) -> bool {
        parse(q).matches(&doc, &["Harbor Canvas Tote".into(), "Octo Goods".into()])
    }

    #[test]
    fn field_terms_wildcards_and_words() {
        assert!(hit("title:*canvas*"));
        assert!(hit("title:*Canvas Tote*"));
        assert!(!hit("title:*Canvas Mini*"));
        assert!(hit("title:tote"));
        assert!(!hit("title:tot"));
        assert!(hit("title:'Harbor Canvas Tote'"));
        assert!(hit("status:ACTIVE tag:bags"));
        assert!(!hit("status:draft"));
    }

    #[test]
    fn negation_or_comparisons_free_text_and_unknown_fields() {
        assert!(!hit("-tag:summer"));
        assert!(!hit("tag_not:summer"));
        assert!(hit("tag_not:winter"));
        assert!(hit("NOT tag:winter"));
        assert!(hit("tag_x:whatever status:active"));
        assert!(hit("status:draft OR tag:summer"));
        assert!(hit("inventory_total:>=40 inventory_total:<50"));
        assert!(!hit("inventory_total:>42"));
        assert!(hit("octo"));
        assert!(hit("\"canvas tote\""));
        assert!(!hit("backpack"));
        assert!(parse("").groups.is_empty());
    }
}
