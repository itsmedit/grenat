//! Files in (`on_file`): the declaration's checks, `deliver_file` in tests
//! (outcomes, the path untrusted), and `grenat serve` watching real folders
//! — a file created, rewritten, renamed in, written slowly, there at the
//! start, matched or not by its pattern, a link leading out, a raising
//! handler retried then given up while the server goes on, and what was
//! handled kept across a restart. The system's notifications are a test's
//! own here ([`FakeNotifier`]): the watches scan on a timer once a minute,
//! so a file handed within seconds was told of; `files_watched.rs` proves
//! the system's own (inotify on Linux, FSEvents on macOS).

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use grenat_interp::{Options, Output};
use grenat_watch::FakeNotifier;

// ── In tests: deliver_file ─────────────────────────────────────────────

#[test]
fn deliver_file_hands_the_handler_an_untrusted_path() {
    let output = tests_pass(
        "on_file \"inbox/*.pdf\" do |path|
  assert path.tainted?
  puts \"got #{path.trust!}\"
  Shell.run([\"echo\", path])
end
on_file \"inbox/**/*\" do |path|
  puts \"all #{path.trust!}\"
end
test \"a name is the dropper's: it reaches no command unchecked\" do
  r = deliver_file(\"inbox/; rm -rf ~.pdf\")
  assert_equal \"failed\", r[\"status\"]
  assert r[\"error\"].start_with?(\"TaintError\")
end
test \"every matching handler\" do
  assert_equal({\"status\" => \"handled\"}, deliver_file(\"inbox/2026/a.txt\"))
end
",
    );
    assert!(output.contains("got inbox/; rm -rf ~.pdf\n") && output.contains("all inbox/; rm -rf ~.pdf\n"), "{output}");
    assert!(output.contains("all inbox/2026/a.txt\n") && !output.contains("got inbox/2026"), "{output}");
}

#[test]
fn a_handler_reads_the_file_it_is_given() {
    let dir = temp_dir("deliver-read");
    std::fs::write(dir.join("a.csv"), "a,b\n1,2\n").unwrap();
    let d = dir.display();
    tests_pass(&format!(
        "on_file \"{d}/*.csv\" do |path|
  raise ArgumentError, \"empty\" if File.lines(path).size < 2
end
test \"read\" do
  assert_equal \"handled\", deliver_file(\"{d}/a.csv\")[\"status\"]
  r = deliver_file(\"{d}/missing.csv\")
  assert_equal \"failed\", r[\"status\"]
  assert r[\"error\"].start_with?(\"IoError\")
end
"
    ));
}

#[cfg(unix)]
#[test]
fn a_read_under_a_watched_folder_never_follows_a_link_leading_out() {
    let dir = std::fs::canonicalize(temp_dir("deliver-links")).unwrap();
    std::fs::create_dir_all(dir.join("inbox/sub")).unwrap();
    std::fs::write(dir.join("secret.pdf"), "secret").unwrap();
    std::fs::write(dir.join("inbox/a.pdf"), "mine").unwrap();
    std::os::unix::fs::symlink(dir.join("secret.pdf"), dir.join("inbox/escape.pdf")).unwrap();
    std::os::unix::fs::symlink(dir.join("inbox/a.pdf"), dir.join("inbox/alias.pdf")).unwrap();
    std::os::unix::fs::symlink(&dir, dir.join("inbox/up")).unwrap();
    let d = dir.display();
    tests_pass(&format!(
        "on_file \"{d}/inbox/**/*.pdf\" do |path|
  File.read(path)
end
test \"inside\" do
  assert_equal \"mine\", File.read(\"{d}/inbox/a.pdf\")
  assert_equal \"mine\", File.read(\"{d}/inbox/alias.pdf\")
  assert_equal \"handled\", deliver_file(\"{d}/inbox/alias.pdf\")[\"status\"]
end
test \"leading out, swapped after the scan\" do
  r = deliver_file(\"{d}/inbox/escape.pdf\")
  assert_equal \"failed\", r[\"status\"]
  assert r[\"error\"].include?(\"refused: a link leading out of the watched directory\")
  assert_raises(IoError) {{ File.lines(\"{d}/inbox/escape.pdf\") }}
  assert_raises(IoError) {{ Pdf.read(\"{d}/inbox/up/secret.pdf\") }}
  assert_raises(IoError) {{ File.read(\"{d}/inbox/sub/../up/secret.pdf\") }}
end
test \"outside the folder: read as it is\" do
  assert_equal \"secret\", File.read(\"{d}/secret.pdf\")
end
"
    ));
}

#[test]
fn deliver_file_is_a_test_double_with_checked_arguments() {
    let e = run_err("on_file \"inbox/*\" do |p|\nend\ndeliver_file(\"inbox/a\")\n", Vec::new());
    assert_eq!(e.ty, "RuntimeError");
    assert!(e.message.contains("only works in a test"), "{}", e.message);
    let (results, _) = test_outcomes(
        "on_file \"inbox/*.pdf\" do |p|\nend
test \"no match\" do\n  deliver_file(\"outbox/a.pdf\")\nend
test \"a secret\" do\n  deliver_file(Credentials.fetch(:a, :b))\nend
test \"not a path\" do\n  deliver_file(3)\nend
test \"two paths\" do\n  deliver_file(\"inbox/a.pdf\", \"inbox/b.pdf\")\nend
",
    );
    let error = |i: usize| results[i].1.clone().unwrap_or_default();
    assert!(error(0).contains("no `on_file` pattern matches \"outbox/a.pdf\""), "{}", error(0));
    assert!(error(1).starts_with("SecretError"), "{}", error(1));
    assert!(error(2).starts_with("TypeError"), "{}", error(2));
    assert!(error(3).starts_with("ArgumentError"), "{}", error(3));
    let (results, _) = test_outcomes("test \"none\" do\n  deliver_file(\"a\")\nend\n");
    assert!(results[0].1.as_ref().unwrap().contains("no folder is watched"));
}

#[test]
fn the_declaration_checks_its_pattern_its_folder_and_its_options() {
    // the folder is held to the capabilities of the function declaring it
    let e = run_err(
        "def watch uses fs.read(\"outbox\")\n  on_file \"inbox/*.pdf\" do |p|\n  end\nend\nwatch\n",
        Vec::new(),
    );
    assert_eq!(e.ty, "CapabilityError");
    assert!(e.message.contains("`fs.read` on `inbox`"), "{}", e.message);
    run("def watch uses fs.read(\"inbox\")\n  on_file \"inbox/**/*.pdf\" do |p|\n  end\nend\nwatch\n");
    for (declaration, error) in [
        ("on_file \"inbox/\"", "ArgumentError"),
        ("on_file \"inbox/[ab\"", "ArgumentError"),
        ("on_file 3", "TypeError"),
        ("on_file \"inbox/*\", every: 0", "ArgumentError"),
        ("on_file \"inbox/*\", settle: -1", "ArgumentError"),
        ("on_file \"inbox/*\", existing: \"yes\"", "ArgumentError"),
        ("on_file \"inbox/*\", recursive: true", "ArgumentError"),
    ] {
        let e = run_err(&format!("{declaration} do |p|\nend\n"), Vec::new());
        assert_eq!(e.ty, error, "{declaration}: {}", e.message);
    }
    run("on_file \"inbox/*\", every: 30.s, settle: 0, existing: false do |p|\nend\n");
    let src = format!("{SUMMARY}s = summarize(\"x\")\non_file s.title do |p|\nend\n");
    assert_eq!(run_err(&src, vec![summary_reply()]).ty, "TaintError");
}

// ── Served, on real folders ───────────────────────────────────────────

/// Serves `src` on a thread of its own (it never returns), told of changes
/// by `notifier`: what it prints and logs.
fn serve_with(src: &str, notifier: &FakeNotifier) -> Arc<Mutex<String>> {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let program: &'static grenat_ast::Program = Box::leak(Box::new(parsed.program));
    let buffer = Arc::new(Mutex::new(String::new()));
    let options = Options {
        output: Output::Capture(buffer.clone()),
        journal: Some(temp_dir("journal")),
        notifier: Some(notifier.clone()),
        ..Options::default()
    };
    std::thread::spawn(move || grenat_interp::serve(program, options, "127.0.0.1:0", |_| {}));
    buffer
}

/// Serves `src`, told of no change: its scans on a timer find them.
fn serve(src: &str) -> Arc<Mutex<String>> {
    serve_with(src, &FakeNotifier::default())
}

/// Serves `src`, once told of the changes of its one folder: what it
/// prints, and what tells it.
fn serve_told(src: &str) -> (Arc<Mutex<String>>, FakeNotifier) {
    let notifier = FakeNotifier::default();
    let output = serve_with(src, &notifier);
    wait_until("told of changes", &output, |out| out.contains(": told of changes by the system\n"));
    (output, notifier)
}

/// Writes `content` to `path`, and tells of it.
fn write_told(notifier: &FakeNotifier, path: &Path, content: &str) {
    std::fs::write(path, content).unwrap();
    assert_eq!(notifier.change(path), 1, "{} is not watched", path.display());
}

/// Waits (20 s at most) until `done` holds.
fn wait_until(what: &str, output: &Arc<Mutex<String>>, mut done: impl FnMut(&str) -> bool) {
    let started = Instant::now();
    while !done(&output.lock().unwrap()) {
        assert!(started.elapsed() < Duration::from_secs(20), "timed out waiting: {what}\n{}", output.lock().unwrap());
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn count(output: &Arc<Mutex<String>>, line: &str) -> usize {
    output.lock().unwrap().matches(line).count()
}

/// A folder to watch, `inbox`, in a directory of its own (resolved, as
/// the system reports it).
fn folder(name: &str) -> PathBuf {
    let dir = std::fs::canonicalize(temp_dir(name)).unwrap();
    std::fs::create_dir_all(dir.join("inbox")).unwrap();
    dir
}

/// A handler that prints what it got: the path, then the content.
fn printing(pattern: &str, options: &str) -> String {
    format!("on_file \"{pattern}\"{options} do |path|\n  puts \"got #{{path.trust!}}: #{{File.read(path)}}\"\nend\n")
}

/// Scans on a timer once a minute only: what is handed sooner was told of.
const TOLD: &str = ", every: 60, settle: 0.2";

/// Scans often: what the system tells is not waited for.
const OFTEN: &str = ", every: 0.2, settle: 0.2";

#[test]
fn served_a_file_created_then_rewritten_is_told_of_and_handed_once_per_content() {
    let dir = folder("files-created");
    let inbox = dir.join("inbox");
    let (output, notifier) = serve_told(&printing(&format!("{}/*.pdf", inbox.display()), TOLD));
    write_told(&notifier, &inbox.join("a.pdf"), "first");
    write_told(&notifier, &inbox.join("a.txt"), "not a pdf");
    let got = format!("got {}/a.pdf: first\n", inbox.display());
    wait_until("the first content", &output, |out| out.contains(&got));
    // touched, the same content: nothing
    write_told(&notifier, &inbox.join("a.pdf"), "first");
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(count(&output, &got), 1);
    // new content: handed
    write_told(&notifier, &inbox.join("a.pdf"), "second");
    let second = format!("got {}/a.pdf: second\n", inbox.display());
    wait_until("the second content", &output, |out| out.contains(&second));
    std::thread::sleep(Duration::from_millis(500));
    let out = output.lock().unwrap().clone();
    assert_eq!(out.matches(&second).count(), 1, "{out}");
    assert!(!out.contains("a.txt"), "{out}");
    assert!(out.contains(&format!("[file] {}/*.pdf: \"{}/a.pdf\"\n", inbox.display(), inbox.display())), "{out}");
}

#[test]
fn served_a_file_renamed_into_the_folder_or_to_a_matching_name() {
    let dir = folder("files-renamed");
    let inbox = dir.join("inbox");
    let output = serve(&printing(&format!("{}/*.pdf", inbox.display()), OFTEN));
    // moved in from elsewhere
    std::fs::write(dir.join("b.pdf"), "moved").unwrap();
    std::fs::rename(dir.join("b.pdf"), inbox.join("b.pdf")).unwrap();
    wait_until("the one moved in", &output, |out| out.contains("/inbox/b.pdf: moved\n"));
    // written under a temporary name, then renamed: as browsers and sync clients do
    std::fs::write(inbox.join("c.pdf.part"), "renamed").unwrap();
    std::thread::sleep(Duration::from_millis(400));
    assert!(!output.lock().unwrap().contains("c.pdf"));
    std::fs::rename(inbox.join("c.pdf.part"), inbox.join("c.pdf")).unwrap();
    wait_until("the one renamed", &output, |out| out.contains("/inbox/c.pdf: renamed\n"));
}

#[test]
fn served_a_file_still_being_written_waits_until_it_settles() {
    let dir = folder("files-slow");
    let inbox = dir.join("inbox");
    let output = serve(&printing(&format!("{}/*.csv", inbox.display()), ", every: 0.2, settle: 0.6"));
    {
        use std::io::Write;
        let mut file = std::fs::File::create(inbox.join("big.csv")).unwrap();
        for n in 0..8 {
            write!(file, "{n}").unwrap();
            file.flush().unwrap();
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    wait_until("the whole file", &output, |out| out.contains("/inbox/big.csv: 01234567\n"));
    std::thread::sleep(Duration::from_millis(800));
    let out = output.lock().unwrap().clone();
    assert_eq!(out.matches("big.csv").count(), 2, "never handed half-written:\n{out}");
}

#[test]
fn served_files_there_at_the_start_and_patterns() {
    let dir = folder("files-start");
    let inbox = dir.join("inbox");
    std::fs::create_dir_all(inbox.join("2026/03")).unwrap();
    std::fs::create_dir_all(inbox.join(".cache")).unwrap();
    for (path, content) in
        [("old.pdf", "old"), ("2026/03/deep.PDF", "deep"), ("2026/x.txt", "x"), (".cache/h.pdf", "h"), (".h.pdf", "h")]
    {
        std::fs::write(inbox.join(path), content).unwrap();
    }
    let i = inbox.display();
    let all = printing(&format!("{i}/**/*.{{pdf,PDF}}"), OFTEN);
    let src = format!(
        "{all}on_file \"{i}/*.pdf\", every: 0.2, settle: 0.2, existing: false do |path|\n  puts \"new #{{path.trust!}}\"\nend\n"
    );
    let output = serve(&src);
    wait_until("the files there at the start", &output, |out| {
        out.contains(&format!("got {i}/old.pdf: old\n")) && out.contains(&format!("got {i}/2026/03/deep.PDF: deep\n"))
    });
    std::thread::sleep(Duration::from_millis(500));
    let out = output.lock().unwrap().clone();
    assert!(!out.contains("x.txt") && !out.contains("h.pdf") && !out.contains("new "), "{out}");
    // `existing: false`: what changes afterwards only
    std::fs::write(inbox.join("old.pdf"), "changed").unwrap();
    wait_until("the changed one", &output, |out| out.contains(&format!("new {i}/old.pdf\n")));
    assert_eq!(count(&output, &format!("new {i}/")), 1);
}

#[cfg(unix)]
#[test]
fn served_a_link_leading_out_is_refused() {
    let dir = folder("files-links");
    let inbox = dir.join("inbox");
    std::fs::write(dir.join("secret.pdf"), "secret").unwrap();
    std::fs::write(inbox.join("real.pdf"), "real").unwrap();
    std::os::unix::fs::symlink(dir.join("secret.pdf"), inbox.join("escape.pdf")).unwrap();
    std::os::unix::fs::symlink(inbox.join("real.pdf"), inbox.join("alias.pdf")).unwrap();
    let (db_path, database) = events_database();
    let output = serve(&format!("{database}{}", printing(&format!("{}/*.pdf", inbox.display()), OFTEN)));
    wait_until("the two inside", &output, |out| out.contains("alias.pdf: real\n") && out.contains("real.pdf: real\n"));
    std::thread::sleep(Duration::from_millis(500));
    let out = output.lock().unwrap().clone();
    assert!(!out.contains(": secret"), "{out}");
    assert_eq!(out.matches("escape.pdf\": refused: a link leading out of the watched directory\n").count(), 1, "{out}");
    let events = file_events(&db_path);
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(events[0].0.ends_with("/inbox/escape.pdf\"") && events[0].1 == "FileError", "{events:?}");
}

#[cfg(unix)]
#[test]
fn served_a_file_swapped_for_a_link_while_handled_is_never_read_through_it() {
    let dir = folder("files-swapped");
    let inbox = dir.join("inbox");
    std::fs::write(dir.join("secret.pdf"), "secret").unwrap();
    std::fs::write(inbox.join("a.pdf"), "mine").unwrap();
    let i = inbox.display();
    let src = format!(
        "on_file \"{i}/*.pdf\", every: 0.2, settle: 0.1 do |path|
  puts \"start\"
  sleep 0.8
  puts \"got #{{File.read(path)}}\"
end
"
    );
    let output = serve(&src);
    wait_until("the handler started", &output, |out| out.contains("start\n"));
    // scanned as a file, swapped for a link leading out before it is read
    std::fs::remove_file(inbox.join("a.pdf")).unwrap();
    std::os::unix::fs::symlink(dir.join("secret.pdf"), inbox.join("a.pdf")).unwrap();
    wait_until("the read refused", &output, |out| out.contains("refused: a link leading out"));
    std::thread::sleep(Duration::from_millis(1000));
    let out = output.lock().unwrap().clone();
    assert!(!out.contains("got secret"), "{out}");
}

#[cfg(unix)]
#[test]
fn served_a_path_flipping_between_a_file_and_a_link_leading_out_is_reported_once() {
    let dir = folder("files-flipping");
    let inbox = dir.join("inbox");
    std::fs::write(dir.join("secret.pdf"), "secret").unwrap();
    let output = serve(&printing(&format!("{}/*.pdf", inbox.display()), ", every: 0.1, settle: 0"));
    // each swap a rename over the path: never gone in between
    let (a, swap) = (inbox.join("a.pdf"), inbox.join("swap"));
    for n in 0..4 {
        std::os::unix::fs::symlink(dir.join("secret.pdf"), &swap).unwrap();
        std::fs::rename(&swap, &a).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        std::fs::write(&swap, format!("mine {n}")).unwrap();
        std::fs::rename(&swap, &a).unwrap();
        std::thread::sleep(Duration::from_millis(400));
    }
    let out = output.lock().unwrap().clone();
    assert!(!out.contains(": secret"), "{out}");
    assert!(out.contains("a.pdf: mine"), "{out}");
    assert_eq!(out.matches("refused: a link leading out").count(), 1, "{out}");
}

#[test]
fn served_a_raising_handler_is_retried_then_given_up_and_the_server_goes_on() {
    let dir = folder("files-raising");
    let inbox = dir.join("inbox");
    std::fs::write(inbox.join("poison.pdf"), "poison").unwrap();
    let (db_path, database) = events_database();
    let i = inbox.display();
    let src = format!(
        "{database}on_file \"{i}/*.pdf\", every: 0.3, settle: 0.1 do |path|
  raise ArgumentError, \"cannot read it\" if File.read(path) == \"poison\"
  puts \"handled #{{path.trust!}}\"
end
"
    );
    let output = serve(&src);
    wait_until("given up", &output, |out| out.contains("given up after 3 attempts"));
    std::fs::write(inbox.join("fine.pdf"), "fine").unwrap();
    wait_until("the next one", &output, |out| out.contains(&format!("handled {i}/fine.pdf\n")));
    std::thread::sleep(Duration::from_millis(800));
    let out = output.lock().unwrap().clone();
    let failed = format!("[file] {i}/*.pdf \"{i}/poison.pdf\": ArgumentError: cannot read it\n");
    assert_eq!(out.matches(&failed).count(), 3, "{out}");
    let events = file_events(&db_path);
    assert_eq!(events.iter().filter(|e| e.1 == "ArgumentError").count(), 3, "{events:?}");
    assert_eq!(events.iter().filter(|e| e.1 == "FileError").count(), 1, "{events:?}");
    assert!(events.iter().all(|e| e.0 == format!("{i}/*.pdf \"{i}/poison.pdf\"")), "{events:?}");
    // changed: tried again
    std::fs::write(inbox.join("poison.pdf"), "cured").unwrap();
    wait_until("the cured one", &output, |out| out.contains(&format!("handled {i}/poison.pdf\n")));
}

#[test]
fn served_what_was_handled_is_not_handed_again_after_a_restart() {
    let dir = folder("files-restart");
    let inbox = dir.join("inbox");
    std::fs::write(inbox.join("a.pdf"), "a").unwrap();
    let (_, database) = events_database();
    let src = format!("{database}{}", printing(&format!("{}/*.pdf", inbox.display()), OFTEN));
    let first = serve(&src);
    wait_until("handled", &first, |out| out.contains("a.pdf: a\n"));
    // a new server on the same database: the file is known
    std::fs::write(inbox.join("b.pdf"), "b").unwrap();
    let second = serve(&src);
    wait_until("the new one", &second, |out| out.contains("b.pdf: b\n"));
    std::thread::sleep(Duration::from_millis(500));
    assert!(!second.lock().unwrap().contains("a.pdf"), "{}", second.lock().unwrap());
}

#[cfg(unix)]
#[test]
fn served_a_folder_replaced_between_two_scans_is_watched_again() {
    let dir = folder("files-replaced");
    let inbox = dir.join("inbox");
    let (output, notifier) = serve_told(&printing(&format!("{}/*.pdf", inbox.display()), TOLD));
    // another directory under the same name, at once: no scan sees it missing
    std::fs::create_dir_all(dir.join("inbox.new")).unwrap();
    std::fs::rename(dir.join("inbox.new"), &inbox).unwrap();
    assert_eq!(notifier.change(&inbox), 1);
    // the new directory watched (inotify stays on the one gone otherwise)
    wait_until("watched anew", &output, |out| out.matches(": told of changes by the system\n").count() == 2);
    assert_eq!(notifier.registrations(), 2);
    write_told(&notifier, &inbox.join("a.pdf"), "new folder");
    // scanned once a minute only: handed sooner, it was told of
    wait_until("told of", &output, |out| out.contains("a.pdf: new folder\n"));
}

#[test]
fn served_a_missing_folder_is_reported_then_watched_once_there() {
    let dir = std::fs::canonicalize(temp_dir("files-missing")).unwrap();
    let inbox = dir.join("inbox");
    let (db_path, database) = events_database();
    let src = format!("{database}{}", printing(&format!("{}/*.pdf", inbox.display()), ", every: 0.3, settle: 0.1"));
    let output = serve(&src);
    wait_until("reported", &output, |out| out.contains("IoError: cannot read"));
    // scanned again and again: reported once
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(count(&output, "IoError: cannot read"), 1, "{}", output.lock().unwrap());
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::write(inbox.join("a.pdf"), "late").unwrap();
    wait_until("handled", &output, |out| out.contains("a.pdf: late\n"));
    // the same error: one event
    assert_eq!(file_events(&db_path).len(), 1);
}

/// A served program's SQLite database (where events go): its path, and
/// the line declaring it.
fn events_database() -> (PathBuf, String) {
    let path = temp_dir("file-events").join("app.db");
    let url = format!("sqlite://{}", path.display());
    (path, format!("database \"{url}\"\n"))
}

/// The `file` events recorded: their subject, error type and message.
fn file_events(path: &Path) -> Vec<(String, String, String)> {
    let mut connection = grenat_db::connect(&format!("sqlite://{}", path.display())).unwrap();
    let events = grenat_ops::events::latest(connection.as_mut(), false, 100).unwrap();
    events.into_iter().filter(|e| e.source == "file").map(|e| (e.subject, e.error, e.message)).collect()
}
