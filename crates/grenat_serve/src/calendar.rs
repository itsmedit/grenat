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
}
