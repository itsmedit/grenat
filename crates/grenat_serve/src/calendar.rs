//! The calendar (proleptic Gregorian, UTC), without a date library.

/// (year, month, day, hour, minute, weekday) of `t` seconds since the epoch, UTC.
pub fn civil(t: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = t.div_euclid(86_400);
    let seconds = t.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    let weekday = (days + 4).rem_euclid(7) as u32; // 1970-01-01 was a Thursday
    (year, month, day, (seconds / 3600) as u32, (seconds % 3600 / 60) as u32, weekday)
}

/// Days since the epoch of a civil date (`days_from_civil`, its inverse).
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    // Howard Hinnant's days_from_civil
    let (month, day) = (i64::from(month), i64::from(day));
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Whether `year` has a 29 February.
pub fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// The number of days of `month` (1 to 12) in `year`; 0 for no month.
pub fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// `YYYY-MM-DD` of `t` seconds since the epoch, UTC.
pub fn date(t: i64) -> String {
    let (year, month, day, ..) = civil(t);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(date(-86_400), "1969-12-31");
    }

    #[test]
    fn days_from_civil_inverts_civil() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert_eq!(days_from_civil(2000, 2, 29), 11_016);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        for days in (-800_000..800_000).step_by(997) {
            let (year, month, day, ..) = civil(days * 86_400);
            assert_eq!(days_from_civil(year, month, day), days, "{year}-{month}-{day}");
        }
    }

    #[test]
    fn months_and_leap_years() {
        assert!(is_leap(2000) && is_leap(2024) && !is_leap(1900) && !is_leap(2026));
        assert_eq!((days_in_month(2024, 2), days_in_month(2026, 2), days_in_month(1900, 2)), (29, 28, 28));
        assert_eq!((days_in_month(2026, 4), days_in_month(2026, 12), days_in_month(2026, 13)), (30, 31, 0));
    }
}
