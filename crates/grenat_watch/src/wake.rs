//! Woken when a watched directory changes: the operating system tells
//! (`notify`: inotify on Linux, FSEvents on macOS), and [`Waiter::wait`]
//! returns early. Only a wake-up: what changed is the next scan's to find,
//! so an event lost, merged or never sent (a network drive, inotify's
//! watch limit reached) costs only the wait until the next periodic scan.
//!
//! **Never waited for.** Registering may take long (FSEvents asks a system
//! daemon, which can answer seconds later under load), and so may stopping:
//! both happen on a thread of their own, and the watch is woken once it is
//! registered — a scan then finds what changed meanwhile.
//!
//! Events of a file being *read* are not changes, and wake nothing (a scan
//! opening the directory would otherwise wake the next one, forever).

use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// A directory watched, or being registered, or that could not be.
pub struct Wake {
    signal: Arc<Signal>,
    state: Arc<Mutex<State>>,
}

pub(crate) enum State {
    Starting,
    /// Told by the system (`None`: by a [`FakeNotifier`](crate::FakeNotifier)).
    Watching(Option<RecommendedWatcher>),
    /// Nothing watched, and why.
    Stopped(String),
    /// The `Wake` is gone: a registration finishing now stops at once.
    Dropped,
}

#[derive(Default)]
pub(crate) struct Signal {
    changed: Mutex<bool>,
    woken: Condvar,
}

impl Signal {
    pub(crate) fn raise(&self) {
        *self.changed.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.woken.notify_all();
    }
}

impl Wake {
    /// Watches `dir` (and what is under it when `recursive`), registered on
    /// a thread of its own: returns at once.
    pub fn watch(dir: &Path, recursive: bool) -> Wake {
        let wake = Wake { signal: Arc::default(), state: Arc::new(Mutex::new(State::Starting)) };
        let (signal, state, dir) = (wake.signal.clone(), wake.state.clone(), dir.to_path_buf());
        let spawned = std::thread::Builder::new().name("grenat-watch".into()).spawn(move || {
            let registered = register(&dir, recursive, signal.clone());
            let mut current = state.lock().unwrap_or_else(PoisonError::into_inner);
            match (registered, &*current) {
                // gone meanwhile: the watcher is dropped here, on this thread
                (_, State::Dropped) => {}
                (Ok(watcher), _) => {
                    *current = State::Watching(Some(watcher));
                    drop(current);
                    // what changed while registering is the next scan's
                    signal.raise();
                }
                (Err(why), _) => *current = State::Stopped(why),
            }
        });
        if let Err(e) = spawned {
            *wake.state.lock().unwrap_or_else(PoisonError::into_inner) = State::Stopped(e.to_string());
        }
        wake
    }

    /// A watch in `state` already, woken by `signal`: what a
    /// [`FakeNotifier`](crate::FakeNotifier) hands.
    pub(crate) fn with(signal: Arc<Signal>, state: Arc<Mutex<State>>) -> Wake {
        Wake { signal, state }
    }

    /// Whether the operating system tells of changes now.
    pub fn is_watching(&self) -> bool {
        matches!(*self.state.lock().unwrap_or_else(PoisonError::into_inner), State::Watching(_))
    }

    /// Why the directory could not be watched (`None` while registering or watching).
    pub fn stopped(&self) -> Option<String> {
        match &*self.state.lock().unwrap_or_else(PoisonError::into_inner) {
            State::Stopped(why) => Some(why.clone()),
            _ => None,
        }
    }

    /// What waits for this watch's changes, from any thread.
    pub fn waiter(&self) -> Waiter {
        Waiter(self.signal.clone())
    }
}

impl Drop for Wake {
    fn drop(&mut self) {
        let previous =
            std::mem::replace(&mut *self.state.lock().unwrap_or_else(PoisonError::into_inner), State::Dropped);
        if let State::Watching(Some(watcher)) = previous {
            // stopping waits for the system too: on a thread of its own
            let _ = std::thread::Builder::new().name("grenat-unwatch".into()).spawn(move || drop(watcher));
        }
    }
}

fn register(dir: &Path, recursive: bool, signal: Arc<Signal>) -> Result<RecommendedWatcher, String> {
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if is_change(&event) {
            signal.raise();
        }
    })
    .map_err(|e| e.to_string())?;
    let mode = if recursive { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
    watcher.watch(dir, mode).map_err(|e| e.to_string())?;
    Ok(watcher)
}

/// Whether what the system told is a change: a file read is not; an
/// error (events lost, a queue overflowed) is, a scan finds what changed.
fn is_change(event: &notify::Result<notify::Event>) -> bool {
    !matches!(event, Ok(event) if matches!(event.kind, EventKind::Access(_)))
}

/// Waits for a [`Wake`]'s changes; sent to the thread that waits.
#[derive(Clone)]
pub struct Waiter(Arc<Signal>);

impl Waiter {
    /// Waits until a change is told, or `timeout`: whether one was.
    pub fn wait(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut changed = self.0.changed.lock().unwrap_or_else(PoisonError::into_inner);
        while !*changed {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            changed = self.0.woken.wait_timeout(changed, left).unwrap_or_else(PoisonError::into_inner).0;
        }
        *changed = false;
        true
    }
}

#[cfg(test)]
mod tests {
    use notify::event::{AccessKind, AccessMode, CreateKind, DataChange, ModifyKind, RenameMode};
    use notify::{Event, EventKind};

    use super::*;

    #[test]
    fn a_file_read_is_no_change_and_everything_else_is() {
        let event = |kind| Ok(Event::new(kind));
        for read in [AccessKind::Read, AccessKind::Open(AccessMode::Read), AccessKind::Close(AccessMode::Read)] {
            assert!(!is_change(&event(EventKind::Access(read))), "{read:?}");
        }
        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            EventKind::Modify(ModifyKind::Name(RenameMode::To)),
            EventKind::Remove(notify::event::RemoveKind::File),
            EventKind::Any,
        ] {
            assert!(is_change(&event(kind)), "{kind:?}");
        }
        assert!(is_change(&Err(notify::Error::generic("queue overflowed"))));
    }

    #[test]
    fn a_missing_directory_cannot_be_watched_and_waits_its_timeout() {
        let missing = std::env::temp_dir().join(format!("grenat-watch-missing-{}", std::process::id()));
        let wake = Wake::watch(&missing, false);
        let started = Instant::now();
        while wake.stopped().is_none() {
            assert!(started.elapsed() < Duration::from_secs(20), "never refused");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!wake.is_watching());
        let started = Instant::now();
        assert!(!wake.waiter().wait(Duration::from_millis(50)));
        assert!(started.elapsed() >= Duration::from_millis(50));
    }
}
