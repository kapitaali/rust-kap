//! Kap timestamps (port of `dates/dates-platform.kt` `APLTimestamp` + the ISO-8601
//! surface used by `builtins/time-functions.kt`).
//!
//! A timestamp is epoch milliseconds (`i64`). Parsing/formatting is UTC ISO-8601
//! (`Instant.parse` / `Instant.toString`): `YYYY-MM-DDTHH:MM:SS[.fff]Z` with exactly
//! the precision Java emits (millis shown only when nonzero).

/// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Inverse: civil date from days since epoch.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

fn two(s: &str) -> Option<i64> {
    if s.len() == 2 && s.bytes().all(|b| b.is_ascii_digit()) {
        s.parse::<i64>().ok()
    } else {
        None
    }
}

/// Parse `YYYY-MM-DDTHH:MM:SS[.fff]Z` to epoch millis (Kotlin `Instant.parse`,
/// UTC only — the precision the conformance tests use).
pub fn parse_timestamp(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    // Exactly `YYYY-MM-DDTHH:MM:SSZ` (20) or with `.fff` millis (24).
    if b.len() != 20 && b.len() != 24 {
        return None;
    }
    if *b.last()? != b'Z' {
        return None;
    }
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let year: i64 = s[0..4].parse().ok()?;
    let month = two(&s[5..7])?;
    let day = two(&s[8..10])?;
    let hour = two(&s[11..13])?;
    let min = two(&s[14..16])?;
    let sec = two(&s[17..19])?;
    let ms: i64 = if b.len() == 24 {
        if b[19] != b'.' {
            return None;
        }
        let f = s[20..23].parse::<i64>().ok()?;
        if !(s[20..23].bytes().all(|x| x.is_ascii_digit())) {
            return None;
        }
        f
    } else {
        0
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || min > 59 || sec > 59 || !(0..1000).contains(&ms) {
        return None;
    }
    // Day-of-month upper bound (leap years): reject impossible dates like Feb 30.
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let max_day = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if day > max_day {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400_000 + hour * 3_600_000 + min * 60_000 + sec * 1000 + ms)
}

/// Format epoch millis as ISO-8601 UTC (Kotlin `Instant.toString`: millis shown
/// only when nonzero).
pub fn format_timestamp(millis: i64) -> String {
    let days = millis.div_euclid(86_400_000);
    let rem = millis.rem_euclid(86_400_000);
    let (y, m, d) = civil_from_days(days);
    let h = rem / 3_600_000;
    let mi = (rem % 3_600_000) / 60_000;
    let s = (rem % 60_000) / 1000;
    let ms = rem % 1000;
    let base = format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", y, m, d, h, mi, s);
    if ms == 0 {
        format!("{}Z", base)
    } else {
        format!("{}.{:03}Z", base, ms)
    }
}
