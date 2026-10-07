//! `grenat serve` watching the folders of `on_file`, each on a task of its
//! own: the folder is scanned at the start, whenever the system tells of a
//! change (inotify on Linux, FSEvents on macOS), and every `every:` (the
//! fallback when it tells nothing: a network drive, a folder not there yet).
//!
//! A file is handed to the handler once it has stopped changing for
//! `settle:` — never half-written — and once per content: its SHA-256 is
//! taken and compared to that of the content handed over last for its path
//! (kept in the application's database when it has one, so a restart hands
//! nothing twice; in memory otherwise). A file gone is forgotten. The files
//! there at the start are handed over too, unless `existing: false`.
//!
//! A handler that raises is an event (`grenat console`) and a line on
//! standard error; the file is tried again after `every:`, [`MAX_ATTEMPTS`]
//! times in all, then given up until its content changes. A link leading
//! out of the folder, an unreadable folder or file: each reported once, an
//! event too. Nothing stops the server. A path is shown escaped (`{:?}`):
//! its name is the dropper's, and never writes a line of the log.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use grenat_watch::{MAX_ATTEMPTS, Tracker, Wake};

use crate::RuntimeError;
use crate::builtins::dropped_path;
use crate::eval::store::now;
use crate::prelude::*;

/// What a folder's task keeps between two scans.
struct Watch {
    tracker: Tracker,
    /// The system's notifications, once the folder was read.
    wake: Option<Wake>,
    /// The last error scanning the folder, recorded as an event once.
    last_error: Option<String>,
    /// The paths refused at the last scan, each reported once.
    refused: HashSet<PathBuf>,
    /// What the log said last of the system's notifications.
    told: Option<Told>,
}

#[derive(PartialEq)]
enum Told {
    Watching,
    /// Not told of changes, and why.
    Not(String),
}

impl<'p> Interp<'p> {
    /// Watches folder `i` until the process ends; a failure is reported,
    /// never fatal.
    pub(crate) fn watch_folder(&mut self, i: usize) {
        let (glob, every, settle, existing) = {
            let folders = self.folders.borrow();
            let folder = &folders[i];
            (folder.glob.clone(), folder.every(), folder.settle(), folder.existing)
        };
        let label = glob.pattern().to_string();
        let mut watch = Watch {
            tracker: Tracker::new(settle, every),
            wake: None,
            last_error: None,
            refused: HashSet::new(),
            told: None,
        };
        for (path, digest) in self.files_store(|db| grenat_ops::files::handled(db, &label)).unwrap_or_default() {
            watch.tracker.remember(PathBuf::from(path), digest);
        }
        let mut first = true;
        loop {
            match grenat_green::blocking(|| grenat_watch::scan(&glob)) {
                Ok(scan) => {
                    watch.last_error = None;
                    self.follow_wake(&label, &glob, &mut watch, every);
                    self.report_refused(&label, &mut watch, scan.refused);
                    if std::mem::take(&mut first) && !existing {
                        watch.tracker.skip_present(&scan.found, Instant::now());
                    }
                    let observed = watch.tracker.observe(&scan.found, Instant::now());
                    for gone in observed.gone {
                        let path = gone.to_string_lossy().into_owned();
                        self.files_store(|db| grenat_ops::files::forget(db, &label, &path));
                    }
                    for path in observed.ready {
                        self.hand_over(&label, i, &mut watch, &path);
                    }
                }
                Err(e) => {
                    let error = file_error("IoError", format!("cannot read {}: {e}", glob.base().display()));
                    self.write_err(&format!("[file] {label}: {}: {}\n", error.ty, error.message));
                    if watch.last_error.as_ref() != Some(&error.message) {
                        self.record_event("file", &label, &error);
                    }
                    watch.last_error = Some(error.message);
                    // the folder gone, its watch went with it
                    watch.wake = None;
                    watch.told = None;
                }
            }
            let wait = watch.tracker.next_check(Instant::now()).map_or(every, |next| next.min(every));
            match watch.wake.as_ref().map(Wake::waiter) {
                Some(waiter) => {
                    grenat_green::blocking(move || waiter.wait(wait));
                }
                None => grenat_green::sleep(wait),
            }
        }
    }

