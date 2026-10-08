//! Replays refused: a signed request is accepted once. A Slack signature
//! covers its timestamp, and a timestamp more than five minutes old is
//! refused (`signature::slack_check`); within those minutes, the same
//! signature seen again is a request captured and sent again. Signatures
//! are kept as long as their timestamp could still pass, then forgotten.

use std::collections::HashMap;

use crate::signature::SLACK_TOLERANCE;

/// The signatures accepted lately, with their timestamp.
#[derive(Debug, Default)]
pub struct Replays {
    seen: HashMap<String, i64>,
}

impl Replays {
    /// Whether `signature` (of a request sent at `timestamp`) is seen for the
    /// first time; it is remembered. `now` forgets the signatures whose
    /// timestamp could no longer pass.
    pub fn first(&mut self, signature: &str, timestamp: i64, now: i64) -> bool {
        self.seen.retain(|_, sent| (now - *sent).abs() <= SLACK_TOLERANCE);
        if self.seen.contains_key(signature) {
            return false;
        }
        self.seen.insert(signature.to_string(), timestamp);
        true
    }

    /// Whether `signature` was accepted lately (a test double signs anew
    /// what it would otherwise send twice).
    pub fn seen(&self, signature: &str) -> bool {
        self.seen.contains_key(signature)
    }

    /// How many signatures are remembered.
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_passes_once() {
        let mut replays = Replays::default();
        assert!(replays.first("v0=a", 1000, 1000));
        assert!(!replays.first("v0=a", 1000, 1010));
        assert!(replays.seen("v0=a"));
        assert!(replays.first("v0=b", 1001, 1010));
        assert_eq!(replays.len(), 2);
    }

    #[test]
    fn signatures_are_forgotten_once_their_timestamp_is_stale() {
        let mut replays = Replays::default();
        assert!(replays.first("v0=a", 1000, 1000));
        assert!(replays.first("v0=b", 1200, 1200));
        // `v0=a` could no longer pass: it is forgotten, `v0=b` is not
        assert!(replays.first("v0=c", 1301, 1301));
        assert!(!replays.seen("v0=a"));
        assert!(!replays.first("v0=b", 1200, 1301));
        assert_eq!(replays.len(), 2);
    }
}
