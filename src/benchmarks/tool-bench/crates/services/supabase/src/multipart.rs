//! Minimal `multipart/form-data` reading and writing (Edge Function deploy and body).

#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub name: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub data: Vec<u8>,
}

pub fn boundary(content_type: &str) -> Option<String> {
    content_type.split(';').find_map(|p| {
        let p = p.trim();
        p.strip_prefix("boundary=")
            .map(|b| b.trim_matches('"').to_owned())
    })
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

fn header_param(header: &str, key: &str) -> Option<String> {
    header.split(';').find_map(|p| {
        let p = p.trim();
        let (k, v) = p.split_once('=')?;
        (k.trim().eq_ignore_ascii_case(key)).then(|| v.trim().trim_matches('"').to_owned())
    })
}

pub fn parse(body: &[u8], boundary: &str) -> Vec<Part> {
    let delim = format!("--{boundary}").into_bytes();
    let mut parts = Vec::new();
    let Some(mut pos) = find(body, &delim, 0) else {
        return parts;
    };
    loop {
        pos += delim.len();
        if body.get(pos..pos + 2) == Some(b"--") {
            break;
        }
        if body.get(pos..pos + 2) == Some(b"\r\n") {
            pos += 2;
        } else if body.get(pos) == Some(&b'\n') {
            pos += 1;
        }
        let Some(next) = find(body, &delim, pos) else {
            break;
        };
        let chunk = &body[pos..next];
        let (head_end, sep) = match find(chunk, b"\r\n\r\n", 0) {
            Some(i) => (i, 4),
            None => match find(chunk, b"\n\n", 0) {
                Some(i) => (i, 2),
                None => (chunk.len(), 0),
            },
        };
        let head = String::from_utf8_lossy(&chunk[..head_end]);
        let mut data = chunk[(head_end + sep).min(chunk.len())..].to_vec();
        if data.ends_with(b"\r\n") {
            data.truncate(data.len() - 2);
        } else if data.ends_with(b"\n") {
            data.truncate(data.len() - 1);
        }
        let mut name = String::new();
        let mut filename = None;
        let mut content_type = None;
        for line in head.lines() {
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            if k.trim().eq_ignore_ascii_case("content-disposition") {
                name = header_param(v, "name").unwrap_or_default();
                filename = header_param(v, "filename");
            } else if k.trim().eq_ignore_ascii_case("content-type") {
                content_type = Some(v.trim().to_owned());
            }
        }
        parts.push(Part {
            name,
            filename,
            content_type,
            data,
        });
        pos = next;
    }
    parts
}

/// Encodes `parts` with `boundary`.
pub fn encode(parts: &[Part], boundary: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for p in parts {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        let mut disp = format!("Content-Disposition: form-data; name=\"{}\"", p.name);
        if let Some(f) = &p.filename {
            disp.push_str(&format!("; filename=\"{f}\""));
        }
        out.extend_from_slice(disp.as_bytes());
        out.extend_from_slice(b"\r\n");
        if let Some(ct) = &p.content_type {
            out.extend_from_slice(format!("Content-Type: {ct}\r\n").as_bytes());
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&p.data);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let parts = vec![
            Part {
                name: "metadata".into(),
                filename: None,
                content_type: Some("application/json".into()),
                data: br#"{"name":"hello"}"#.to_vec(),
            },
            Part {
                name: "file".into(),
                filename: Some("index.ts".into()),
                content_type: Some("application/typescript".into()),
                data: b"Deno.serve(() => new Response('hi'))\n".to_vec(),
            },
        ];
        let body = encode(&parts, "XyZ");
        assert_eq!(
            boundary("multipart/form-data; boundary=XyZ").as_deref(),
            Some("XyZ")
        );
        assert_eq!(parse(&body, "XyZ"), parts);
    }
}
