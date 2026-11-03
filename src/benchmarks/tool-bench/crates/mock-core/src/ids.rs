use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Deterministic ID source for one service in one scenario.
#[derive(Debug, Clone)]
pub struct IdGen {
    seed: String,
    counters: BTreeMap<String, u64>,
}

impl IdGen {
    pub fn new(seed: impl Into<String>) -> Self {
        Self {
            seed: seed.into(),
            counters: BTreeMap::new(),
        }
    }

    fn bump(&mut self, kind: &str) -> u64 {
        let counter = self.counters.entry(kind.to_owned()).or_insert(0);
        let n = *counter;
        *counter += 1;
        n
    }

    fn digest(&self, kind: &str, n: u64, block: u64) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.seed.as_bytes());
        hasher.update([0]);
        hasher.update(kind.as_bytes());
        hasher.update([0]);
        hasher.update(n.to_be_bytes());
        hasher.update(block.to_be_bytes());
        let mut out = [0u8; 32];
        out.copy_from_slice(&hasher.finalize());
        out
    }

    /// `start`, `start + 1`, ... across calls with the same `kind`.
    pub fn next_int(&mut self, kind: &str, start: u64) -> u64 {
        start + self.bump(kind)
    }

    /// `prefix` followed by `len` base62 characters, e.g. `cus_` + 14 characters.
    pub fn next_prefixed(&mut self, kind: &str, prefix: &str, len: usize) -> String {
        let n = self.bump(kind);
        let mut out = String::with_capacity(prefix.len() + len);
        out.push_str(prefix);
        let mut block = 0;
        while out.len() < prefix.len() + len {
            for byte in self.digest(kind, n, block) {
                if out.len() == prefix.len() + len {
                    break;
                }
                out.push(BASE62[usize::from(byte) % BASE62.len()] as char);
            }
            block += 1;
        }
        out
    }

    /// RFC 9562 version 8 UUID derived from the seed, kind and counter.
    pub fn next_uuid(&mut self, kind: &str) -> uuid::Uuid {
        let n = self.bump(kind);
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&self.digest(kind, n, 0)[..16]);
        uuid::Uuid::new_v8(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let mut a = IdGen::new("seed-1");
        let mut b = IdGen::new("seed-1");
        for _ in 0..5 {
            assert_eq!(
                a.next_prefixed("customer", "cus_", 14),
                b.next_prefixed("customer", "cus_", 14)
            );
            assert_eq!(a.next_uuid("event"), b.next_uuid("event"));
        }
    }

    #[test]
    fn different_seeds_differ() {
        let mut a = IdGen::new("seed-1");
        let mut b = IdGen::new("seed-2");
        assert_ne!(
            a.next_prefixed("customer", "cus_", 14),
            b.next_prefixed("customer", "cus_", 14)
        );
    }

    #[test]
    fn kinds_count_independently() {
        let mut ids = IdGen::new("s");
        assert_eq!(ids.next_int("issue", 100), 100);
        assert_eq!(ids.next_int("issue", 100), 101);
        assert_eq!(ids.next_int("comment", 7), 7);
        assert_eq!(ids.next_int("issue", 100), 102);
    }

    #[test]
    fn prefixed_ids_have_requested_shape() {
        let mut ids = IdGen::new("s");
        let id = ids.next_prefixed("customer", "cus_", 40);
        assert_eq!(id.len(), 44);
        assert!(id.starts_with("cus_"));
        assert!(id[4..].chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(id, ids.next_prefixed("customer", "cus_", 40));
    }

    #[test]
    fn uuids_are_version_8() {
        let mut ids = IdGen::new("s");
        assert_eq!(ids.next_uuid("event").get_version_num(), 8);
    }
}
