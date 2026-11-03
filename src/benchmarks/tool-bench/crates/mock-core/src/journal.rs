use serde::Serialize;
use serde_json::Value;

/// One request/response pair as seen by the world.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct JournalEntry {
    pub seq: u64,
    /// Logical time after any tick caused by this request.
    pub time: String,
    pub service: String,
    /// Peer IP of the caller; used to detect requests that bypassed the MCP servers.
    pub source: String,
    pub method: String,
    /// `Host` header and scheme of the request, so a replay rebuilds identical URLs.
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default = "default_scheme")]
    pub scheme: String,
    #[serde(default)]
    pub content_type: Option<String>,
    pub path: String,
    pub query: Option<String>,
    pub request_body: Value,
    pub status: u16,
    pub response_body: Value,
}

fn default_scheme() -> String {
    "http".to_owned()
}

/// Append-only, ordered record of every request served.
#[derive(Debug, Clone, Default)]
pub struct Journal {
    entries: Vec<JournalEntry>,
}

impl Journal {
    pub fn next_seq(&self) -> u64 {
        self.entries.len() as u64 + 1
    }

    pub fn push(&mut self, entry: JournalEntry) {
        assert_eq!(
            entry.seq,
            self.next_seq(),
            "journal sequence must be contiguous"
        );
        self.entries.push(entry);
    }

    pub fn entries(&self) -> &[JournalEntry] {
        &self.entries
    }
}

/// `null` for an empty body, parsed JSON when possible, otherwise the text.
pub fn body_value(bytes: &[u8]) -> Value {
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(bytes).into_owned()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn body_value_classifies_bytes() {
        assert_eq!(body_value(b""), Value::Null);
        assert_eq!(body_value(br#"{"a":1}"#), json!({"a": 1}));
        assert_eq!(body_value(b"plain text"), json!("plain text"));
    }

    #[test]
    fn sequence_numbers_start_at_one() {
        let mut journal = Journal::default();
        assert_eq!(journal.next_seq(), 1);
        journal.push(entry(1));
        assert_eq!(journal.next_seq(), 2);
        assert_eq!(journal.entries().len(), 1);
    }

    #[test]
    #[should_panic(expected = "journal sequence")]
    fn rejects_out_of_order_entries() {
        let mut journal = Journal::default();
        journal.push(entry(2));
    }

    fn entry(seq: u64) -> JournalEntry {
        JournalEntry {
            seq,
            time: "2026-03-14T09:00:00Z".into(),
            service: "github".into(),
            source: "127.0.0.1".into(),
            method: "GET".into(),
            host: Some("world:8101".into()),
            scheme: "http".into(),
            content_type: None,
            path: "/api/v3/user".into(),
            query: None,
            request_body: Value::Null,
            status: 200,
            response_body: json!({"login": "mona"}),
        }
    }
}
