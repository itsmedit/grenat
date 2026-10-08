//! A rate: so many calls per period (`10.per_second`, `5000.per_hour`).

use std::fmt;
use std::time::Duration;

/// So many calls per period.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rate {
    pub count: u32,
    pub period: Duration,
}

impl Rate {
    /// `count` calls per `seconds`: both positive, the count at most
    /// `u32::MAX`, the period at most a year.
    pub fn new(count: i64, seconds: f64) -> Result<Rate, String> {
        if count < 1 {
            return Err(format!("a rate allows at least one call, not {count}"));
        }
        let count = u32::try_from(count).map_err(|_| format!("a rate allows at most {} calls", u32::MAX))?;
        if !seconds.is_finite() || seconds <= 0.0 {
            return Err(format!("a rate's period is positive, not {seconds}s"));
        }
        if seconds > 366.0 * 86_400.0 {
            return Err("a rate's period is a year at most".into());
        }
        Ok(Rate { count, period: Duration::from_secs_f64(seconds) })
    }

    /// The time between two calls once the bucket is empty.
    pub fn interval(&self) -> Duration {
        self.period / self.count
    }
}

impl fmt::Display for Rate {
    /// `10 per second`, `5000 per hour`, `3 per 1.5s`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let unit = match self.period.as_secs_f64() {
            1.0 => "second".to_string(),
            60.0 => "minute".to_string(),
            3600.0 => "hour".to_string(),
            86_400.0 => "day".to_string(),
            other => format!("{other}s"),
        };
        write!(f, "{} per {unit}", self.count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_are_positive_and_shown_by_their_unit() {
        let rate = Rate::new(10, 1.0).unwrap();
        assert_eq!(rate.interval(), Duration::from_millis(100));
        assert_eq!(rate.to_string(), "10 per second");
        assert_eq!(Rate::new(5000, 3600.0).unwrap().to_string(), "5000 per hour");
        assert_eq!(Rate::new(3, 1.5).unwrap().to_string(), "3 per 1.5s");
        assert!(Rate::new(0, 1.0).unwrap_err().contains("at least one"));
        assert!(Rate::new(-2, 1.0).is_err());
        assert!(Rate::new(1, 0.0).unwrap_err().contains("positive"));
        assert!(Rate::new(1, f64::NAN).is_err());
        assert!(Rate::new(1, 400.0 * 86_400.0).unwrap_err().contains("a year"));
        assert!(Rate::new(i64::from(u32::MAX) + 1, 1.0).is_err());
    }
}
