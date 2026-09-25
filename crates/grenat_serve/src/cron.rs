//! Cron schedules: `minute hour day-of-month month day-of-week`, in UTC.
//!
//! Each field is `*`, a number, a range `1-5`, a list `1,15`, a step
//! `*/15` or `0-30/10`; months and days of the week may be named
//! (`JAN`, `MON`); Sunday is 0 (or 7).

#[derive(Debug, Clone, PartialEq)]
pub struct Cron {
    minutes: Vec<bool>,
    hours: Vec<bool>,
    days: Vec<bool>,
    months: Vec<bool>,
    weekdays: Vec<bool>,
    /// Day of month and day of week both restricted: either one matches (as cron does).
    either_day: bool,
}

use crate::calendar::civil;

const MONTHS: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];
const WEEKDAYS: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

impl Cron {
    pub fn parse(text: &str) -> Result<Cron, String> {
        let fields: Vec<&str> = text.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = fields.as_slice() else {
            return Err(format!("`{text}`: a schedule has 5 fields: minute hour day month weekday"));
        };
        let mut weekdays = field(weekday, 0, 7, &WEEKDAYS, 0)?;
        if weekdays[7] {
            weekdays[0] = true;
        }
        weekdays.truncate(7);
        Ok(Cron {
            minutes: field(minute, 0, 59, &[], 0)?,
            hours: field(hour, 0, 23, &[], 0)?,
            days: field(day, 1, 31, &[], 0)?,
            months: field(month, 1, 12, &MONTHS, 1)?,
            weekdays,
            either_day: *day != "*" && *weekday != "*",
        })
    }

    /// The first time strictly after `after` (seconds since the epoch, UTC)
    /// that the schedule matches, to the minute.
    pub fn next(&self, after: i64) -> Option<i64> {
        let mut t = (after.div_euclid(60) + 1) * 60;
        // a schedule matches at least once in 4 years (29 February)
        for _ in 0..(4 * 366 * 24 * 60) {
            let (_, month, day, hour, minute, weekday) = civil(t);
            let day_ok = if self.either_day {
                self.days[day as usize] || self.weekdays[weekday as usize]
            } else {
                self.days[day as usize] && self.weekdays[weekday as usize]
            };
            if !self.months[month as usize] || !day_ok {
                t = (t.div_euclid(86_400) + 1) * 86_400;
            } else if !self.hours[hour as usize] {
                t = (t.div_euclid(3600) + 1) * 3600;
            } else if !self.minutes[minute as usize] {
                t += 60;
            } else {
                return Some(t);
            }
        }
        None
    }
}

/// Which values of `min..=max` a field allows (indexed by value).
fn field(text: &str, min: u32, max: u32, names: &[&str], first_name: u32) -> Result<Vec<bool>, String> {
    let mut allowed = vec![false; max as usize + 1];
    let value = |s: &str| -> Result<u32, String> {
        if let Some(i) = names.iter().position(|n| n.eq_ignore_ascii_case(s)) {
            return Ok(i as u32 + first_name);
        }
        let n: u32 = s.parse().map_err(|_| format!("`{s}` is not a valid value"))?;
        if n < min || n > max {
            return Err(format!("`{n}` is out of range {min}-{max}"));
        }
        Ok(n)
    };
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => (r, s.parse::<u32>().ok().filter(|s| *s > 0).ok_or(format!("`{part}`: invalid step"))?),
            None => (part, 1),
        };
        let (lo, hi) = match range {
            "*" => (min, max),
            r => match r.split_once('-') {
                Some((a, b)) => (value(a)?, value(b)?),
                None if step > 1 => (value(r)?, max),
                None => (value(r)?, value(r)?),
            },
        };
        if lo > hi {
            return Err(format!("`{part}`: empty range"));
        }
        for v in (lo..=hi).step_by(step as usize) {
            allowed[v as usize] = true;
        }
    }
    Ok(allowed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-25 12:34:56 UTC, a Friday.
    const NOW: i64 = 1_790_339_696;

    fn at(t: i64) -> String {
        let (y, mo, d, h, mi, wd) = civil(t);
        format!("{y}-{mo:02}-{d:02} {h:02}:{mi:02} {}", WEEKDAYS[wd as usize])
    }

    #[test]
    fn calendar() {
        assert_eq!(at(0), "1970-01-01 00:00 THU");
        assert_eq!(at(NOW), "2026-09-25 12:34 FRI");
        assert_eq!(at(951_782_400), "2000-02-29 00:00 TUE");
    }

    #[test]
    fn next_occurrences() {
        let next = |s: &str| at(Cron::parse(s).unwrap().next(NOW).unwrap());
        assert_eq!(next("* * * * *"), "2026-09-25 12:35 FRI");
        assert_eq!(next("*/15 * * * *"), "2026-09-25 12:45 FRI");
        assert_eq!(next("0 8 * * MON"), "2026-09-28 08:00 MON");
        assert_eq!(next("0 8 * * 1-5"), "2026-09-28 08:00 MON");
        assert_eq!(next("30 9 1 * *"), "2026-10-01 09:30 THU");
        assert_eq!(next("0 0 1 JAN *"), "2027-01-01 00:00 FRI");
        assert_eq!(next("0 12 29 2 *"), "2028-02-29 12:00 TUE");
        assert_eq!(next("0 0 * * 7"), "2026-09-27 00:00 SUN");
        // day of month or day of week, as cron does
        assert_eq!(next("0 0 1 * FRI"), "2026-10-01 00:00 THU");
    }

    #[test]
    fn invalid_schedules() {
        assert!(Cron::parse("* * * *").unwrap_err().contains("5 fields"));
        assert_eq!(Cron::parse("60 * * * *").unwrap_err(), "`60` is out of range 0-59");
        assert_eq!(Cron::parse("* * * * FUN").unwrap_err(), "`FUN` is not a valid value");
        assert_eq!(Cron::parse("*/0 * * * *").unwrap_err(), "`*/0`: invalid step");
    }
}
