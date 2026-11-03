//! URL normalisation, a small deterministic relevance scorer, markdown helpers, and
//! monitor schedule parsing.

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

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A plain HTML rendering of simple markdown (headings, lists, tables as text, paragraphs).
pub fn markdown_to_html(md: &str) -> String {
    let link = Regex::new(r"\[([^\]]*)\]\(([^)\s]+)\)").expect("valid regex");
    let inline = |s: &str| {
        link.replace_all(&escape_html(s), "<a href=\"$2\">$1</a>")
            .into_owned()
    };
    let mut out = String::new();
    let mut in_list = false;
    for line in md.lines() {
        let t = line.trim();
        let is_item = t.starts_with("- ") || t.starts_with("* ");
        if in_list && !is_item {
            out.push_str("</ul>");
            in_list = false;
        }
        if t.is_empty() {
            continue;
        }
        let hashes = t.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && t[hashes..].starts_with(' ') {
            out.push_str(&format!(
                "<h{hashes}>{}</h{hashes}>",
                inline(t[hashes..].trim())
            ));
        } else if is_item {
            if !in_list {
                out.push_str("<ul>");
                in_list = true;
            }
            out.push_str(&format!("<li>{}</li>", inline(&t[2..])));
        } else {
            out.push_str(&format!("<p>{}</p>", inline(t)));
        }
    }
    if in_list {
        out.push_str("</ul>");
    }
    out
}

/// The first paragraph-ish text of a page, for summaries.
pub fn summarize(md: &str) -> String {
    let text: Vec<&str> = md
        .lines()
        .map(str::trim)
        .filter(|l| {
            !l.is_empty() && !l.starts_with('#') && !l.starts_with('|') && !l.starts_with("![")
        })
        .take(3)
        .collect();
    let joined = text.join(" ");
    if joined.chars().count() > 400 {
        let cut: String = joined.chars().take(400).collect();
        format!("{cut}...")
    } else {
        joined
    }
}

fn number_word(word: &str) -> Option<u64> {
    let n = match word {
        "one" | "a" | "an" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "twelve" => 12,
        "fifteen" => 15,
        "thirty" => 30,
        _ => return word.parse().ok(),
    };
    Some(n)
}

/// Minutes between checks for a schedule phrase such as "every 30 minutes", "daily at
/// 9am", "hourly", or "every monday".
pub fn schedule_interval_minutes(text: &str) -> Option<u64> {
    let t = text.to_lowercase();
    let every =
        Regex::new(r"every\s+([a-z0-9]+?)\s*(minute|min|month|m|hour|hr|h|day|d|week|wk|w)s?\b")
            .expect("valid regex");
    if let Some(c) = every.captures(&t)
        && let Some(n) = number_word(&c[1])
    {
        let unit = match &c[2] {
            "minute" | "min" | "m" => 1,
            "hour" | "hr" | "h" => 60,
            "day" | "d" => 1440,
            "week" | "wk" | "w" => 10080,
            _ => 43200,
        };
        return Some(n * unit);
    }
    let has = |words: &[&str]| words.iter().any(|w| t.contains(w));
    if has(&["hourly", "every hour", "each hour", "once an hour"]) {
        Some(60)
    } else if has(&[
        "daily",
        "every day",
        "each day",
        "once a day",
        "per day",
        "every morning",
        "every evening",
        "every night",
        "nightly",
        "every weekday",
    ]) {
        Some(1440)
    } else if has(&[
        "weekly",
        "every week",
        "once a week",
        "each week",
        "every monday",
        "every tuesday",
        "every wednesday",
        "every thursday",
        "every friday",
        "every saturday",
        "every sunday",
    ]) {
        Some(10080)
    } else if has(&["monthly", "every month", "once a month"]) {
        Some(43200)
    } else {
        cron_interval_minutes(&t)
    }
}

/// Minutes between runs for common 5-field cron shapes.
pub fn cron_interval_minutes(cron: &str) -> Option<u64> {
    let f: Vec<&str> = cron.split_whitespace().collect();
    if f.len() != 5 {
        return None;
    }
    let num = |s: &str| s.parse::<u64>().is_ok();
    let step = |s: &str| s.strip_prefix("*/").and_then(|n| n.parse::<u64>().ok());
    match (f[0], f[1], f[2], f[3], f[4]) {
        (m, "*", "*", "*", "*") if step(m).is_some() => step(m),
        ("*", "*", "*", "*", "*") => Some(1),
        (m, "*", "*", "*", "*") if num(m) => Some(60),
        (m, h, "*", "*", "*") if num(m) && step(h).is_some() => step(h).map(|n| n * 60),
        (m, h, "*", "*", "*") if num(m) && (num(h) || h.contains(',')) => Some(1440),
        (m, h, "*", "*", d) if num(m) && num(h) && d.contains('-') => Some(1440),
        (m, h, "*", "*", d) if num(m) && num(h) && d != "*" => Some(10080),
        (m, h, dom, "*", "*") if num(m) && num(h) && num(dom) => Some(43200),
        _ => None,
    }
}

