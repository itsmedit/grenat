//! A stand-in for the operating system's notifications, for tests: the
//! [`Wake`]s it hands are registered at once (a directory that is not
//! there is refused, as the system would), and told of a change only when
//! the test says so ([`FakeNotifier::change`]). What reacts to a wake-up —
//! a scan, settling, once per content — is then tested without waiting for
//! FSEvents' daemon or inotify; `tests/wake.rs` proves the system's own.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use crate::Wake;
use crate::wake::{Signal, State};

/// Hands [`Wake`]s told of changes by the test; clones share them.
#[derive(Clone, Default)]
pub struct FakeNotifier(Arc<Mutex<Registered>>);

#[derive(Default)]
struct Registered {
    watched: Vec<Watched>,
    /// Registrations so far, dropped or not.
    count: usize,
}

struct Watched {
    dir: PathBuf,
    recursive: bool,
    signal: Weak<Signal>,
    state: Weak<Mutex<State>>,
}

impl Watched {
    /// Whether its `Wake` is still there, watching.
    fn watching(&self) -> Option<Arc<Signal>> {
        let state = self.state.upgrade()?;
        let watching = matches!(*state.lock().unwrap_or_else(PoisonError::into_inner), State::Watching(_));
        watching.then(|| self.signal.upgrade()).flatten()
    }

    /// Whether a change of `path` is told: the directory itself (removed,
    /// renamed over), what is in it, and deeper when recursive.
    fn covers(&self, path: &Path) -> bool {
        path == self.dir || if self.recursive { path.starts_with(&self.dir) } else { path.parent() == Some(&self.dir) }
    }
}

impl FakeNotifier {
    /// Watches `dir` (and what is under it when `recursive`): registered at
    /// once, and woken once, as [`Wake::watch`] is once registered.
    pub fn watch(&self, dir: &Path, recursive: bool) -> Wake {
        let signal = Arc::<Signal>::default();
        let watching = dir.is_dir();
        let state = Arc::new(Mutex::new(match watching {
            true => State::Watching(None),
            false => State::Stopped(format!("{} is not a directory", dir.display())),
        }));
        if watching {
            let mut registered = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            registered.count += 1;
            registered.watched.push(Watched {
                dir: dir.to_path_buf(),
                recursive,
                signal: Arc::downgrade(&signal),
                state: Arc::downgrade(&state),
            });
            signal.raise();
        }
        Wake::with(signal, state)
    }

    /// Tells of a change of `path` (a file created, written, renamed in or
    /// out; the directory watched itself): wakes the watches that cover it;
    /// how many.
    pub fn change(&self, path: &Path) -> usize {
        let watched = &mut self.0.lock().unwrap_or_else(PoisonError::into_inner).watched;
        watched.retain(|w| w.state.strong_count() > 0);
        let woken: Vec<Arc<Signal>> = watched.iter().filter(|w| w.covers(path)).filter_map(Watched::watching).collect();
        woken.iter().for_each(|signal| signal.raise());
        woken.len()
    }

    /// How many watches were registered so far, dropped or not.
    pub fn registrations(&self) -> usize {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).count
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("grenat-watch-fake-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("deep/er")).unwrap();
        dir
    }

    #[test]
    fn a_change_wakes_the_watches_that_cover_it_only() {
        let dir = temp("cover");
        let fake = FakeNotifier::default();
        let (flat, recursive) = (fake.watch(&dir, false), fake.watch(&dir, true));
        assert!(flat.is_watching() && recursive.is_watching());
        let (flat_waiter, recursive_waiter) = (flat.waiter(), recursive.waiter());
        // registered: woken once
        assert!(flat_waiter.wait(Duration::ZERO) && recursive_waiter.wait(Duration::ZERO));
        assert!(!flat_waiter.wait(Duration::ZERO));
        assert_eq!(fake.change(&dir.join("a.pdf")), 2);
        assert!(flat_waiter.wait(Duration::ZERO) && recursive_waiter.wait(Duration::ZERO));
        assert_eq!(fake.change(&dir.join("deep/er/a.pdf")), 1);
        assert!(!flat_waiter.wait(Duration::ZERO) && recursive_waiter.wait(Duration::ZERO));
        assert_eq!(fake.change(&dir), 2);
        assert!(flat_waiter.wait(Duration::ZERO) && recursive_waiter.wait(Duration::ZERO));
        assert_eq!(fake.change(&std::env::temp_dir().join("elsewhere.pdf")), 0);
        assert_eq!(fake.registrations(), 2);
    }

    #[test]
    fn a_watch_dropped_is_woken_no_more_and_a_missing_directory_is_refused() {
        let dir = temp("dropped");
        let fake = FakeNotifier::default();
        let wake = fake.watch(&dir, false);
        let waiter = wake.waiter();
        drop(wake);
        assert!(waiter.wait(Duration::ZERO));
        assert_eq!(fake.change(&dir.join("a.pdf")), 0);
        assert!(!waiter.wait(Duration::ZERO));
        let missing = fake.watch(&dir.join("missing"), false);
        assert!(!missing.is_watching());
        assert!(missing.stopped().unwrap().contains("not a directory"));
        assert_eq!(fake.change(&dir.join("missing/a.pdf")), 0);
    }
}
