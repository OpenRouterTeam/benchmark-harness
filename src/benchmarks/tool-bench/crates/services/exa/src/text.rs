//! URL normalisation, a small deterministic relevance scorer and markdown helpers.
//! (Private copy of the helpers in mock-firecrawl; services do not share code.)

use std::collections::BTreeSet;

use regex::Regex;

/// `host/path?query` with the scheme, `www.`, fragment and trailing slash removed, so
/// `https://www.example.com/pricing/` and `http://example.com/pricing` compare equal.
pub fn norm_url(url: &str) -> String {
    let url = url.trim();
    let url = url.split('#').next().unwrap_or("");
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let (authority, tail) = match rest.find(['/', '?']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let host = authority.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_owned();
    let (path, query) = match tail.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (tail, None),
    };
    let path = path.trim_end_matches('/');
    match query {
        Some(q) if !q.is_empty() => format!("{host}{path}?{q}"),
        _ => format!("{host}{path}"),
    }
}

/// Lowercased host without `www.` (port kept).
pub fn host_of(url: &str) -> String {
    let n = norm_url(url);
    n.split(['/', '?']).next().unwrap_or("").to_owned()
}

/// Path starting with `/` (empty path is `/`), no query.
pub fn path_of(url: &str) -> String {
    let n = norm_url(url);
    let after = n.find('/').map_or("", |i| &n[i..]);
    let path = after.split('?').next().unwrap_or("");
    if path.is_empty() {
        "/".to_owned()
    } else {
        path.to_owned()
    }
}

/// `true` when `url` looks like an absolute http(s) URL with a host.
pub fn is_http_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"));
    rest.is_some_and(|r| {
        let host = r.split(['/', '?', '#']).next().unwrap_or("");
        (!host.is_empty() && host.contains('.')) || host.starts_with("localhost")
    })
}

/// `host` is `domain` or one of its subdomains (both without `www.`).
pub fn host_matches(host: &str, domain: &str) -> bool {
    let domain = domain.trim().to_ascii_lowercase();
    let domain = domain.strip_prefix("www.").unwrap_or(&domain);
    host == domain || host.ends_with(&format!(".{domain}"))
}

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "can", "do", "does", "for", "from", "how",
    "i", "in", "is", "it", "its", "me", "my", "of", "on", "or", "our", "the", "their", "this",
    "to", "what", "when", "where", "which", "who", "with", "www", "http", "https", "com",
];

fn stem(token: &str) -> String {
    if token.len() > 4
        && let Some(base) = token.strip_suffix("ies")
    {
        return format!("{base}y");
    }
    if token.len() > 3 && token.ends_with('s') && !token.ends_with("ss") {
        return token[..token.len() - 1].to_owned();
    }
    token.to_owned()
}

/// Lowercased, stemmed alphanumeric tokens.
pub fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(stem)
        .collect()
}

/// A parsed search query: significant terms plus `site:`, `-term` and quoted phrases.
#[derive(Debug, Default, Clone)]
pub struct SearchQuery {
    pub terms: Vec<String>,
    pub sites: Vec<String>,
    pub excludes: Vec<String>,
    pub phrases: Vec<String>,
}

impl SearchQuery {
    pub fn parse(query: &str) -> Self {
        let mut q = Self::default();
        let mut rest = String::new();
        let mut in_quote = false;
        let mut phrase = String::new();
        for c in query.chars() {
            if c == '"' {
                if in_quote && !phrase.trim().is_empty() {
                    q.phrases.push(phrase.trim().to_lowercase());
                    rest.push(' ');
                    rest.push_str(&phrase);
                    rest.push(' ');
                }
                phrase.clear();
                in_quote = !in_quote;
            } else if in_quote {
                phrase.push(c);
            } else {
                rest.push(c);
            }
        }
        for word in rest.split_whitespace() {
            let lower = word.to_lowercase();
            if let Some(site) = lower.strip_prefix("site:") {
                q.sites.push(site.trim_end_matches('/').to_owned());
            } else if let Some(ex) = lower.strip_prefix('-') {
                q.excludes.extend(tokens(ex));
            } else if let Some(v) = ["inurl:", "intitle:", "related:"]
                .iter()
                .find_map(|p| lower.strip_prefix(p))
            {
                q.terms.extend(tokens(v));
            } else {
                q.terms.extend(
                    tokens(&lower)
                        .into_iter()
                        .filter(|t| !STOPWORDS.contains(&t.as_str())),
                );
            }
        }
        let mut seen = BTreeSet::new();
        q.terms.retain(|t| seen.insert(t.clone()));
        q
    }

    /// Relevance of `text` (already including title, description, url, keywords): the
    /// number of query terms it contains, or `None` when it fails a phrase/exclude filter
    /// or matches too few terms to be returned.
    pub fn score(&self, text: &str) -> Option<usize> {
        let lower = text.to_lowercase();
        if self.phrases.iter().any(|p| !lower.contains(p.as_str())) {
            return None;
        }
        let toks: BTreeSet<String> = tokens(&lower).into_iter().collect();
        if self.excludes.iter().any(|e| toks.contains(e)) {
            return None;
        }
        if self.terms.is_empty() {
            return (!self.phrases.is_empty() || !self.sites.is_empty()).then_some(0);
        }
        let matched = self.terms.iter().filter(|t| toks.contains(*t)).count();
        (matched > 0 && matched * 3 >= self.terms.len()).then_some(matched)
    }
}

/// Absolute link targets in markdown (`[text](url)`), in order, deduplicated.
pub fn markdown_links(md: &str) -> Vec<String> {
    let re = Regex::new(r"(!?)\[[^\]]*\]\((https?://[^)\s]+)").expect("valid regex");
    let mut seen = BTreeSet::new();
    re.captures_iter(md)
        .filter(|c| c[1].is_empty())
        .map(|c| c[2].to_owned())
        .filter(|u| seen.insert(u.clone()))
        .collect()
}

/// Image sources in markdown (`![alt](url)`).
pub fn markdown_images(md: &str) -> Vec<String> {
    let re = Regex::new(r"!\[[^\]]*\]\((https?://[^)\s]+)").expect("valid regex");
    re.captures_iter(md).map(|c| c[1].to_owned()).collect()
}
