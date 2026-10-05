//! Instants as text: RFC 3339 / ISO 8601 read into epoch seconds, and
//! epoch seconds written back in UTC, without a date library (the
//! calendar is `grenat_serve::calendar`'s).
//!
//! Read: `2026-09-28T08:00:00Z`, an offset (`+02:00`, `-0500`, `+02`), a
//! fraction of a second (`.250`), `t` or a space for `T`, no seconds
//! (`08:00Z`), no offset (UTC), or a date alone (midnight UTC). Years
//! 0000 to 9999, as RFC 3339 has them.

use grenat_serve::calendar::{civil, days_from_civil, days_in_month};

/// The first instant written (0000-01-01T00:00:00Z) and the first past the
/// last (10000-01-01T00:00:00Z), in seconds since the epoch.
const FIRST: i64 = -62_167_219_200;
const END: i64 = 253_402_300_800;

/// What every reading error says next to its reason.
const EXPECTED: &str = "expected ISO 8601 such as \"2026-09-28T08:00:00Z\" or \"2026-09-28\"";

/// A civil instant, UTC.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Civil {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// 1 (Monday) to 7 (Sunday), as ISO 8601 numbers them.
    pub weekday: u32,
    pub millis: u32,
}

/// The civil instant of `t` seconds since the epoch (to the millisecond),
/// or why it has none.
pub(crate) fn civil_of(t: f64) -> Result<Civil, String> {
    if !t.is_finite() {
        return Err(format!("{t} is not an instant"));
    }
    let millis = (t * 1000.0).round();
    let seconds = (millis / 1000.0).floor();
    if seconds < FIRST as f64 || seconds >= END as f64 {
        return Err(format!("{t} is out of range: years 0000 to 9999"));
    }
    let (seconds, millis) = (seconds as i64, (millis as i64).rem_euclid(1000) as u32);
    let (year, month, day, hour, minute, weekday) = civil(seconds);
    let weekday = if weekday == 0 { 7 } else { weekday };
    Ok(Civil { year, month, day, hour, minute, second: seconds.rem_euclid(60) as u32, weekday, millis })
}

/// `2026-09-28T08:00:00Z`, or `2026-09-28T08:00:00.250Z` with milliseconds.
pub(crate) fn format(t: f64) -> Result<String, String> {
    let c = civil_of(t)?;
    let fraction = if c.millis == 0 { String::new() } else { format!(".{:03}", c.millis) };
    Ok(format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{fraction}Z", c.year, c.month, c.day, c.hour, c.minute, c.second))
}

/// `2026-09-28`.
pub(crate) fn format_date(t: f64) -> Result<String, String> {
    let c = civil_of(t)?;
    Ok(format!("{:04}-{:02}-{:02}", c.year, c.month, c.day))
}

/// Seconds since the epoch of a civil date and time, UTC, or why it is none.
pub(crate) fn seconds_of(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: f64) -> Result<f64, String> {
    if !(0..=9999).contains(&year) {
        return Err(format!("year {year} is out of range (0 to 9999)"));
    }
    if !(1..=12).contains(&month) {
        return Err(format!("month {month} is out of range (1 to 12)"));
    }
    if day < 1 || day > i64::from(days_in_month(year, month as u32)) {
        return Err(format!("day {day} is not in {year:04}-{month:02}"));
    }
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
        return Err(format!("{hour:02}:{minute:02} is not a time of day"));
    }
    // 60: a leap second, as RFC 3339 allows
    if !(0.0..61.0).contains(&second) {
        return Err(format!("second {second} is out of range (0 to 60)"));
    }
    let days = days_from_civil(year, month as u32, day as u32);
    Ok((days * 86_400 + hour * 3600 + minute * 60) as f64 + second)
}

