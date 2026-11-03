//! Time values as Datadog accepts them: epoch milliseconds, ISO 8601, or `now-15m` math.

use time::{OffsetDateTime, format_description::well_known::Rfc3339};

/// Epoch milliseconds from an RFC 3339 / ISO 8601 timestamp.
pub fn rfc3339_ms(text: &str) -> Option<i64> {
    let parsed = OffsetDateTime::parse(text.trim(), &Rfc3339).ok()?;
    Some((parsed.unix_timestamp_nanos() / 1_000_000) as i64)
}

/// A search-API time bound in epoch milliseconds, relative to `now_ms` for `now` math.
/// Accepts `1761750000000` (ms), `1761750000` (s, if under 1e11), ISO 8601, `now`,
/// `now-15m`, `now-1h`, `now-2d`, `now-30s`.
pub fn bound_ms(text: &str, now_ms: i64) -> Option<i64> {
    let text = text.trim();
    if let Ok(n) = text.parse::<f64>() {
        let n = n as i64;
        return Some(if n.abs() < 100_000_000_000 {
            n * 1000
        } else {
            n
        });
    }
    if let Some(rest) = text.strip_prefix("now") {
        if rest.is_empty() {
            return Some(now_ms);
        }
        let (sign, amount) = match rest.as_bytes()[0] {
            b'-' => (-1, &rest[1..]),
            b'+' => (1, &rest[1..]),
            _ => return None,
        };
        let unit_at = amount.find(|c: char| !c.is_ascii_digit())?;
        let value: i64 = amount[..unit_at].parse().ok()?;
        let unit = match &amount[unit_at..] {
            "s" => 1_000,
            "m" => 60_000,
            "h" => 3_600_000,
            "d" => 86_400_000,
            "w" => 604_800_000,
            "mo" => 2_592_000_000,
            _ => return None,
        };
        return Some(now_ms + sign * value * unit);
    }
    rfc3339_ms(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_bound_forms() {
        let now = 1_761_750_000_000;
        assert_eq!(bound_ms("1761740000000", now), Some(1_761_740_000_000));
        assert_eq!(bound_ms("1761740000", now), Some(1_761_740_000_000));
        assert_eq!(bound_ms("now", now), Some(now));
        assert_eq!(bound_ms("now-15m", now), Some(now - 900_000));
        assert_eq!(bound_ms("now-1h", now), Some(now - 3_600_000));
        assert_eq!(
            bound_ms("2025-10-29T15:00:00.000Z", now),
            Some(1_761_750_000_000)
        );
        assert_eq!(bound_ms("yesterday", now), None);
    }
}
