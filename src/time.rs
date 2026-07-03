//! Time: unix seconds internally, ISO dates at the edges.
//!
//! Erosion physics work in days; timestamps are unix seconds (i64). Query
//! time (`as_of`) is a parameter everywhere — past and future are the same
//! operation. Date arithmetic uses Howard Hinnant's civil-days algorithm,
//! so there is no calendar dependency.

use crate::error::{Error, Result};

pub const SECONDS_PER_DAY: f64 = 86_400.0;

/// Days since 1970-01-01 for a civil date (proleptic Gregorian).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Civil date (y, m, d) from days since 1970-01-01.
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

/// Parse a point in time: raw unix seconds ("1751500800"), a date
/// ("2026-07-03"), or a date-time ("2026-07-03T14:30:00" / "... 14:30").
pub fn parse_when(s: &str) -> Result<i64> {
    let s = s.trim();
    if let Ok(n) = s.parse::<i64>() {
        return Ok(n);
    }
    let (date, time) = match s.split_once(['T', ' ']) {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };
    let parts: Vec<&str> = date.split('-').collect();
    let bad = || {
        Error::Invalid(format!(
            "cannot parse time {s:?}; use YYYY-MM-DD, YYYY-MM-DDTHH:MM[:SS], or unix seconds"
        ))
    };
    if parts.len() != 3 {
        return Err(bad());
    }
    let y: i64 = parts[0].parse().map_err(|_| bad())?;
    let m: i64 = parts[1].parse().map_err(|_| bad())?;
    let d: i64 = parts[2].parse().map_err(|_| bad())?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return Err(bad());
    }
    let mut secs = days_from_civil(y, m, d) * 86_400;
    if let Some(t) = time {
        let t = t.trim_end_matches('Z');
        let tp: Vec<&str> = t.split(':').collect();
        if tp.len() < 2 || tp.len() > 3 {
            return Err(bad());
        }
        let hh: i64 = tp[0].parse().map_err(|_| bad())?;
        let mm: i64 = tp[1].parse().map_err(|_| bad())?;
        let ss: i64 = if tp.len() == 3 {
            tp[2].parse().map_err(|_| bad())?
        } else {
            0
        };
        if !(0..24).contains(&hh) || !(0..60).contains(&mm) || !(0..60).contains(&ss) {
            return Err(bad());
        }
        secs += hh * 3600 + mm * 60 + ss;
    }
    Ok(secs)
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn format_unix(t: i64) -> String {
    let days = t.div_euclid(86_400);
    let rem = t.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    if rem == 0 {
        format!("{y:04}-{m:02}-{d:02}")
    } else {
        format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
            rem / 3600,
            (rem % 3600) / 60,
            rem % 60
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_roundtrip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 7, 3), 20_637);
        for z in [-1000, 0, 20_637, 99_999] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
    }

    #[test]
    fn parse_variants() {
        assert_eq!(parse_when("0").unwrap(), 0);
        assert_eq!(parse_when("2026-07-03").unwrap(), 20_637 * 86_400);
        assert_eq!(
            parse_when("2026-07-03T01:02:03").unwrap(),
            20_637 * 86_400 + 3723
        );
        assert!(parse_when("gestern").is_err());
        assert_eq!(format_unix(20_637 * 86_400), "2026-07-03");
    }
}
