use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

use crate::error::{Error, Result};

/// Scenario time. Starts at the scenario's `now` and moves only when [`LogicalClock::tick`]
/// is called (once per mutating request), so timestamps never depend on the wall clock.
#[derive(Debug, Clone)]
pub struct LogicalClock {
    start: i64,
    step_secs: i64,
    ticks: i64,
}

impl LogicalClock {
    pub const DEFAULT_STEP_SECS: i64 = 7;

    pub fn new(start_rfc3339: &str, step_secs: i64) -> Result<Self> {
        let start = OffsetDateTime::parse(start_rfc3339, &Rfc3339)
            .map_err(|source| Error::Timestamp {
                value: start_rfc3339.to_owned(),
                source,
            })?
            .unix_timestamp();
        Ok(Self {
            start,
            step_secs,
            ticks: 0,
        })
    }

    pub fn now_unix(&self) -> i64 {
        self.start + self.ticks * self.step_secs
    }

    pub fn now_rfc3339(&self) -> String {
        format_unix(self.now_unix())
    }

    /// Advances one step and returns the new time.
    pub fn tick(&mut self) -> i64 {
        self.ticks += 1;
        self.now_unix()
    }
}

/// Formats a unix timestamp as second-precision RFC 3339 UTC, e.g. `2026-03-14T09:00:00Z`.
pub fn format_unix(secs: i64) -> String {
    OffsetDateTime::from_unix_timestamp(secs)
        .expect("timestamp within supported range")
        .to_offset(UtcOffset::UTC)
        .format(&Rfc3339)
        .expect("RFC 3339 formatting of a UTC timestamp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_scenario_now() {
        let clock = LogicalClock::new("2026-03-14T09:00:00Z", 7).unwrap();
        assert_eq!(clock.now_rfc3339(), "2026-03-14T09:00:00Z");
    }

    #[test]
    fn tick_advances_by_step() {
        let mut clock = LogicalClock::new("2026-03-14T09:00:00Z", 7).unwrap();
        let after = clock.tick();
        assert_eq!(after, clock.now_unix());
        assert_eq!(clock.now_rfc3339(), "2026-03-14T09:00:07Z");
        clock.tick();
        assert_eq!(clock.now_rfc3339(), "2026-03-14T09:00:14Z");
    }

    #[test]
    fn normalizes_offsets_to_utc() {
        let clock = LogicalClock::new("2026-03-14T10:00:00+01:00", 7).unwrap();
        assert_eq!(clock.now_rfc3339(), "2026-03-14T09:00:00Z");
    }

    #[test]
    fn rejects_bad_timestamp() {
        assert!(LogicalClock::new("yesterday", 7).is_err());
    }
}
