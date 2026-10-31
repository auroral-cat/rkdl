//! rkdl's own reasons a string isn't an ISO 8601 date, time or date-time. `fromisoformat` decides what reads, apart
//! from hour 24; these say why a string didn't, in words that don't change with the Python version, as Python's own do.

#[derive(Clone, Copy)]
pub(crate) enum Iso {
    Date,
    Time,
    DateTime,
}

/// Why `text`, which `fromisoformat` refused, doesn't fit: the first field out of range when the fields can be picked
/// out (`month must be in 1..12, not 13`), otherwise the form expected.
pub(crate) fn reason(kind: Iso, text: &str) -> String {
    let s = text.as_bytes();
    let field = match kind {
        Iso::Date => date(s).and_then(|(reason, _)| reason),
        Iso::Time | Iso::DateTime => split(kind, s).and_then(|(reason, rest)| reason.or_else(|| time(rest?))),
    };
    field.unwrap_or_else(|| {
        match kind {
            Iso::Date => "expected an ISO 8601 date, like 2026-01-31",
            Iso::Time => "expected an ISO 8601 time, like 09:30:00",
            Iso::DateTime => "expected an ISO 8601 date-time, like 2026-01-31T09:30:00",
        }
        .to_owned()
    })
}

/// Whether a time or date-time gives hour 24. Python 3.14's `fromisoformat` reads it as the end of the day (the next
/// day's midnight), earlier Pythons refuse it; rkdl refuses it on every Python, as RFC 3339 does, so a file means the
/// same everywhere.
pub(crate) fn hour_24(kind: Iso, text: &str) -> bool {
    split(kind, text.as_bytes())
        .and_then(|(_, rest)| number(rest?, 2))
        .is_some_and(|(hour, _)| hour == 24)
}

/// A time or date-time's date part's reason, if it has one, and the time part, if there is one to pick out.
fn split(kind: Iso, s: &[u8]) -> Option<(Option<String>, Option<&[u8]>)> {
    match kind {
        Iso::Date => None,
        Iso::Time => Some((None, Some(s.strip_prefix(b"T").unwrap_or(s)))),
        // Python takes any one character between the date and the time.
        Iso::DateTime => date(s).map(|(reason, rest)| (reason, rest.get(1..))),
    }
}

/// `YYYY-MM-DD` or `YYYYMMDD` at the start of `s`: the first field out of range, if one is, and the rest of `s`. A
/// week date (`YYYY-Www[-D]`, `YYYYWww[D]`) gives no reason, only the rest. None if `s` doesn't start with a date.
fn date(s: &[u8]) -> Option<(Option<String>, &[u8])> {
    let (year, s) = number(s, 4)?;
    let dashed = s.first() == Some(&b'-');
    let s = if dashed { s.get(1..)? } else { s };
    if let Some(week) = s.strip_prefix(b"W") {
        let (_, rest) = number(week, 2)?;
        let day = if dashed { rest.strip_prefix(b"-") } else { Some(rest) };
        let rest = day.and_then(|d| number(d, 1)).map_or(rest, |(_, after)| after);
        return Some((None, rest));
    }
    let (month, s) = number(s, 2)?;
    let s = if dashed { s.strip_prefix(b"-")? } else { s };
    let (day, rest) = number(s, 2)?;
    let reason = if year == 0 {
        Some("year must be in 1..9999, not 0".to_owned())
    } else if !(1..=12).contains(&month) {
        Some(format!("month must be in 1..12, not {month}"))
    } else {
        let last = days_in_month(year, month);
        (!(1..=last).contains(&day)).then(|| format!("day must be in 1..{last}, not {day}"))
    };
    Some((reason, rest))
}

/// The first field out of range in a time at the start of `s`, `HH[:MM[:SS]]` or `HH[MM[SS]]` with a fraction and a
/// UTC offset after it. None if none is out of range, or the fields can't be picked out.
fn time(s: &[u8]) -> Option<String> {
    let (hour, mut s) = number(s, 2)?;
    let mut fields = vec![("hour", hour, 23)];
    let colons = s.first() == Some(&b':');
    for name in ["minute", "second"] {
        let after = if colons { s.strip_prefix(b":") } else { Some(s) };
        let Some((value, rest)) = after.and_then(|t| number(t, 2)) else {
            break;
        };
        fields.push((name, value, 59));
        s = rest;
    }
    if let Some((name, value, max)) = fields.into_iter().find(|&(_, value, max)| value > max) {
        return Some(format!("{name} must be in 0..{max}, not {value}"));
    }
    if let Some(fraction) = s.strip_prefix(b".").or_else(|| s.strip_prefix(b",")) {
        s = skip_digits(fraction);
    }
    offset(s)
}

