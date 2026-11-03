//! Standard base64 with padding, for opaque pagination cursors (private copy; mock-core
//! has no base64 helper).

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn decode(text: &str) -> Option<Vec<u8>> {
    let values: Vec<u32> = text
        .trim_end_matches('=')
        .bytes()
        .map(|c| ALPHABET.iter().position(|b| *b == c).map(|p| p as u32))
        .collect::<Option<_>>()?;
    let mut out = Vec::new();
    for chunk in values.chunks(4) {
        if chunk.len() == 1 {
            return None;
        }
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, v)| acc | (v << (18 - 6 * i)));
        for i in 0..chunk.len() - 1 {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        assert_eq!(encode(b"{\"last_id\":1}"), "eyJsYXN0X2lkIjoxfQ==");
        for s in ["", "a", "ab", "abc", "abcd"] {
            assert_eq!(decode(&encode(s.as_bytes())).unwrap(), s.as_bytes());
        }
        assert_eq!(decode("!!"), None);
    }
}