/// Decodes `application/x-www-form-urlencoded` query text into ordered pairs.
pub fn parse_query(raw: Option<&str>) -> Vec<(String, String)> {
    raw.unwrap_or("")
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect()
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len()
                && bytes[i + 1].is_ascii_hexdigit()
                && bytes[i + 2].is_ascii_hexdigit() =>
            {
                let hex = |b: u8| (b as char).to_digit(16).unwrap_or(0) as u8;
                out.push(hex(bytes[i + 1]) * 16 + hex(bytes[i + 2]));
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_urls() {
        assert_eq!(
            norm_url("https://www.Example.com/pricing/#plans"),
            "example.com/pricing"
        );
        assert_eq!(norm_url("http://example.com"), "example.com");
        assert_eq!(norm_url("https://example.com/?a=1"), "example.com?a=1");
        assert_eq!(path_of("https://docs.x.dev/api/refunds"), "/api/refunds");
        assert_eq!(path_of("https://docs.x.dev"), "/");
        assert_eq!(host_of("https://www.x.dev:8080/a"), "x.dev:8080");
        assert!(is_http_url("https://x.dev/a"));
        assert!(!is_http_url("ftp://x.dev"));
    }

    #[test]
    fn scores_queries() {
        let q = SearchQuery::parse("Northwind Analytics pricing plans site:northwind.com -jobs");
        assert_eq!(q.terms, ["northwind", "analytic", "pricing", "plan"]);
        assert_eq!(q.sites, ["northwind.com"]);
        assert_eq!(q.score("Northwind Analytics | Pricing"), Some(3));
        assert_eq!(q.score("Northwind jobs pricing"), None);
        assert_eq!(q.score("unrelated"), None);
        let p = SearchQuery::parse("\"team plan\" cost");
        assert_eq!(p.score("The Team plan costs"), Some(3));
        assert_eq!(p.score("plan for the team"), None);
    }

    #[test]
    fn parses_schedules() {
        assert_eq!(schedule_interval_minutes("every 30 minutes"), Some(30));
        assert_eq!(schedule_interval_minutes("Daily at 9am"), Some(1440));
        assert_eq!(schedule_interval_minutes("every day at 08:00"), Some(1440));
        assert_eq!(schedule_interval_minutes("every 24 hours"), Some(1440));
        assert_eq!(schedule_interval_minutes("every two hours"), Some(120));
        assert_eq!(schedule_interval_minutes("every 24h"), Some(1440));
        assert_eq!(schedule_interval_minutes("every 15m"), Some(15));
        assert_eq!(schedule_interval_minutes("every 2 months"), Some(86400));
        assert_eq!(schedule_interval_minutes("every Monday"), Some(10080));
        assert_eq!(schedule_interval_minutes("0 9 * * *"), Some(1440));
        assert_eq!(schedule_interval_minutes("*/15 * * * *"), Some(15));
        assert_eq!(schedule_interval_minutes("whenever"), None);
    }

    #[test]
    fn markdown_helpers() {
        let md = "# Title\n\nSee [docs](https://x.dev/docs) and ![logo](https://x.dev/l.png).\n\n- [a](https://x.dev/a)";
        assert_eq!(
            markdown_links(md),
            ["https://x.dev/docs", "https://x.dev/a"]
        );
        assert_eq!(markdown_images(md), ["https://x.dev/l.png"]);
        assert!(
            markdown_to_html(md)
                .starts_with("<h1>Title</h1><p>See <a href=\"https://x.dev/docs\">docs</a>")
        );
        assert_eq!(
            parse_query(Some("query=a+b%20c&k=3&authors=x&authors=y")),
            [
                ("query".to_owned(), "a b c".to_owned()),
                ("k".to_owned(), "3".to_owned()),
                ("authors".to_owned(), "x".to_owned()),
                ("authors".to_owned(), "y".to_owned())
            ]
        );
    }
}