/// Seconds since the epoch of an RFC 3339 / ISO 8601 text.
pub(crate) fn parse(text: &str) -> Result<f64, String> {
    let fail = |reason: &str| format!("invalid time `{text}` ({reason}): {EXPECTED}");
    let mut r = Reader { bytes: text.trim().as_bytes(), at: 0 };
    let date = (|| {
        let year = r.digits(4)?;
        r.expect(b"-")?;
        let month = r.digits(2)?;
        r.expect(b"-")?;
        Some((year, month, r.digits(2)?))
    })()
    .ok_or_else(|| fail("no date YYYY-MM-DD"))?;
    let (year, month, day) = date;
    if r.done() {
        return seconds_of(year, month, day, 0, 0, 0.0).map_err(|e| fail(&e));
    }
    r.expect(b"Tt ").ok_or_else(|| fail("no `T` between the date and the time"))?;
    let (hour, minute) = (|| {
        let hour = r.digits(2)?;
        r.expect(b":")?;
        Some((hour, r.digits(2)?))
    })()
    .ok_or_else(|| fail("no time HH:MM"))?;
    let mut second = 0.0;
    if r.expect(b":").is_some() {
        second = r.digits(2).ok_or_else(|| fail("no seconds after `:`"))? as f64;
        if r.expect(b".,").is_some() {
            second += r.fraction().ok_or_else(|| fail("no digits after the decimal point"))?;
        }
    }
    let offset = r.offset().ok_or_else(|| fail("an offset is `Z`, `+HH:MM` or `-HH:MM`"))?;
    if !r.done() {
        return Err(fail("unexpected text after the time"));
    }
    seconds_of(year, month, day, hour, minute, second).map(|t| t - offset as f64).map_err(|e| fail(&e))
}

