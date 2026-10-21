//! What a watch remembers between two scans: when each file last changed,
//! and what of it was handed over.
//!
//! **Settled.** A file is *ready* once its [`Stamp`] (size and time of last
//! change) has stayed the same for the settling time: one still being
//! written, copied or synced is never handed over half-written.
//!
//! **Once per content.** A ready file is read and its [`digest`] taken;
//! it is handed over only when that digest is not the one handed over last
//! for its path. Touching a file, or a sync rewriting it as it was, hands
//! nothing; new content at the same path does. Digests may come from an
//! earlier run ([`Tracker::remember`]).
//!
//! **A file gone is forgotten**: dropped again, it is handed over again.
//!
//! **Failures.** A run that fails leaves the content unhandled: tried again
//! after the retry delay, [`MAX_ATTEMPTS`] times in all, then given up —
//! counted as handled, until the content changes.
//!
//! [`digest`]: crate::digest

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::{Found, Stamp};

/// Runs of a handler on one content before it is given up.
pub const MAX_ATTEMPTS: u32 = 3;

pub struct Tracker {
    settle: Duration,
    retry: Duration,
    files: HashMap<PathBuf, File>,
    /// Digests handed over by an earlier run, until their path is seen.
    remembered: HashMap<PathBuf, String>,
    /// Whether a scan was observed yet (what `remembered` lacks is gone).
    started: bool,
}

struct File {
    stamp: Stamp,
    /// When `stamp` was first seen.
    since: Instant,
    /// The state whose content was looked at: handed over, the same as
    /// before, given up, or there before the watch when it skips those.
    checked: Option<Stamp>,
    /// The content handed over last.
    digest: Option<String>,
    failures: u32,
    retry_at: Option<Instant>,
}

/// What a scan changed.
#[derive(Debug, Default, PartialEq)]
pub struct Observed {
    /// Settled files whose content is to be looked at, in path order.
    pub ready: Vec<PathBuf>,
    /// Files that are no longer there.
    pub gone: Vec<PathBuf>,
}

impl Tracker {
    /// A file is ready after `settle` without change; a failed run is tried
    /// again after `retry`.
    pub fn new(settle: Duration, retry: Duration) -> Tracker {
        Tracker { settle, retry, files: HashMap::new(), remembered: HashMap::new(), started: false }
    }

    /// The content handed over for `path` by an earlier run.
    pub fn remember(&mut self, path: PathBuf, digest: String) {
        self.remembered.insert(path, digest);
    }

    /// The files there now count as handled, as they are: only what is
    /// dropped or changed from now on is handed over.
    pub fn skip_present(&mut self, found: &[Found], now: Instant) {
        for file in found {
            let digest = self.remembered.remove(&file.path);
            self.files.insert(
                file.path.clone(),
                File { stamp: file.stamp, since: now, checked: Some(file.stamp), digest, failures: 0, retry_at: None },
            );
        }
    }

    /// Takes in a scan's files at `now`.
    pub fn observe(&mut self, found: &[Found], now: Instant) -> Observed {
        let mut out = Observed::default();
        let present: std::collections::HashSet<&Path> = found.iter().map(|f| f.path.as_path()).collect();
        self.files.retain(|path, _| {
            let kept = present.contains(path.as_path());
            if !kept {
                out.gone.push(path.clone());
            }
            kept
        });
        if !std::mem::replace(&mut self.started, true) {
            let remembered = std::mem::take(&mut self.remembered);
            for (path, digest) in remembered {
                if present.contains(path.as_path()) {
                    self.remembered.insert(path, digest);
                } else {
                    out.gone.push(path);
                }
            }
        }
        for file in found {
            let remembered = &mut self.remembered;
            let entry = self.files.entry(file.path.clone()).or_insert_with(|| File {
                stamp: file.stamp,
                since: now,
                checked: None,
                digest: remembered.remove(&file.path),
                failures: 0,
                retry_at: None,
            });
            if entry.stamp != file.stamp {
                // still changing: settling starts again, and so do its attempts
                entry.stamp = file.stamp;
                entry.since = now;
                entry.failures = 0;
                entry.retry_at = None;
            }
            let settled = now.duration_since(entry.since) >= self.settle;
            let due = entry.retry_at.is_none_or(|at| now >= at);
            if settled && due && entry.checked != Some(entry.stamp) {
                out.ready.push(file.path.clone());
            }
        }
        out.ready.sort();
        out.gone.sort();
        out
    }

