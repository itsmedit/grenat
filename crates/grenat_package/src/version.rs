//! Versions (`1.2.3`, from tags such as `v1.2.3`) and requirements
//! (`~> 1.2`, `>= 1.0, < 2`, `= 1.0.0`), with Ruby's meanings.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    /// `1.2.3`, `v1.2.3`, `1.2` (patch 0); anything else is not a version.
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim().strip_prefix('v').unwrap_or(text.trim());
        let parts: Vec<u64> = text.split('.').map(|p| p.parse().ok()).collect::<Option<_>>()?;
        match parts.as_slice() {
            [major, minor] => Some(Version { major: *major, minor: *minor, patch: 0 }),
            [major, minor, patch] => Some(Version { major: *major, minor: *minor, patch: *patch }),
            _ => None,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Eq,
    Gt,
    Ge,
    Lt,
    Le,
    /// `~>`: this version or later, below the next release of its
    /// next-to-last part (`~> 1.2` < 2.0, `~> 1.2.3` < 1.3).
    Pessimistic {
        parts: usize,
    },
}

/// Every condition must hold.
#[derive(Debug, Clone, PartialEq)]
pub struct Requirement {
    conditions: Vec<(Op, Version)>,
    text: String,
}

impl Requirement {
    pub fn parse(text: &str) -> Result<Requirement, String> {
        let mut conditions = Vec::new();
        for part in text.split(',') {
            let part = part.trim();
            let (op, rest) = ["~>", ">=", "<=", "=", ">", "<"]
                .iter()
                .find_map(|op| part.strip_prefix(op).map(|rest| (*op, rest.trim())))
                .unwrap_or(("=", part));
            // in a requirement, `2` is 2.0.0
            let whole =
                rest.trim_start_matches('v').parse::<u64>().ok().map(|major| Version { major, minor: 0, patch: 0 });
            let version =
                Version::parse(rest).or(whole).ok_or_else(|| format!("`{part}`: expected a version such as 1.2.3"))?;
            let op = match op {
                "~>" => Op::Pessimistic { parts: rest.trim_start_matches('v').split('.').count() },
                ">=" => Op::Ge,
                "<=" => Op::Le,
                ">" => Op::Gt,
                "<" => Op::Lt,
                _ => Op::Eq,
            };
            conditions.push((op, version));
        }
        Ok(Requirement { conditions, text: text.trim().to_string() })
    }

    /// Any version.
    pub fn any() -> Requirement {
        Requirement { conditions: Vec::new(), text: ">= 0".into() }
    }

    pub fn matches(&self, v: &Version) -> bool {
        self.conditions.iter().all(|(op, r)| match op {
            Op::Eq => v == r,
            Op::Gt => v > r,
            Op::Ge => v >= r,
            Op::Lt => v < r,
            Op::Le => v <= r,
            Op::Pessimistic { parts } => {
                let below = if *parts <= 2 {
                    Version { major: r.major + 1, minor: 0, patch: 0 }
                } else {
                    Version { major: r.major, minor: r.minor + 1, patch: 0 }
                };
                v >= r && *v < below
            }
        })
    }

    /// The requirement a new dependency gets: `~> major.minor`.
    pub fn compatible_with(v: &Version) -> Requirement {
        Requirement::parse(&format!("~> {}.{}", v.major, v.minor)).expect("a valid requirement")
    }
}

impl fmt::Display for Requirement {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// The highest of `versions` every requirement allows.
pub fn highest<'v>(versions: impl IntoIterator<Item = &'v Version>, requirements: &[Requirement]) -> Option<Version> {
    versions.into_iter().filter(|v| requirements.iter().all(|r| r.matches(v))).max().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn versions() {
        assert_eq!(v("v1.2.3"), Version { major: 1, minor: 2, patch: 3 });
        assert_eq!(v("0.3"), v("0.3.0"));
        assert!(v("1.10.0") > v("1.9.9"));
        assert_eq!(Version::parse("latest"), None);
        assert_eq!(Version::parse("1"), None);
        assert_eq!(v("2.0.1").to_string(), "2.0.1");
    }

    #[test]
    fn requirements() {
        let r = |text: &str| Requirement::parse(text).unwrap();
        assert!(r("~> 0.3").matches(&v("0.3.0")) && r("~> 0.3").matches(&v("0.9.1")));
        assert!(!r("~> 0.3").matches(&v("1.0.0")) && !r("~> 0.3").matches(&v("0.2.9")));
        assert!(r("~> 1.2.3").matches(&v("1.2.9")) && !r("~> 1.2.3").matches(&v("1.3.0")));
        assert!(r(">= 1.0, < 2").matches(&v("1.5.0")) && !r(">= 1.0, < 2").matches(&v("2.0.0")));
        assert!(r("1.0.0").matches(&v("1.0.0")) && !r("= 1.0.0").matches(&v("1.0.1")));
        assert_eq!(Requirement::parse("~> latest").unwrap_err(), "`~> latest`: expected a version such as 1.2.3");
        assert_eq!(Requirement::compatible_with(&v("0.4.2")).to_string(), "~> 0.4");
        let versions = [v("0.2.0"), v("0.3.1"), v("0.3.4"), v("1.0.0")];
        assert_eq!(highest(&versions, &[r("~> 0.3")]), Some(v("0.3.4")));
        assert_eq!(highest(&versions, &[r("~> 0.3"), r("< 0.3.2")]), Some(v("0.3.1")));
        assert_eq!(highest(&versions, &[r(">= 2")]), None);
    }
}
