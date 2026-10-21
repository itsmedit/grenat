//! The operating system's notifications, for real (inotify on Linux,
//! FSEvents on macOS): a file created, written, renamed into the folder,
//! or created deep under it, wakes a waiting watch — long before its
//! timeout; a watch dropped stops without waiting.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use grenat_watch::{Waiter, Wake};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("grenat-watch-wake-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // resolved: FSEvents reports `/private/var/…` for `/var/…`
    std::fs::canonicalize(dir).unwrap()
}

/// Watches `dir`, once registered (FSEvents may take a while): its waiter,
/// past the wake-up registering gives.
fn watched(dir: &std::path::Path, recursive: bool) -> (Wake, Waiter) {
    let wake = Wake::watch(dir, recursive);
    let started = Instant::now();
    while !wake.is_watching() {
        assert!(wake.stopped().is_none(), "{:?}", wake.stopped());
        // FSEvents' daemon may take seconds per registration (it looks at the
        // test's executable, in a crowded `target/debug/deps`)
        assert!(started.elapsed() < Duration::from_secs(600), "never registered");
        std::thread::sleep(Duration::from_millis(10));
    }
    let waiter = wake.waiter();
    drain(&waiter);
    (wake, waiter)
}

/// Whether `change` wakes `wake` within 20 s (it is retried meanwhile:
/// FSEvents may start a moment after it is registered).
fn wakes(wake: &Waiter, mut change: impl FnMut(u32)) -> bool {
    let started = Instant::now();
    let mut n = 0;
    while started.elapsed() < Duration::from_secs(20) {
        change(n);
        n += 1;
        if wake.wait(Duration::from_millis(500)) {
            return true;
        }
    }
    false
}

/// Waits out the events already under way, so the next wait sees new ones only.
fn drain(wake: &Waiter) {
    while wake.wait(Duration::from_millis(300)) {}
}

#[test]
fn a_file_created_written_or_renamed_in_wakes_the_watch() {
    let dir = temp("flat");
    let outside = temp("flat-outside");
    let (_watch, wake) = watched(&dir, false);
    assert!(wakes(&wake, |n| std::fs::write(dir.join(format!("a{n}.pdf")), "x").unwrap()));
    drain(&wake);
    assert!(wakes(&wake, |n| std::fs::write(dir.join("a0.pdf"), format!("more {n}")).unwrap()));
    drain(&wake);
    assert!(wakes(&wake, |n| {
        let source = outside.join(format!("b{n}.pdf"));
        std::fs::write(&source, "x").unwrap();
        std::fs::rename(&source, dir.join(format!("b{n}.pdf"))).unwrap();
    }));
}

#[test]
fn reading_a_file_wakes_nothing() {
    let dir = temp("read");
    std::fs::write(dir.join("a.pdf"), "x").unwrap();
    let (_watch, wake) = watched(&dir, false);
    for _ in 0..5 {
        std::fs::read(dir.join("a.pdf")).unwrap();
        let _ = std::fs::read_dir(&dir).unwrap().count();
    }
    assert!(!wake.wait(Duration::from_millis(500)));
}

#[test]
fn a_recursive_watch_sees_deep_changes() {
    let dir = temp("deep");
    std::fs::create_dir_all(dir.join("2026/03")).unwrap();
    let (_watch, wake) = watched(&dir, true);
    assert!(wakes(&wake, |n| std::fs::write(dir.join(format!("2026/03/a{n}.pdf")), "x").unwrap()));
}

#[test]
fn a_watch_dropped_while_registering_or_watching_returns_at_once() {
    let dir = temp("dropped");
    let started = Instant::now();
    drop(Wake::watch(&dir, true));
    let (wake, _) = watched(&dir, true);
    let dropping = Instant::now();
    drop(wake);
    assert!(dropping.elapsed() < Duration::from_millis(100));
    assert!(started.elapsed() < Duration::from_secs(360));
}