    /// How long until a file settles or a run is tried again: when the
    /// next scan is worth making. `None`: nothing is waiting.
    pub fn next_check(&self, now: Instant) -> Option<Duration> {
        self.files
            .values()
            .filter(|f| f.checked != Some(f.stamp))
            .map(|f| {
                let settled = f.since + self.settle;
                f.retry_at.map_or(settled, |at| at.max(settled))
            })
            .min()
            .map(|at| at.saturating_duration_since(now))
    }

    /// Whether `digest` is new content for `path`, to hand over; when it
    /// is not, the file's state counts as looked at.
    pub fn is_new(&mut self, path: &Path, digest: &str) -> bool {
        let Some(file) = self.files.get_mut(path) else { return false };
        if file.digest.as_deref() == Some(digest) {
            file.checked = Some(file.stamp);
            return false;
        }
        true
    }

    /// `path` as it stands is never handed over (refused when opened): its
    /// state counts as looked at, what was handed over last stays.
    pub fn passed_over(&mut self, path: &Path) {
        if let Some(file) = self.files.get_mut(path) {
            file.checked = Some(file.stamp);
        }
    }

    /// `path`'s content (`digest`) was handed over, or given up.
    pub fn handled(&mut self, path: &Path, digest: &str) {
        if let Some(file) = self.files.get_mut(path) {
            file.checked = Some(file.stamp);
            file.digest = Some(digest.to_string());
            file.failures = 0;
            file.retry_at = None;
        }
    }

    /// A run on `path` failed at `now`: whether it is given up now (the
    /// caller then calls [`Tracker::handled`]).
    pub fn failed(&mut self, path: &Path, now: Instant) -> bool {
        let Some(file) = self.files.get_mut(path) else { return true };
        file.failures += 1;
        if file.failures >= MAX_ATTEMPTS {
            return true;
        }
        file.retry_at = Some(now + self.retry);
        false
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;

    const SETTLE: Duration = Duration::from_secs(2);
    const RETRY: Duration = Duration::from_secs(10);

    fn found(path: &str, size: u64, modified: u64) -> Found {
        let modified = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(modified));
        Found { path: PathBuf::from(path), stamp: Stamp { size, modified } }
    }

