//! Civil time without a timezone database: the handful of zones the datasets use, with
//! their real DST rules (US and EU). Deterministic and wall-clock free.

/// Days since 1970-01-01 for a proleptic Gregorian date.
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 0 = Sunday.
fn weekday(days: i64) -> i64 {
    (days + 4).rem_euclid(7)
}

/// Day number of the `n`th (1-based) Sunday of a month, or the last one when `n == 0`.
fn nth_sunday(y: i64, m: i64, n: i64) -> i64 {
    if n == 0 {
        let next = if m == 12 {
            days_from_civil(y + 1, 1, 1)
        } else {
            days_from_civil(y, m + 1, 1)
        };
        let last = next - 1;
        return last - weekday(last);
    }
    let first = days_from_civil(y, m, 1);
    let first_sunday = first + (7 - weekday(first)) % 7;
    first_sunday + 7 * (n - 1)
}

struct Zone {
    std_offset: i64,
    std_abbr: &'static str,
    dst_abbr: Option<&'static str>,
    eu: bool,
}

fn zone(name: &str) -> Zone {
    let us = |h: i64, s: &'static str, d: &'static str| Zone {
        std_offset: h * 3600,
        std_abbr: s,
        dst_abbr: Some(d),
        eu: false,
    };
    match name {
        "America/New_York" => us(-5, "EST", "EDT"),
        "America/Chicago" => us(-6, "CST", "CDT"),
        "America/Denver" => us(-7, "MST", "MDT"),
        "America/Los_Angeles" => us(-8, "PST", "PDT"),
        "Europe/London" => Zone {
            std_offset: 0,
            std_abbr: "GMT",
            dst_abbr: Some("BST"),
            eu: true,
        },
        "Europe/Berlin" | "Europe/Paris" => Zone {
            std_offset: 3600,
            std_abbr: "CET",
            dst_abbr: Some("CEST"),
            eu: true,
        },
        "Asia/Kolkata" => Zone {
            std_offset: 19_800,
            std_abbr: "IST",
            dst_abbr: None,
            eu: false,
        },
        _ => Zone {
            std_offset: 0,
            std_abbr: "UTC",
            dst_abbr: None,
            eu: false,
        },
    }
}

/// UTC offset in seconds and abbreviation of `zone_name` at unix time `t`.
pub fn offset_at(zone_name: &str, t: i64) -> (i64, &'static str) {
    let z = zone(zone_name);
    let Some(dst) = z.dst_abbr else {
        return (z.std_offset, z.std_abbr);
    };
    let (y, _, _) = civil_from_days((t + z.std_offset).div_euclid(86_400));
    let (start, end) = if z.eu {
        (
            nth_sunday(y, 3, 0) * 86_400 + 3600,
            nth_sunday(y, 10, 0) * 86_400 + 3600,
        )
    } else {
        (
            nth_sunday(y, 3, 2) * 86_400 + 2 * 3600 - z.std_offset,
            nth_sunday(y, 11, 1) * 86_400 + 2 * 3600 - (z.std_offset + 3600),
        )
    };
    if t >= start && t < end {
        (z.std_offset + 3600, dst)
    } else {
        (z.std_offset, z.std_abbr)
    }
}

/// Parses `YYYY-MM-DD` (or `YYYY/MM/DD`) to (y, m, d).
pub fn parse_date(s: &str) -> Option<(i64, i64, i64)> {
    let parts: Vec<&str> = s.trim().split(['-', '/']).collect();
    if parts.len() != 3 {
        return None;
    }
    let y = parts[0].parse().ok()?;
    let m = parts[1].parse().ok()?;
    let d = parts[2].parse().ok()?;
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then_some((y, m, d))
}

/// RFC 3339 UTC `2026-10-26T17:10:43Z`.
pub fn rfc3339(t: i64) -> String {
    let (y, m, d) = civil_from_days(t.div_euclid(86_400));
    let s = t.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        s / 3600,
        s % 3600 / 60,
        s % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chicago_dst_boundaries() {
        // 2026-10-26 17:10:43Z is CDT.
        let t = days_from_civil(2026, 10, 26) * 86_400 + 17 * 3600 + 10 * 60 + 43;
        assert_eq!(offset_at("America/Chicago", t), (-5 * 3600, "CDT"));
        // DST ends 2026-11-01 07:00Z.
        let end = days_from_civil(2026, 11, 1) * 86_400 + 7 * 3600;
        assert_eq!(offset_at("America/Chicago", end - 1).1, "CDT");
        assert_eq!(offset_at("America/Chicago", end).1, "CST");
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn civil_round_trip() {
        for d in [-1000, 0, 19_000, 20_757] {
            let (y, m, dd) = civil_from_days(d);
            assert_eq!(days_from_civil(y, m, dd), d);
        }
    }
}
