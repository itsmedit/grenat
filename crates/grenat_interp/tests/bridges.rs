//! `native def` served by a bridge: Grenat code calling the Ruby functions
//! of the fixture `texts` and the Python functions of `numbers`, each run
//! by its server process — types, taint, effects, secrets, errors, deaths,
//! timeouts and the log.
//!
//! A test whose interpreter is not installed says so and returns.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use grenat_bridge::fixture::{self, PYTHON, RUBY};
use grenat_interp::{Options, Output, RuntimeError, run_main};

struct Run {
    result: Result<(), RuntimeError>,
    output: String,
}

/// Runs `code` after the declarations of `facet`, with its bridge.
fn run_with(facet: &str, code: &str, edit: impl FnOnce(&mut grenat_bridge::Installed), log: bool) -> Option<Run> {
    let (_, installed) = fixture::installed(facet)?;
    let mut installed = installed.clone();
    edit(&mut installed);
    let src = format!("{}\n{code}", fixture::declarations(facet)?);
    let parsed = grenat_parser::parse(&src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}\n{src}", parsed.diagnostics);
    let out = Arc::new(Mutex::new(String::new()));
    let options = Options { output: Output::Capture(out.clone()), bridges: vec![installed], log, ..Options::default() };
    let result = run_main(&parsed.program, Vec::new(), options).map(drop);
    let output = out.lock().unwrap().clone();
    Some(Run { result, output })
}

fn run(facet: &str, code: &str) -> Option<Run> {
    run_with(facet, code, |_| {}, false)
}

fn ok(facet: &str, code: &str) -> Option<String> {
    let run = run(facet, code)?;
    match run.result {
        Ok(()) => Some(run.output),
        Err(e) => panic!("{e:?}\noutput:\n{}", run.output),
    }
}

fn error(facet: &str, code: &str) -> Option<RuntimeError> {
    let run = run(facet, code)?;
    match run.result {
        Err(e) => Some(e),
        Ok(()) => panic!("expected an error, got:\n{}", run.output),
    }
}