/// Why a UTC offset at the start of `s` is out of range, if it is: Python takes up to 24 hours, not including 24.
fn offset(s: &[u8]) -> Option<String> {
    let rest = s.strip_prefix(b"+").or_else(|| s.strip_prefix(b"-"))?;
    let (hours, mut rest) = number(rest, 2)?;
    let mut seconds = hours * 3600;
    let colons = rest.first() == Some(&b':');
    for unit in [60, 1] {
        let after = if colons { rest.strip_prefix(b":") } else { Some(rest) };
        let Some((value, next)) = after.and_then(|t| number(t, 2)) else {
            break;
        };
        seconds += value * unit;
        rest = next;
    }
    let written = std::str::from_utf8(s).ok()?;
    (seconds >= 24 * 3600).then(|| format!("offset must be under 24 hours, not {written}"))
}

/// The number made of exactly `digits` ASCII digits at the start of `s`, and the rest of `s`.
fn number(s: &[u8], digits: usize) -> Option<(u32, &[u8])> {
    let (head, rest) = s.split_at_checked(digits)?;
    head.iter().all(u8::is_ascii_digit).then(|| {
        let value = head.iter().fold(0, |n, d| n * 10 + u32::from(d - b'0'));
        (value, rest)
    })
}

const fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn skip_digits(s: &[u8]) -> &[u8] {
    let digits = s.iter().take_while(|b| b.is_ascii_digit()).count();
    s.get(digits..).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{Iso, hour_24, reason};

    #[test]
    fn names_the_field_out_of_range() {
        let cases = [
            (Iso::Date, "2026-13-01", "month must be in 1..12, not 13"),
            (Iso::Date, "20260001", "month must be in 1..12, not 0"),
            (Iso::Date, "2026-02-30", "day must be in 1..28, not 30"),
            (Iso::Date, "2024-02-30", "day must be in 1..29, not 30"),
            (Iso::Date, "2026-04-31", "day must be in 1..30, not 31"),
            (Iso::Date, "0000-01-01", "year must be in 1..9999, not 0"),
            (Iso::Time, "25:00", "hour must be in 0..23, not 25"),
            (Iso::Time, "T12:60", "minute must be in 0..59, not 60"),
            (Iso::Time, "120061", "second must be in 0..59, not 61"),
            (
                Iso::Time,
                "12:00:00.5+25:00",
                "offset must be under 24 hours, not +25:00",
            ),
            (Iso::Time, "12:00-2400", "offset must be under 24 hours, not -2400"),
            (Iso::DateTime, "2026-13-01T25:00", "month must be in 1..12, not 13"),
            (Iso::DateTime, "2026-01-01 24:30", "hour must be in 0..23, not 24"),
            (Iso::DateTime, "2026-W05-6T24:00", "hour must be in 0..23, not 24"),
            (
                Iso::DateTime,
                "20260101T12:00:00+24:00",
                "offset must be under 24 hours, not +24:00",
            ),
        ];
        for (kind, text, why) in cases {
            assert_eq!(reason(kind, text), why, "{text}");
        }
    }

    #[test]
    fn gives_the_form_when_no_field_is_out_of_range() {
        assert_eq!(
            reason(Iso::Date, "May 6th"),
            "expected an ISO 8601 date, like 2026-01-31"
        );
        assert_eq!(
            reason(Iso::Date, "2026-1-1"),
            "expected an ISO 8601 date, like 2026-01-31"
        );
        assert_eq!(
            reason(Iso::Time, "half past"),
            "expected an ISO 8601 time, like 09:30:00"
        );
        assert_eq!(
            reason(Iso::DateTime, "2026-01-01T"),
            "expected an ISO 8601 date-time, like 2026-01-31T09:30:00"
        );
    }

    #[test]
    fn finds_hour_24_after_any_date_form() {
        let cases = [
            (Iso::Time, "24:00", true),
            (Iso::Time, "T2400", true),
            (Iso::Time, "24:00:00+01:00", true),
            (Iso::Time, "23:59:59", false),
            (Iso::Time, "12:24", false),
            (Iso::DateTime, "2026-01-31T24:00:00", true),
            (Iso::DateTime, "20260131 24", true),
            (Iso::DateTime, "2026-W05-6T24:00", true),
            (Iso::DateTime, "2026-W05T24:00", true),
            (Iso::DateTime, "2026W056T24:00", true),
            (Iso::DateTime, "2026W05T24:00", true),
            (Iso::DateTime, "2026-01-24T12:00", false),
            (Iso::Date, "2026-01-24", false),
        ];
        for (kind, text, expected) in cases {
            assert_eq!(hour_24(kind, text), expected, "{text}");
        }
    }
}
