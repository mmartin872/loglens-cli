//! Converts recognized timestamp shapes into a value that orders the same
//! way the timestamps do chronologically, so `--since`/`--until` can compare
//! by time instead of by text. Formats this doesn't recognize still work for
//! filtering, they just fall back to lexicographic comparison (see
//! `compare` below).

use std::cmp::Ordering;

/// Compare two raw timestamp tokens chronologically when both are in a
/// recognized format, falling back to a plain string comparison otherwise.
/// The fallback keeps the old behavior for formats like
/// `2026-08-21T10:15:03Z` that happen to sort correctly as text, and for any
/// format nobody's taught this parser yet.
pub fn compare(a: &str, b: &str) -> Ordering {
    match (to_epoch_nanos(a), to_epoch_nanos(b)) {
        (Some(na), Some(nb)) => na.cmp(&nb),
        _ => a.cmp(b),
    }
}

/// Parse a timestamp token into nanoseconds since the Unix epoch (UTC).
/// Recognizes RFC 3339 (`Z` or a numeric `+HH:MM`/`-HH:MM` offset, with or
/// without fractional seconds) and bare Unix epoch seconds.
fn to_epoch_nanos(input: &str) -> Option<i128> {
    parse_unix_epoch(input).or_else(|| parse_rfc3339(input))
}

/// A run of 9 or 10 digits, optionally with a fractional part, e.g.
/// `1755767703` or `1755767703.5`. The digit-count bound is a heuristic to
/// avoid mistaking something like an 8-digit `YYYYMMDD` date stamp for an
/// epoch time; it covers Unix time from 2001 through 2286.
fn parse_unix_epoch(input: &str) -> Option<i128> {
    let (int_part, frac_part) = match input.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (input, None),
    };
    if int_part.len() < 9 || int_part.len() > 10 || !int_part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let seconds: i128 = int_part.parse().ok()?;
    let nanos = match frac_part {
        Some(f) if !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()) => pad_nanos(f)?,
        Some(_) => return None,
        None => 0,
    };
    Some(seconds * 1_000_000_000 + nanos)
}

/// `2026-08-21T10:15:03Z`, `2026-08-21T10:15:03.125+02:00`, or the same with
/// a space instead of `T`.
fn parse_rfc3339(input: &str) -> Option<i128> {
    if input.len() < 20 {
        return None;
    }
    let year: i64 = input.get(0..4)?.parse().ok()?;
    if input.as_bytes().get(4) != Some(&b'-') {
        return None;
    }
    let month: u32 = input.get(5..7)?.parse().ok()?;
    if input.as_bytes().get(7) != Some(&b'-') {
        return None;
    }
    let day: u32 = input.get(8..10)?.parse().ok()?;
    match input.as_bytes().get(10) {
        Some(b'T') | Some(b't') | Some(b' ') => {}
        _ => return None,
    }
    let hour: u32 = input.get(11..13)?.parse().ok()?;
    if input.as_bytes().get(13) != Some(&b':') {
        return None;
    }
    let minute: u32 = input.get(14..16)?.parse().ok()?;
    if input.as_bytes().get(16) != Some(&b':') {
        return None;
    }
    let second: u32 = input.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let mut rest = &input[19..];
    let mut nanos: i128 = 0;
    if let Some(stripped) = rest.strip_prefix('.') {
        let digit_len = stripped.bytes().take_while(|b| b.is_ascii_digit()).count();
        if digit_len == 0 {
            return None;
        }
        nanos = pad_nanos(&stripped[..digit_len])?;
        rest = &stripped[digit_len..];
    }

    let offset_seconds: i64 = if rest == "Z" || rest == "z" {
        0
    } else if rest.len() == 6 && (rest.starts_with('+') || rest.starts_with('-')) {
        let sign: i64 = if rest.starts_with('-') { -1 } else { 1 };
        let offset_hour: i64 = rest.get(1..3)?.parse().ok()?;
        if rest.as_bytes().get(3) != Some(&b':') {
            return None;
        }
        let offset_minute: i64 = rest.get(4..6)?.parse().ok()?;
        if offset_hour > 23 || offset_minute > 59 {
            return None;
        }
        sign * (offset_hour * 3600 + offset_minute * 60)
    } else {
        return None;
    };

    let days = days_from_civil(year, month, day);
    let seconds =
        days * 86400 + hour as i64 * 3600 + minute as i64 * 60 + second as i64 - offset_seconds;
    Some(seconds as i128 * 1_000_000_000 + nanos)
}

/// Left-justify a run of fractional-second digits to nanosecond precision,
/// truncating anything finer.
fn pad_nanos(digits: &str) -> Option<i128> {
    let mut padded = digits.to_string();
    padded.truncate(9);
    while padded.len() < 9 {
        padded.push('0');
    }
    padded.parse().ok()
}

/// Days since 1970-01-01 for a proleptic Gregorian date. Howard Hinnant's
/// `days_from_civil` algorithm - handles negative years and leap years
/// without pulling in a date library for it.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let year_of_era = y - era * 400;
    let month_index = (m as i64 + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + d as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146097 + day_of_era - 719468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_z_suffixed_rfc3339() {
        assert_eq!(
            to_epoch_nanos("2026-08-21T10:15:03Z"),
            to_epoch_nanos("2026-08-21T10:15:03+00:00")
        );
    }

    #[test]
    fn parses_numeric_offsets_relative_to_utc() {
        // 10:15:03+02:00 is 08:15:03 UTC, one hour behind 09:15:03Z.
        let plus_two = to_epoch_nanos("2026-08-21T10:15:03+02:00").unwrap();
        let utc = to_epoch_nanos("2026-08-21T09:15:03Z").unwrap();
        assert_eq!(plus_two, utc);
    }

    #[test]
    fn parses_fractional_seconds() {
        let a = to_epoch_nanos("2026-08-21T10:15:03.5Z").unwrap();
        let b = to_epoch_nanos("2026-08-21T10:15:03Z").unwrap();
        assert_eq!(a - b, 500_000_000);
    }

    #[test]
    fn parses_unix_epoch_seconds() {
        // 2026-08-21T10:15:03Z: 20686 days since the epoch (verified against
        // 2024-01-01T00:00:00Z = 1704067200) times 86400, plus the
        // 10:15:03 time-of-day offset.
        assert_eq!(
            to_epoch_nanos("1787307303"),
            to_epoch_nanos("2026-08-21T10:15:03Z")
        );
    }

    #[test]
    fn rejects_a_bare_date_as_epoch_seconds() {
        assert_eq!(to_epoch_nanos("20260821"), None);
    }

    #[test]
    fn rejects_unrecognized_shapes() {
        assert_eq!(to_epoch_nanos("Aug 21 10:15:03"), None);
        assert_eq!(to_epoch_nanos("not a timestamp"), None);
    }

    #[test]
    fn compare_orders_recognized_formats_chronologically_across_shapes() {
        assert_eq!(
            compare("1787307303", "2026-08-21T10:15:04Z"),
            Ordering::Less
        );
    }

    #[test]
    fn compare_falls_back_to_text_for_unrecognized_formats() {
        assert_eq!(compare("banana", "apple"), Ordering::Greater);
    }
}
