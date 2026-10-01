//! A workbook's cell as text, as a person reads it in the sheet: numbers
//! in their usual form (`3`, `2.5`, never `3.0`), booleans as `true` and
//! `false`, dates as `2024-01-15` (with a time, `2024-01-15 10:30:00`; a
//! time alone, `10:30:00`), durations as `27:15:00`, errors as Excel
//! writes them (`#DIV/0!`), an empty cell as `""`.

use calamine::{Data, ExcelDateTime};

/// The text of `cell`.
pub fn text(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) | Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => float(*f),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(d) => datetime(d),
        Data::Error(e) => e.to_string(),
    }
}

/// A number as people write it: `3` rather than `3.0`, `0.1`, `-2.5`;
/// never an exponent (`100000000000000000000`).
pub fn float(f: f64) -> String {
    if f == 0.0 {
        // `-0` is not something anyone writes
        "0".to_string()
    } else {
        format!("{f}")
    }
}

fn datetime(d: &ExcelDateTime) -> String {
    if d.is_duration() {
        return duration(d.as_f64());
    }
    let (year, month, day, hour, minute, second, milli) = d.to_ymd_hms_milli();
    let time = clock(i64::from(hour), minute, second, milli);
    // less than a day after Excel's epoch: a time of day, without a date
    if (0.0..1.0).contains(&d.as_f64()) {
        return time;
    }
    let date = format!("{year:04}-{month:02}-{day:02}");
    if (hour, minute, second, milli) == (0, 0, 0, 0) { date } else { format!("{date} {time}") }
}

/// `days` (Excel counts durations in days) as hours, minutes and seconds:
/// `27:15:00`, `-01:30:00`.
fn duration(days: f64) -> String {
    let millis = (days * 86_400_000.0).round() as i64;
    let sign = if millis < 0 { "-" } else { "" };
    let millis = millis.unsigned_abs();
    let hours = (millis / 3_600_000) as i64;
    let minute = ((millis / 60_000) % 60) as u8;
    let second = ((millis / 1000) % 60) as u8;
    format!("{sign}{}", clock(hours, minute, second, (millis % 1000) as u16))
}

fn clock(hours: i64, minute: u8, second: u8, milli: u16) -> String {
    if milli == 0 {
        format!("{hours:02}:{minute:02}:{second:02}")
    } else {
        format!("{hours:02}:{minute:02}:{second:02}.{milli:03}")
    }
}

#[cfg(test)]
mod tests {
    use calamine::{CellErrorType, ExcelDateTimeType};

    use super::*;

    fn date(value: f64) -> Data {
        Data::DateTime(ExcelDateTime::new(value, ExcelDateTimeType::DateTime, false))
    }

    #[test]
    fn numbers_read_as_people_write_them() {
        assert_eq!(text(&Data::Float(3.0)), "3");
        assert_eq!(text(&Data::Float(2.5)), "2.5");
        assert_eq!(text(&Data::Float(-0.1)), "-0.1");
        assert_eq!(text(&Data::Float(-0.0)), "0");
        assert_eq!(text(&Data::Float(1e20)), "100000000000000000000");
        assert_eq!(text(&Data::Int(-42)), "-42");
    }

    #[test]
    fn other_cells_have_their_text() {
        assert_eq!(text(&Data::Empty), "");
        assert_eq!(text(&Data::String("a, \"b\"".into())), "a, \"b\"");
        assert_eq!(text(&Data::Bool(true)), "true");
        assert_eq!(text(&Data::Bool(false)), "false");
        assert_eq!(text(&Data::Error(CellErrorType::Div0)), "#DIV/0!");
        assert_eq!(text(&Data::DateTimeIso("2024-01-15T10:30:00".into())), "2024-01-15T10:30:00");
        assert_eq!(text(&Data::DurationIso("PT1H".into())), "PT1H");
    }

    #[test]
    fn dates_times_and_durations_are_readable() {
        // 2025-10-13 is Excel's day 45943
        assert_eq!(text(&date(45943.0)), "2025-10-13");
        assert_eq!(text(&date(45943.5)), "2025-10-13 12:00:00");
        assert_eq!(text(&date(45943.541)), "2025-10-13 12:59:02.400");
        assert_eq!(text(&date(0.4375)), "10:30:00");
        let duration = |days| Data::DateTime(ExcelDateTime::new(days, ExcelDateTimeType::TimeDelta, false));
        assert_eq!(text(&duration(1.0 + 3.25 / 24.0)), "27:15:00");
        assert_eq!(text(&duration(-1.5 / 24.0)), "-01:30:00");
        assert_eq!(text(&duration(0.0)), "00:00:00");
    }
}
