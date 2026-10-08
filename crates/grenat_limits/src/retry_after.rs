//! `Retry-After` (RFC 9110, §10.2.3): how long a server asks to be left
//! alone, as seconds (`120`) or as an HTTP date. A date is read in each of
//! the three formats §5.6.7 says a recipient must accept: the IMF-fixdate
//! (`Sun, 06 Nov 1994 08:49:37 GMT`), RFC 850's (`Sunday, 06-Nov-94
//! 08:49:37 GMT`, a two-digit year more than 50 years ahead taken as the
//! latest past year ending with those digits) and asctime's (`Sun Nov  6
//! 08:49:37 1994`). Anything else is no answer.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// How long to wait from `now`, as `value` says: a date already past is
/// no wait at all.
pub fn parse(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        // more digits than a u64 holds: as long as can be
        return Some(Duration::from_secs(value.parse().unwrap_or(u64::MAX)));
    }
    let now_secs = now.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    let at = http_date(value, now_secs)?;
    Some(Duration::from_secs(at.saturating_sub(now_secs).max(0) as u64))
}

/// Seconds since the epoch of an HTTP date; `now` (seconds too) settles a
/// two-digit year.
pub fn http_date(value: &str, now: i64) -> Option<i64> {
    let words: Vec<&str> = value.split_ascii_whitespace().collect();
    let (day, month, year, time) = match words.as_slice() {
        // IMF-fixdate
        [name, day, month, year, time, "GMT"] if name.ends_with(',') && day.len() == 2 && year.len() == 4 => {
            (day.parse().ok()?, month_number(month)?, year.parse().ok()?, *time)
        }
        // RFC 850
        [name, date, time, "GMT"] if name.ends_with(',') => {
            let mut parts = date.split('-');
            let (day, month, year) = (parts.next()?, parts.next()?, parts.next()?);
            if parts.next().is_some() || day.len() != 2 || year.len() != 2 {
                return None;
            }
            (day.parse().ok()?, month_number(month)?, two_digit_year(year.parse().ok()?, now), *time)
        }
        // asctime
        [_, month, day, time, year] if (1..=2).contains(&day.len()) && year.len() == 4 => {
            (day.parse().ok()?, month_number(month)?, year.parse().ok()?, *time)
        }
        _ => return None,
    };
    let mut clock = time.split(':');
    let (h, m, s) = (clock.next()?, clock.next()?, clock.next()?);
    if clock.next().is_some() || [h, m, s].iter().any(|part| part.len() != 2) {
        return None;
    }
    let (h, m, s): (i64, i64, i64) = (h.parse().ok()?, m.parse().ok()?, s.parse().ok()?);
    // 60: a leap second
    if day < 1 || day > days_in_month(year, month) || h > 23 || m > 59 || s > 60 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + h * 3600 + m * 60 + s)
}

fn month_number(name: &str) -> Option<i64> {
    MONTHS.iter().position(|m| *m == name).map(|i| i as i64 + 1)
}

/// RFC 9110: a two-digit year that would be more than 50 years ahead is
/// the latest past year with those last digits.
fn two_digit_year(yy: i64, now: i64) -> i64 {
    let this_year = 1970 + now.div_euclid(86_400 * 36_525 / 100);
    let century = this_year - this_year.rem_euclid(100);
    let year = century + yy;
    if year > this_year + 50 { year - 100 } else { year }
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1994-11-06T08:49:37Z, RFC 9110's example.
    const EXAMPLE: i64 = 784_111_777;

    fn at(secs: i64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs as u64)
    }

    #[test]
    fn seconds() {
        assert_eq!(parse("120", at(0)), Some(Duration::from_secs(120)));
        assert_eq!(parse(" 0 ", at(0)), Some(Duration::ZERO));
        assert_eq!(parse("99999999999999999999999", at(0)), Some(Duration::from_secs(u64::MAX)));
        assert_eq!(parse("-1", at(0)), None);
        assert_eq!(parse("1.5", at(0)), None);
        assert_eq!(parse("", at(0)), None);
    }

    #[test]
    fn the_three_date_formats() {
        for date in ["Sun, 06 Nov 1994 08:49:37 GMT", "Sunday, 06-Nov-94 08:49:37 GMT", "Sun Nov  6 08:49:37 1994"] {
            assert_eq!(http_date(date, EXAMPLE), Some(EXAMPLE), "{date}");
        }
        assert_eq!(parse("Sun, 06 Nov 1994 08:51:37 GMT", at(EXAMPLE)), Some(Duration::from_secs(120)));
        // already past
        assert_eq!(parse("Sun, 06 Nov 1994 08:49:37 GMT", at(EXAMPLE + 60)), Some(Duration::ZERO));
        assert_eq!(http_date("Fri, 31 Dec 1999 23:59:59 GMT", 0), Some(946_684_799));
        assert_eq!(http_date("Thu, 29 Feb 2024 00:00:00 GMT", 0), Some(1_709_164_800));
    }

    #[test]
    fn two_digit_years_are_never_more_than_fifty_years_ahead() {
        // in 2026, `76` is 2076 and `77` is 1977
        let now = http_date("Thu, 08 Oct 2026 00:00:00 GMT", 0).unwrap();
        let year = |yy: &str| http_date(&format!("Monday, 01-Jan-{yy} 00:00:00 GMT"), now).unwrap();
        assert_eq!(year("76"), http_date("Fri, 01 Jan 2076 00:00:00 GMT", 0).unwrap());
        assert_eq!(year("77"), http_date("Sat, 01 Jan 1977 00:00:00 GMT", 0).unwrap());
        assert_eq!(year("26"), http_date("Thu, 01 Jan 2026 00:00:00 GMT", 0).unwrap());
    }

    #[test]
    fn what_is_not_a_date() {
        for bad in [
            "Sun, 06 Nov 1994 08:49:37 UTC",
            "Sun, 31 Feb 1994 08:49:37 GMT",
            "Sun, 06 Nov 1994 24:00:00 GMT",
            "Sun, 06 Foo 1994 08:49:37 GMT",
            "Sun, 6 Nov 1994 08:49:37 GMT",
            "Sun, 06 Nov 1994 8:49:37 GMT",
            "Sunday, 06-Nov-1994 08:49:37 GMT",
            "2026-10-08T10:00:00Z",
            "soon",
        ] {
            assert_eq!(http_date(bad, EXAMPLE), None, "{bad}");
        }
    }
}