#[test]
fn ruby_functions_are_called_with_their_types() {
    let Some(out) = ok(
        RUBY,
        "\
puts add(40, 2)
words = words(\"a bb\")
puts words.size, words[1].text, words[1].position
p first_word(\"\"), first_word(\"x y\")
counts = count(\"a b a\")
puts counts[\"a\"], counts[\"b\"]
puts shout(\"hi\").trust!
",
    ) else {
        return;
    };
    assert_eq!(out, "42\n2\nbb\n1\nnil\n\"x\"\n2\n1\nHI\n");
}

#[test]
fn python_functions_are_called_with_their_types() {
    let Some(out) = ok(
        PYTHON,
        "\
puts mean([1.0, 2.0]), add(40, 2)
p mean([])
h = histogram([1, 2, 1])
puts h[\"1\"], h[\"2\"]
m = middle(Point.new(x: 0.0, y: 0.0), Point.new(x: 2.0, y: 4.0))
puts m.x, m.y
",
    ) else {
        return;
    };
    assert_eq!(out, "1.5\n42\nnil\n2\n1\n1.0\n2.0\n");
}

#[test]
fn a_bridge_result_is_untrusted_unless_pure() {
    let Some(out) = ok(
        RUBY,
        "\
puts shout(\"hi\").tainted?, add(1, 2).tainted?, words(\"a\").first.text.tainted?
puts count(shout(\"a a\")).tainted?
",
    ) else {
        return;
    };
    assert_eq!(out, "true\nfalse\nfalse\ntrue\n");
    // an untrusted value cannot reach a function with a dangerous effect
    let tool = "tool send(body: String) -> String uses net\n  body\nend\n";
    let e = error(RUBY, &format!("{tool}send(shout(\"x\"))\n")).unwrap();
    assert_eq!(e.ty, "TaintError");
    assert_eq!(ok(RUBY, &format!("{tool}puts send(shout(\"x\").trust!)\n")).unwrap(), "X\n");
}

#[test]
fn the_effects_of_a_bridge_function_are_enforced_on_its_callers() {
    let dir = std::env::temp_dir().join(format!("grenat-bridges-text-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("text.txt");
    std::fs::write(&file, "some text").unwrap();
    let path = file.display();
    let code = format!(
        "def load(path: String) -> String uses fs.read\n  read_text(path).trust!\nend\nputs load(\"{path}\")\n"
    );
    let Some(out) = ok(RUBY, &code) else { return };
    assert_eq!(out, "some text\n");
    let e = error(RUBY, &format!("def load uses net\n  read_text(\"{path}\")\nend\nload\n")).unwrap();
    assert_eq!(
        (e.ty.as_str(), e.message.as_str()),
        ("CapabilityError", "`fs.read` is not allowed by `load` (uses net)")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn errors_are_grenat_errors() {
    if let Some(e) = error(RUBY, "refuse(\"not today\")\n") {
        assert_eq!((e.ty.as_str(), e.message.as_str()), ("Refused", "not today"));
        assert_eq!(e.trace[0].0, "refuse");
        assert_eq!(error(RUBY, "read_text(\"/nowhere/at/all\")\n").unwrap().ty, "TextError");
        let out = ok(
            RUBY,
            "\
begin
  refuse(\"no\")
rescue Refused => e
  puts \"refused: #{e.message}\"
end
puts add(1, 1)
",
        );
        assert_eq!(out.unwrap(), "refused: no\n2\n");
    }
    if let Some(e) = error(PYTHON, "ratio(1.0, 0.0)\n") {
        assert_eq!(e.ty, "MathError");
        assert!(e.message.contains("division by zero"), "{}", e.message);
        assert_eq!(error(PYTHON, "odd(2)\n").unwrap().ty, "NotOdd");
    }
}

#[test]
fn a_server_that_dies_raises_a_bridge_error_and_is_started_again() {
    let Some(out) = ok(
        PYTHON,
        "\
first = pid.trust!
begin
  crash(5)
rescue BridgeError => e
  puts e.message.start_with?(\"the process of facet `numbers` exited with status 5\")
end
puts pid.trust! != first, add(2, 2)
",
    ) else {
        return;
    };
    assert_eq!(out, "true\ntrue\n4\n");
}

#[test]
fn a_call_that_runs_too_long_raises_a_bridge_error() {
    let quick = |installed: &mut grenat_bridge::Installed| installed.spec.timeout = Duration::from_millis(300);
    let Some(run) = run_with(RUBY, "nap(30.0)\n", quick, false) else { return };
    let e = run.result.unwrap_err();
    assert_eq!(
        (e.ty.as_str(), e.message.as_str()),
        ("BridgeError", "`nap` still running after 300ms: the process of facet `texts` was killed")
    );
}

#[test]
fn no_secret_is_handed_to_a_bridge() {
    let code = "mock_credentials({\"api\" => {\"key\" => \"s3cr3t\"}})\nshout(Credentials.fetch(:api, :key))\n";
    let Some(e) = error(RUBY, code) else { return };
    assert_eq!(e.ty, "SecretError");
    assert!(e.message.starts_with("a secret is never handed to bridge code: `shout`"), "{}", e.message);
    assert!(!e.message.contains("s3cr3t"));
}

#[test]
fn the_log_shows_calls_and_what_servers_write_on_standard_error() {
    let Some(run) = run_with(RUBY, "puts chatter(\"x\").trust!\n", |_| {}, true) else { return };
    run.result.unwrap();
    for line in ["x\n", "[bridge] texts: chatter\n", "[bridge] texts: printed x\n", "[bridge] texts: warned x\n"] {
        assert!(run.output.contains(line), "{line:?} not in:\n{}", run.output);
    }
    // without `--log`, standard error goes nowhere
    let run = run_with(RUBY, "puts chatter(\"x\").trust!\n", |_| {}, false).unwrap();
    assert_eq!(run.output, "x\n");
}

#[test]
fn a_facet_wraps_its_bridge_functions_in_grenat() {
    let Some((dir, _)) = fixture::installed(RUBY) else { return };
    let lib = std::fs::read_to_string(dir.join("src/lib.grn")).unwrap();
    assert_eq!(ok(RUBY, &format!("{lib}\nputs loud_words(\"a b\").join(\",\")\n")).unwrap(), "A,B\n");
}
