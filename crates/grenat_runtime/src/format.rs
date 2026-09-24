//! Text of numbers, shared by the interpreter and native code.

/// A `Float` as Grenat prints it: `3.0`, `0.1`, `1e16`, `NaN`.
pub fn float(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e16 { format!("{f:.1}") } else { f.to_string() }
}

#[cfg(test)]
mod tests {
    #[test]
    fn floats_always_show_a_fraction() {
        assert_eq!(super::float(3.0), "3.0");
        assert_eq!(super::float(-0.5), "-0.5");
        assert_eq!(super::float(1e16), "10000000000000000");
        assert_eq!(super::float(f64::NAN), "NaN");
    }
}
