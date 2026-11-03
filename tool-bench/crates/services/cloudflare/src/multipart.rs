//! Minimal `multipart/form-data` parsing for the HTTP listener.

use crate::api::Part;

pub fn boundary(content_type: &str) -> Option<String> {
    content_type.split(';').find_map(|p| {
        let p = p.trim();
        p.strip_prefix("boundary=")
            .map(|b| b.trim_matches('"').to_owned())
    })
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (from..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

fn param(header: &str, name: &str) -> Option<String> {
    header.split(';').find_map(|p| {
        let p = p.trim();
        let (k, v) = p.split_once('=')?;
        (k.trim().eq_ignore_ascii_case(name)).then(|| v.trim().trim_matches('"').to_owned())
    })
}

pub fn parse(body: &[u8], boundary: &str) -> Vec<Part> {
    let delimiter = format!("--{boundary}");
    let delimiter = delimiter.as_bytes();
    let mut parts = Vec::new();
    let Some(mut at) = find(body, delimiter, 0) else {
        return parts;
    };
    loop {
        let start = at + delimiter.len();
        if body[start..].starts_with(b"--") {
            break;
        }
        let Some(next) = find(body, delimiter, start) else {
            break;
        };
        let chunk = &body[start..next];
        let chunk = chunk.strip_prefix(b"\r\n").unwrap_or(chunk);
        let chunk = chunk.strip_suffix(b"\r\n").unwrap_or(chunk);
        if let Some(split) = find(chunk, b"\r\n\r\n", 0) {
            let headers = String::from_utf8_lossy(&chunk[..split]).into_owned();
            let content = chunk[split + 4..].to_vec();
            let mut name = String::new();
            let mut filename = None;
            let mut content_type = None;
            for line in headers.split("\r\n") {
                let Some((key, value)) = line.split_once(':') else {
                    continue;
                };
                match key.trim().to_ascii_lowercase().as_str() {
                    "content-disposition" => {
                        name = param(value, "name").unwrap_or_default();
                        filename = param(value, "filename");
                    }
                    "content-type" => content_type = Some(value.trim().to_owned()),
                    _ => {}
                }
            }
            parts.push(Part {
                name,
                filename,
                content_type,
                bytes: content,
            });
        }
        at = next;
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fields_and_files() {
        let body = b"--XX\r\nContent-Disposition: form-data; name=\"metadata\"\r\nContent-Type: application/json\r\n\r\n{\"main_module\":\"worker.js\"}\r\n--XX\r\nContent-Disposition: form-data; name=\"worker.js\"; filename=\"worker.js\"\r\nContent-Type: application/javascript+module\r\n\r\nexport default {}\r\n--XX--\r\n";
        let parts = parse(body, "XX");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].name, "metadata");
        assert_eq!(parts[0].text(), "{\"main_module\":\"worker.js\"}");
        assert_eq!(parts[1].filename.as_deref(), Some("worker.js"));
        assert_eq!(parts[1].text(), "export default {}");
        assert_eq!(
            boundary("multipart/form-data; boundary=XX").as_deref(),
            Some("XX")
        );
    }
}