    fn ready(observed: Observed) -> Vec<String> {
        observed.ready.iter().map(|p| p.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn a_file_is_ready_once_it_stops_changing() {
        let mut t = Tracker::new(SETTLE, RETRY);
        let t0 = Instant::now();
        assert!(ready(t.observe(&[found("a", 10, 1)], t0)).is_empty());
        assert_eq!(t.next_check(t0), Some(SETTLE));
        // still growing: settling starts again
        assert!(ready(t.observe(&[found("a", 20, 2)], t0 + Duration::from_secs(1))).is_empty());
        assert!(ready(t.observe(&[found("a", 20, 2)], t0 + Duration::from_secs(2))).is_empty());
        assert_eq!(ready(t.observe(&[found("a", 20, 2)], t0 + Duration::from_secs(3))), ["a"]);
        assert!(t.is_new(Path::new("a"), "d1"));
        t.handled(Path::new("a"), "d1");
        assert!(ready(t.observe(&[found("a", 20, 2)], t0 + Duration::from_secs(9))).is_empty());
        assert_eq!(t.next_check(t0), None);
    }

    #[test]
    fn the_same_content_is_handed_once() {
        let mut t = Tracker::new(SETTLE, RETRY);
        let t0 = Instant::now();
        t.observe(&[found("a", 1, 1)], t0);
        assert_eq!(ready(t.observe(&[found("a", 1, 1)], t0 + SETTLE)), ["a"]);
        t.handled(Path::new("a"), "d1");
        // touched: looked at, the same content, not handed again
        t.observe(&[found("a", 1, 5)], t0 + SETTLE);
        assert_eq!(ready(t.observe(&[found("a", 1, 5)], t0 + SETTLE * 2)), ["a"]);
        assert!(!t.is_new(Path::new("a"), "d1"));
        assert!(ready(t.observe(&[found("a", 1, 5)], t0 + SETTLE * 3)).is_empty());
        // new content: handed
        t.observe(&[found("a", 2, 6)], t0 + SETTLE * 3);
        assert_eq!(ready(t.observe(&[found("a", 2, 6)], t0 + SETTLE * 4)), ["a"]);
        assert!(t.is_new(Path::new("a"), "d2"));
    }

    #[test]
    fn a_file_gone_is_forgotten() {
        let mut t = Tracker::new(SETTLE, RETRY);
        let t0 = Instant::now();
        t.observe(&[found("a", 1, 1)], t0);
        t.observe(&[found("a", 1, 1)], t0 + SETTLE);
        t.handled(Path::new("a"), "d1");
        assert_eq!(t.observe(&[], t0 + SETTLE).gone, [PathBuf::from("a")]);
        // dropped again: handed again
        t.observe(&[found("a", 1, 1)], t0 + SETTLE);
        assert_eq!(ready(t.observe(&[found("a", 1, 1)], t0 + SETTLE * 2)), ["a"]);
        assert!(t.is_new(Path::new("a"), "d1"));
    }

    #[test]
    fn a_failing_run_is_tried_again_then_given_up() {
        let mut t = Tracker::new(Duration::ZERO, RETRY);
        let t0 = Instant::now();
        let a = Path::new("a");
        assert_eq!(ready(t.observe(&[found("a", 1, 1)], t0)), ["a"]);
        assert!(!t.failed(a, t0));
        assert!(ready(t.observe(&[found("a", 1, 1)], t0 + RETRY / 2)).is_empty());
        assert_eq!(t.next_check(t0), Some(RETRY));
        assert_eq!(ready(t.observe(&[found("a", 1, 1)], t0 + RETRY)), ["a"]);
        assert!(!t.failed(a, t0 + RETRY));
        assert!(t.failed(a, t0 + RETRY * 2));
        t.handled(a, "d1");
        assert!(ready(t.observe(&[found("a", 1, 1)], t0 + RETRY * 5)).is_empty());
        // changed: its attempts start again
        assert_eq!(ready(t.observe(&[found("a", 2, 2)], t0 + RETRY * 5)), ["a"]);
        assert!(!t.failed(a, t0 + RETRY * 5));
    }

    #[test]
    fn a_file_passed_over_waits_until_it_changes() {
        let mut t = Tracker::new(Duration::ZERO, RETRY);
        let t0 = Instant::now();
        let a = Path::new("a");
        assert_eq!(ready(t.observe(&[found("a", 1, 1)], t0)), ["a"]);
        t.handled(a, "d1");
        assert_eq!(ready(t.observe(&[found("a", 2, 2)], t0)), ["a"]);
        t.passed_over(a);
        assert!(ready(t.observe(&[found("a", 2, 2)], t0 + RETRY)).is_empty());
        assert_eq!(t.next_check(t0), None);
        // back as it was: the same content as handed, nothing to hand
        assert_eq!(ready(t.observe(&[found("a", 1, 1)], t0 + RETRY)), ["a"]);
        assert!(!t.is_new(a, "d1"));
    }

    #[test]
    fn files_there_at_the_start() {
        let t0 = Instant::now();
        // skipped: only what changes afterwards
        let mut t = Tracker::new(Duration::ZERO, RETRY);
        t.skip_present(&[found("a", 1, 1)], t0);
        assert!(ready(t.observe(&[found("a", 1, 1), found("b", 1, 1)], t0)) == ["b"]);
        assert_eq!(ready(t.observe(&[found("a", 2, 2)], t0)), ["a"]);
        // remembered from an earlier run: looked at, handed only if changed
        let mut t = Tracker::new(Duration::ZERO, RETRY);
        t.remember(PathBuf::from("a"), "d1".into());
        t.remember(PathBuf::from("gone"), "d9".into());
        let observed = t.observe(&[found("a", 1, 1)], t0);
        assert_eq!(observed.gone, [PathBuf::from("gone")]);
        assert_eq!(ready(observed), ["a"]);
        assert!(!t.is_new(Path::new("a"), "d1"));
    }
}
