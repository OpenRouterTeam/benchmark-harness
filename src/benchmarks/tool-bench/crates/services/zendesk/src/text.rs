//! Small text helpers: standard base64 and the Markdown-to-HTML step the server applies to
//! comments before posting them (markdown-it, CommonMark, `breaks=True`).

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decodes standard base64 (padding optional). Invalid characters are skipped.
pub fn b64_decode(text: &str) -> Vec<u8> {
    let values: Vec<u32> = text
        .bytes()
        .filter_map(|c| B64.iter().position(|b| *b == c).map(|p| p as u32))
        .collect();
    let mut out = Vec::with_capacity(values.len() * 3 / 4);
    for chunk in values.chunks(4) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, v)| acc | (v << (18 - 6 * i)));
        for i in 0..chunk.len().saturating_sub(1) {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    out
}

pub fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Paragraphs, hard line breaks, `-`/`*` bullet lists and `**bold**`, which covers what
/// support replies use. Output follows markdown-it (`<p>..</p>\n`, `<br>\n`).
pub fn markdown_to_html(text: &str) -> String {
    let mut out = String::new();
    let text = text.replace("\r\n", "\n");
    for block in text.split("\n\n") {
        let block = block.trim_matches('\n');
        if block.trim().is_empty() {
            continue;
        }
        let lines: Vec<&str> = block.lines().collect();
        if lines
            .iter()
            .all(|l| l.starts_with("- ") || l.starts_with("* "))
        {
            out.push_str("<ul>\n");
            for line in lines {
                out.push_str(&format!("<li>{}</li>\n", inline(&line[2..])));
            }
            out.push_str("</ul>\n");
        } else {
            let body: Vec<String> = lines.iter().map(|l| inline(l.trim())).collect();
            out.push_str(&format!("<p>{}</p>\n", body.join("<br>\n")));
        }
    }
    out
}

fn inline(text: &str) -> String {
    let escaped = escape_html(text);
    let mut out = String::new();
    let mut rest = escaped.as_str();
    while let Some(start) = rest.find("**") {
        let Some(len) = rest[start + 2..].find("**") else {
            break;
        };
        out.push_str(&rest[..start]);
        out.push_str("<strong>");
        out.push_str(&rest[start + 2..start + 2 + len]);
        out.push_str("</strong>");
        rest = &rest[start + 4 + len..];
    }
    out.push_str(rest);
    out
}

/// The `html_body` Zendesk shows for a plain-text comment from the dataset.
pub fn plain_to_comment_html(body: &str) -> String {
    let paragraphs: Vec<String> = body
        .split("\n\n")
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            format!(
                "<p>{}</p>",
                p.trim_matches('\n')
                    .lines()
                    .map(escape_html)
                    .collect::<Vec<_>>()
                    .join("<br>")
            )
        })
        .collect();
    format!(
        "<div class=\"zd-comment\" dir=\"auto\">{}</div>",
        paragraphs.join("")
    )
}

/// Plain text Zendesk derives from an `html_body`: tags stripped, breaks kept.
pub fn html_to_text(html: &str) -> String {
    let mut s = html.to_owned();
    for (from, to) in [
        ("<br>\n", "\n"),
        ("<br/>\n", "\n"),
        ("<br />\n", "\n"),
        ("<br>", "\n"),
        ("<br/>", "\n"),
        ("<br />", "\n"),
        ("</p>\n", "\n\n"),
        ("</p>", "\n\n"),
        ("</li>\n", "\n"),
        ("</li>", "\n"),
    ] {
        s = s.replace(from, to);
    }
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_to_text_reverses_markdown_rendering() {
        let text = "Hi Dana,\nthanks & regards.\n\nSecond paragraph.";
        assert_eq!(html_to_text(&markdown_to_html(text)), text);
        assert_eq!(html_to_text(&plain_to_comment_html(text)), text);
    }

    #[test]
    fn base64_round_trip() {
        for input in [&b""[..], b"f", b"fo", b"foo", b"foob", b"\x89PNG\r\n\x1a\n"] {
            assert_eq!(b64_decode(&b64_encode(input)), input);
        }
        assert_eq!(b64_encode(b"foob"), "Zm9vYg==");
    }

    #[test]
    fn markdown_matches_markdown_it_for_common_input() {
        assert_eq!(
            markdown_to_html("Hi Dana,\nthanks for the report.\n\n- one\n- **two**"),
            "<p>Hi Dana,<br>\nthanks for the report.</p>\n<ul>\n<li>one</li>\n<li><strong>two</strong></li>\n</ul>\n"
        );
    }
}