/// A cursor over the bytes of a text.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn done(&self) -> bool {
        self.at == self.bytes.len()
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    /// One of `any`, consumed.
    fn expect(&mut self, any: &[u8]) -> Option<u8> {
        let b = self.peek().filter(|b| any.contains(b))?;
        self.at += 1;
        Some(b)
    }

    /// Exactly `n` decimal digits.
    fn digits(&mut self, n: usize) -> Option<i64> {
        let slice = self.bytes.get(self.at..self.at + n)?;
        if !slice.iter().all(u8::is_ascii_digit) {
            return None;
        }
        self.at += n;
        Some(slice.iter().fold(0, |acc, b| acc * 10 + i64::from(b - b'0')))
    }

    /// The digits after a decimal point, as a fraction (`25` → 0.25).
    fn fraction(&mut self) -> Option<f64> {
        let start = self.at;
        while self.peek().is_some_and(|b| b.is_ascii_digit()) {
            self.at += 1;
        }
        let digits = std::str::from_utf8(&self.bytes[start..self.at]).ok()?;
        (!digits.is_empty()).then(|| format!("0.{digits}").parse().ok())?
    }

    /// The offset from UTC in seconds: nothing or `Z` (0), `±HH:MM`, `±HHMM`, `±HH`.
    fn offset(&mut self) -> Option<i64> {
        if self.done() || self.expect(b"Zz").is_some() {
            return Some(0);
        }
        let sign = if self.expect(b"+-")? == b'-' { -1 } else { 1 };
        let hours = self.digits(2)?;
        let minutes = match self.expect(b":") {
            Some(_) => self.digits(2)?,
            None if self.done() => 0,
            None => self.digits(2)?,
        };
        (hours <= 23 && minutes <= 59).then_some(sign * (hours * 3600 + minutes * 60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_offsets_and_fractions() {
        assert_eq!(parse("1970-01-01T00:00:00Z"), Ok(0.0));
        assert_eq!(parse("2026-09-28T08:00:00Z"), Ok(1_790_582_400.0));
        assert_eq!(parse("2026-09-28T10:00:00+02:00"), Ok(1_790_582_400.0));
        assert_eq!(parse("2026-09-28T10:00:00+0200"), Ok(1_790_582_400.0));
        assert_eq!(parse("2026-09-28T10:00:00+02"), Ok(1_790_582_400.0));
        assert_eq!(parse("2026-09-28t08:00:00z"), Ok(1_790_582_400.0));
        assert_eq!(parse("2026-09-28 08:00:00"), Ok(1_790_582_400.0));
        assert_eq!(parse("2026-09-28T08:00Z"), Ok(1_790_582_400.0));
        assert_eq!(parse("2026-09-28T08:00:00.25Z"), Ok(1_790_582_400.25));
        assert_eq!(parse("2026-09-28T08:00:00,5Z"), Ok(1_790_582_400.5));
        assert_eq!(parse("  2026-09-28  "), Ok(1_790_553_600.0));
    }

    #[test]
    fn a_negative_offset_crosses_midnight() {
        // 22:30 in New York (UTC-05:00) is 03:30 the next day in UTC
        let t = parse("2026-12-31T22:30:00-05:00").unwrap();
        assert_eq!(format(t).unwrap(), "2027-01-01T03:30:00Z");
        assert_eq!(format_date(t).unwrap(), "2027-01-01");
    }

    #[test]
    fn the_boundaries_of_1970_and_of_the_range() {
        assert_eq!(parse("1969-12-31T23:59:59Z"), Ok(-1.0));
        assert_eq!(format(-0.5).unwrap(), "1969-12-31T23:59:59.500Z");
        assert_eq!(format(0.0).unwrap(), "1970-01-01T00:00:00Z");
        assert_eq!(format(FIRST as f64).unwrap(), "0000-01-01T00:00:00Z");
        assert_eq!(format(END as f64 - 1.0).unwrap(), "9999-12-31T23:59:59Z");
        assert!(format(END as f64).is_err() && format(FIRST as f64 - 1.0).is_err());
        assert!(format(f64::NAN).is_err() && format(f64::INFINITY).is_err());
    }

    #[test]
    fn leap_years() {
        assert_eq!(format(parse("2024-02-29T12:00:00Z").unwrap()).unwrap(), "2024-02-29T12:00:00Z");
        assert_eq!(format(parse("2000-02-29").unwrap()).unwrap(), "2000-02-29T00:00:00Z");
        assert!(parse("2026-02-29").is_err());
        assert!(parse("1900-02-29").is_err());
        assert_eq!(parse("2024-03-01").unwrap() - parse("2024-02-28").unwrap(), 2.0 * 86_400.0);
    }

    #[test]
    fn round_trips() {
        for text in ["2026-09-28T08:00:00Z", "1999-12-31T23:59:59.999Z", "1968-05-01T00:00:00.250Z"] {
            assert_eq!(format(parse(text).unwrap()).unwrap(), text);
        }
    }

    #[test]
    fn weekdays() {
        let weekday = |text| civil_of(parse(text).unwrap()).unwrap().weekday;
        assert_eq!(weekday("1970-01-01"), 4); // a Thursday
        assert_eq!(weekday("2026-09-28"), 1); // a Monday
        assert_eq!(weekday("2026-10-04"), 7); // a Sunday
        assert_eq!(weekday("2000-02-29"), 2); // a Tuesday
    }

    #[test]
    fn what_is_refused_and_why() {
        for (text, reason) in [
            ("", "no date"),
            ("2026-9-28", "no date"),
            ("28/09/2026", "no date"),
            ("2026-13-01", "month 13"),
            ("2026-04-31", "day 31"),
            ("2026-09-28X08:00", "no `T`"),
            ("2026-09-28T8:00", "no time"),
            ("2026-09-28T24:00:00Z", "not a time of day"),
            ("2026-09-28T08:60:00Z", "not a time of day"),
            ("2026-09-28T08:00:61Z", "second 61"),
            ("2026-09-28T08:00:00.Z", "no digits"),
            ("2026-09-28T08:00:00+2:00", "an offset"),
            ("2026-09-28T08:00:00+24:00", "an offset"),
            ("2026-09-28T08:00:00Z junk", "unexpected text"),
        ] {
            let error = parse(text).unwrap_err();
            assert!(error.contains(reason) && error.contains(EXPECTED), "{text}: {error}");
        }
    }

    #[test]
    fn civil_instants() {
        assert_eq!(seconds_of(2026, 9, 28, 8, 0, 0.0), Ok(1_790_582_400.0));
        assert_eq!(seconds_of(1969, 12, 31, 23, 59, 59.5), Ok(-0.5));
        assert!(seconds_of(2026, 2, 29, 0, 0, 0.0).is_err());
        assert!(seconds_of(10_000, 1, 1, 0, 0, 0.0).is_err());
        assert!(seconds_of(2026, 0, 1, 0, 0, 0.0).is_err());
    }
}