    /// Whether the system tells of the folder's changes, said once each
    /// time it changes: a registration that failed (the system's watch limit
    /// reached…) is tried again; the scans on a timer go on meanwhile.
    fn follow_wake(&mut self, label: &str, glob: &grenat_watch::Glob, watch: &mut Watch, every: Duration) {
        let told = match &watch.wake {
            None => None,
            Some(wake) if wake.is_watching() => Some(Told::Watching),
            Some(wake) => match wake.stopped() {
                Some(why) => Some(Told::Not(why)),
                // still registering
                None => return,
            },
        };
        if let Some(told) = told
            && watch.told.as_ref() != Some(&told)
        {
            match &told {
                Told::Watching => self.write_err(&format!("[file] {label}: told of changes by the system\n")),
                Told::Not(why) => self.write_err(&format!(
                    "[file] {label}: not told of changes ({why}): scanned every {}s\n",
                    every.as_secs_f64()
                )),
            }
            watch.told = Some(told);
        }
        if !watch.wake.as_ref().is_some_and(Wake::is_watching) {
            watch.wake = Some(Wake::watch(glob.base(), glob.depth() != Some(1)));
        }
    }

    /// A settled file: its content read, and handed over if it is new.
    fn hand_over(&mut self, label: &str, i: usize, watch: &mut Watch, path: &Path) {
        let shown = path.to_string_lossy().into_owned();
        let subject = format!("{label} {shown:?}");
        let digest = match grenat_green::blocking(|| grenat_watch::digest(path)) {
            Ok(digest) => digest,
            Err(e) => {
                let error = file_error("IoError", format!("cannot read {shown:?}: {e}"));
                self.failed_run(label, watch, path, &subject, &error, "");
                return;
            }
        };
        if !watch.tracker.is_new(path, &digest) {
            return;
        }
        self.write_err(&format!("[file] {label}: {shown:?}\n"));
        let block = self.folders.borrow()[i].block.clone();
        match self.call_block(&block, vec![dropped_path(&shown)]) {
            Ok(_) => self.handled(label, watch, path, &digest),
            Err(ctrl) => {
                let error = self.runtime_error(ctrl);
                self.failed_run(label, watch, path, &subject, &error, &digest);
            }
        }
    }

    /// A run that failed: reported, then tried again, or given up.
    fn failed_run(
        &mut self,
        label: &str,
        watch: &mut Watch,
        path: &Path,
        subject: &str,
        error: &RuntimeError,
        digest: &str,
    ) {
        self.write_err(&format!("[file] {subject}: {}: {}\n", error.ty, error.message));
        self.record_event("file", subject, error);
        if watch.tracker.failed(path, Instant::now()) {
            let error = file_error("FileError", format!("given up after {MAX_ATTEMPTS} attempts, until it changes"));
            self.write_err(&format!("[file] {subject}: {}\n", error.message));
            self.record_event("file", subject, &error);
            self.handled(label, watch, path, digest);
        }
    }

    /// `path`'s content counts as handled, in memory and in the database.
    fn handled(&mut self, label: &str, watch: &mut Watch, path: &Path, digest: &str) {
        watch.tracker.handled(path, digest);
        let path = path.to_string_lossy().into_owned();
        self.files_store(|db| grenat_ops::files::record(db, label, &path, digest, now()));
    }

    /// The links leading out, the names not UTF-8: reported once each.
    fn report_refused(&mut self, label: &str, watch: &mut Watch, refused: Vec<grenat_watch::Refused>) {
        let mut now_refused = HashSet::new();
        for refusal in refused {
            if !watch.refused.contains(&refusal.path) {
                let shown = refusal.path.to_string_lossy().into_owned();
                let subject = format!("{label} {shown:?}");
                let error = file_error("FileError", format!("refused: {}", refusal.why));
                self.write_err(&format!("[file] {subject}: {}\n", error.message));
                self.record_event("file", &subject, &error);
            }
            now_refused.insert(refusal.path);
        }
        watch.refused = now_refused;
    }

    /// Runs `f` on the application's database, if it has one: the files
    /// handled are kept there. A failure is a warning, never fatal.
    fn files_store<T: Send>(
        &mut self,
        f: impl FnOnce(&mut dyn grenat_db::Connection) -> Result<T, String> + Send,
    ) -> Option<T> {
        let database = self.app_db.borrow().clone()?;
        let connection = self.connection_of(&database)?;
        match grenat_green::blocking(|| f(&mut **connection.lock())) {
            Ok(value) => Some(value),
            Err(e) => {
                self.write_err(&format!("warning: the files handled are not recorded: {e}\n"));
                None
            }
        }
    }
}

fn file_error(ty: &str, message: String) -> RuntimeError {
    RuntimeError { ty: ty.into(), message, span: None, trace: Vec::new() }
}
