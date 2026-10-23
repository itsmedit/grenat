//! Files in (`on_file`), told of by the operating system for real
//! (inotify on Linux, FSEvents on macOS): the watches scan on a timer once
//! a minute only, so a file handed within seconds was told of — dropped in
//! the folder, or in another directory that replaced it under the same
//! name (inotify follows a directory, not its name). A target of its own:
//! registering may take long (FSEvents asks a system daemon), and the
//! other tests of `on_file` (`files.rs`) are told of changes by a test's
//! own notifier.

mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use grenat_interp::{Options, Output};

/// Serves `src` on a thread of its own (it never returns): what it prints and logs.
fn serve(src: &str) -> Arc<Mutex<String>> {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let program: &'static grenat_ast::Program = Box::leak(Box::new(parsed.program));
    let buffer = Arc::new(Mutex::new(String::new()));
    let options =
        Options { output: Output::Capture(buffer.clone()), journal: Some(temp_dir("journal")), ..Options::default() };
    std::thread::spawn(move || grenat_interp::serve(program, options, "127.0.0.1:0", |_| {}));
    buffer
}

/// Waits until the system told of changes `times` times (registered anew).
fn wait_told(output: &Arc<Mutex<String>>, times: usize) {
    let started = Instant::now();
    while output.lock().unwrap().matches(": told of changes by the system\n").count() < times {
        // FSEvents may answer late: its daemon looks at the test's executable
        assert!(started.elapsed() < Duration::from_secs(600), "never told:\n{}", output.lock().unwrap());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Waits (20 s at most) until `done` holds.
fn wait_until(what: &str, output: &Arc<Mutex<String>>, mut done: impl FnMut(&str) -> bool) {
    let started = Instant::now();
    while !done(&output.lock().unwrap()) {
        assert!(started.elapsed() < Duration::from_secs(20), "timed out waiting: {what}\n{}", output.lock().unwrap());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A folder to watch, `inbox`, in a directory of its own (resolved, as
/// the system reports it).
fn folder(name: &str) -> PathBuf {
    let dir = std::fs::canonicalize(temp_dir(name)).unwrap();
    std::fs::create_dir_all(dir.join("inbox")).unwrap();
    dir
}

/// Served, printing what it got, scanned on a timer once a minute only.
fn serve_told(inbox: &std::path::Path) -> Arc<Mutex<String>> {
    let output = serve(&format!(
        "on_file \"{}/*.pdf\", every: 60, settle: 0.2 do |path|\n  puts \"got #{{File.read(path)}}\"\nend\n",
        inbox.display()
    ));
    wait_told(&output, 1);
    output
}

#[test]
fn served_a_file_created_then_rewritten_is_told_of_by_the_system() {
    let inbox = folder("watched-created").join("inbox");
    let output = serve_told(&inbox);
    std::fs::write(inbox.join("a.pdf"), "first").unwrap();
    wait_until("the first content", &output, |out| out.contains("got first\n"));
    std::fs::write(inbox.join("a.pdf"), "second").unwrap();
    wait_until("the second content", &output, |out| out.contains("got second\n"));
}

#[cfg(unix)]
#[test]
fn served_a_folder_replaced_is_watched_again_by_the_system() {
    let dir = folder("watched-replaced");
    let inbox = dir.join("inbox");
    let output = serve_told(&inbox);
    // another directory under the same name, at once: no scan sees it missing
    std::fs::create_dir_all(dir.join("inbox.new")).unwrap();
    std::fs::rename(dir.join("inbox.new"), &inbox).unwrap();
    // the new directory watched (inotify stays on the one gone otherwise)
    wait_told(&output, 2);
    std::thread::sleep(Duration::from_millis(500));
    std::fs::write(inbox.join("a.pdf"), "new folder").unwrap();
    wait_until("told of", &output, |out| out.contains("got new folder\n"));
}
