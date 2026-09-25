//! The `grenat` binary: commands, exit codes, diagnostic rendering.

use std::path::PathBuf;
use std::process::{Command, Output};

fn grenat(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_grenat"))
        .args(args)
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .env("NO_COLOR", "1")
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .unwrap()
}

/// Writes a program to a unique temporary file.
fn program(name: &str, src: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("grenat-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, src).unwrap();
    path
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap()
}

#[test]
fn version_and_usage() {
    let out = grenat(&["--version"]);
    assert_eq!(text(&out.stdout), format!("grenat {}\n", env!("CARGO_PKG_VERSION")));
    let out = grenat(&[]);
    assert_eq!(code(&out), 2);
    assert!(text(&out.stderr).contains("Usage:"));
    assert!(text(&grenat(&["--help"]).stdout).contains("grenat check"));
}

#[test]
fn check_accepts_the_examples() {
    let out = grenat(&["check", "examples/basics.grn", "examples/explorer.grn", "examples/support_desk.grn"]);
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("✓ 3 file(s) OK"));
}

#[test]
fn check_reports_syntax_errors_with_location() {
    let path = program("syntax.grn", "def f\n  1\n");
    let out = grenat(&["check", path.to_str().unwrap()]);
    assert_eq!(code(&out), 1);
    let err = text(&out.stderr);
    assert!(err.contains("error: expected `end` to close `def`"), "{err}");
    assert!(err.contains("syntax.grn:3:1"), "{err}");
    assert!(err.contains("note: `def` opened here"), "{err}");
}

#[test]
fn check_reports_taint_errors_with_their_code_and_origin() {
    let path = program(
        "leak.grn",
        "model :m, name: \"claude-haiku-4-5\"\nprompt p(x: String) -> ~String using :m\n  user x\nend\ntool send(b: String) -> Unit uses net\n  puts b\nend\ndef main uses llm, net\n  send(p(\"x\"))\nend\n",
    );
    let out = grenat(&["check", path.to_str().unwrap()]);
    assert_eq!(code(&out), 1);
    let err = text(&out.stderr);
    assert!(err.contains("error[E0412]: an LLM-produced value reaches `send`"), "{err}");
    assert!(err.contains("note: produced here by an LLM"), "{err}");
    assert!(err.contains("= help: validate it"), "{err}");
}

#[test]
fn run_executes_main_with_arguments() {
    let path = program("args.grn", "def main(args: Array(String))\n  puts args.join(\"+\")\nend\n");
    let out = grenat(&["run", path.to_str().unwrap(), "a", "b"]);
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "a+b\n");
}

#[test]
fn run_the_bases_example() {
    let out = grenat(&["run", "examples/basics.grn"]);
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    assert!(text(&out.stdout).starts_with("fib(25) = 75025\n"));
}

#[test]
fn run_reports_runtime_errors_with_a_trace() {
    let path = program("crash.grn", "def inner = [1, 2].fetch_all\ndef main\n  inner\nend\n");
    let out = grenat(&["run", "--unchecked", path.to_str().unwrap()]);
    assert_eq!(code(&out), 1);
    let err = text(&out.stderr);
    assert!(err.contains("error: NoMethodError: unknown method `fetch_all` for Array"), "{err}");
    assert!(err.contains("note: in `inner`"), "{err}");
    assert!(err.contains("note: in `main`"), "{err}");
}

#[test]
fn run_refuses_a_program_that_does_not_check_unless_unchecked() {
    let path = program("refused.grn", "def f uses fs.raed\n  1\nend\nputs 42\n");
    let refused = grenat(&["run", path.to_str().unwrap()]);
    assert_eq!(code(&refused), 1);
    assert!(text(&refused.stderr).contains("error[E0500]: unknown effect `fs.raed`"));
    assert!(text(&refused.stdout).is_empty());

    let forced = grenat(&["run", "--unchecked", path.to_str().unwrap()]);
    assert_eq!(code(&forced), 0);
    assert_eq!(text(&forced.stdout), "42\n");
}

#[test]
fn run_propagates_the_exit_code() {
    let path = program("exit.grn", "puts \"before\"\nexit 3\nputs \"after\"\n");
    let out = grenat(&["run", path.to_str().unwrap()]);
    assert_eq!(code(&out), 3);
    assert_eq!(text(&out.stdout), "before\n");
}

#[test]
fn run_without_api_key_explains_what_to_do() {
    let out = grenat(&["run", "examples/explorer.grn", "crates"]);
    assert_eq!(code(&out), 1);
    assert!(text(&out.stderr).contains("LlmError: ANTHROPIC_API_KEY is not set"));
}

#[test]
fn unknown_options_are_rejected() {
    let out = grenat(&["run", "--fast", "examples/basics.grn"]);
    assert_eq!(code(&out), 2);
    assert!(text(&out.stderr).contains("unknown option --fast"));
}

#[test]
fn missing_files_are_reported() {
    let out = grenat(&["check", "nowhere.grn"]);
    assert_eq!(code(&out), 1);
    assert!(text(&out.stderr).contains("cannot read nowhere.grn"));
}

#[test]
fn test_command_reports_each_test() {
    let path = program(
        "tests.grn",
        "test \"addition\" do\n  assert_equal 4, 2 + 2\nend\ntest \"failure\" do\n  assert 1 > 2, \"false\"\nend\n",
    );
    let out = grenat(&["test", path.to_str().unwrap()]);
    assert_eq!(code(&out), 1);
    let err = text(&out.stderr);
    assert!(err.contains("✓ addition"), "{err}");
    assert!(err.contains("✗ failure"), "{err}");
    assert!(err.contains("AssertionError: false"), "{err}");
    assert!(err.contains("1 passed, 1 failed"), "{err}");
}

#[test]
fn tokens_and_parse_dump_the_front_end() {
    let path = program("dump.grn", "x = \"a#{1}\"\n");
    let tokens = text(&grenat(&["tokens", path.to_str().unwrap()]).stdout);
    assert!(tokens.contains("identifier `x`"), "{tokens}");
    assert!(tokens.contains("string \"a#{…}\""), "{tokens}");
    let ast = text(&grenat(&["parse", path.to_str().unwrap()]).stdout);
    assert!(ast.contains("Assign"), "{ast}");
}

/// Every built-in method, run through `grenat run` — which checks the program first:
/// the checker and the interpreter must know exactly the same methods.
#[test]
fn standard_library_tour_matches_its_reference_output() {
    let out = grenat(&["run", "crates/grenat_cli/tests/fixtures/stdlib.grn"]);
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    let expected = include_str!("fixtures/stdlib.out");
    for (i, (got, want)) in text(&out.stdout).lines().zip(expected.lines()).enumerate() {
        assert_eq!(got, want, "line {}", i + 1);
    }
    assert_eq!(text(&out.stdout).lines().count(), expected.lines().count());
}

#[test]
fn log_shows_what_the_jit_compiled_and_no_jit_disables_it() {
    let path = program(
        "jit.grn",
        "def fib(n: Int) -> Int\n  return n if n < 2\n  fib(n - 1) + fib(n - 2)\nend\ndef shout(n: Int) -> Int\n  puts n\n  n\nend\nputs fib(20)\n",
    );
    let out = grenat(&["run", "--log", path.to_str().unwrap()]);
    assert_eq!(text(&out.stdout), "6765\n");
    let err = text(&out.stderr);
    assert!(err.contains("[jit] native: fib\n"), "{err}");
    assert!(err.contains("[jit] `shout` stays interpreted: it calls `puts`, which is not compiled\n"), "{err}");

    let out = grenat(&["run", "--log", "--no-jit", path.to_str().unwrap()]);
    assert_eq!(text(&out.stdout), "6765\n");
    assert!(!text(&out.stderr).contains("[jit]"));
}

#[test]
fn fmt_rewrites_files_and_check_reports_them() {
    let path = program("messy.grn", "def   f( n: Int )->Int\n      n*2   # double\nend\n");
    let file = path.to_str().unwrap();
    let out = grenat(&["fmt", "--check", file]);
    assert_eq!(code(&out), 1);
    assert!(text(&out.stderr).contains("messy.grn is not formatted"), "{}", text(&out.stderr));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "def   f( n: Int )->Int\n      n*2   # double\nend\n");

    let out = grenat(&["fmt", file]);
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "def f(n: Int) -> Int\n  n * 2  # double\nend\n");
    assert_eq!(code(&grenat(&["fmt", "--check", file])), 0);
}

#[test]
fn fmt_leaves_an_invalid_file_alone() {
    let path = program("broken.grn", "def f(\n");
    let out = grenat(&["fmt", path.to_str().unwrap()]);
    assert_eq!(code(&out), 1);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "def f(\n");
}

#[test]
fn the_examples_are_formatted() {
    assert_eq!(code(&grenat(&["fmt", "--check", "examples"])), 0);
}

/// A directory of its own, for a program and its data files.
fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("grenat-cli-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("fixtures")).unwrap();
    dir
}

const MODEL: &str = "model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"\nprompt echo(t: String) -> ~String using :fast\n  user t\nend\n";

#[test]
fn tests_never_reach_a_real_model() {
    let dir = project("offline");
    std::fs::write(dir.join("fixtures/input.txt"), "hello").unwrap();
    let src = format!(
        "{MODEL}test \"mocked\" do\n  mock :fast, replies: [\"HELLO\"]\n  assert_equal \"HELLO\", echo(fixture(\"input.txt\"))\nend\ntest \"unmocked\" do\n  echo(\"x\")\nend\n"
    );
    std::fs::write(dir.join("t.grn"), src).unwrap();
    let out = grenat(&["test", dir.join("t.grn").to_str().unwrap()]);
    let err = text(&out.stderr);
    assert_eq!(code(&out), 1, "{err}");
    assert!(err.contains("✓ mocked"), "{err}");
    assert!(err.contains("✗ unmocked"), "{err}");
    assert!(err.contains("no real model in tests: `claude-haiku-4-5` is called outside any `mock`"), "{err}");
}

#[test]
fn eval_reports_scores_and_fails_under_the_threshold() {
    let dir = project("eval");
    std::fs::write(dir.join("rows.jsonl"), "{\"n\": 1}\n{\"n\": 2}\n{\"n\": 3}\n{\"n\": 4}\n").unwrap();
    let src = "\
eval \"majority\", dataset: \"rows.jsonl\", threshold: 0.5 do |row|
  raise \"odd one\" if row.n == 1
  row.n > 2
end
eval \"all\", dataset: \"rows.jsonl\" do |row|
  row.n > 1
end
";
    std::fs::write(dir.join("e.grn"), src).unwrap();
    let path = dir.join("e.grn");
    let out = grenat(&["eval", path.to_str().unwrap()]);
    let err = text(&out.stderr);
    assert_eq!(code(&out), 1, "{err}");
    assert!(err.contains("✓ majority · score 0.50 (threshold 0.50) · 4 row(s), 1 failed · $0.0000 · "), "{err}");
    assert!(err.contains("    row 1: RuntimeError: odd one\n"), "{err}");
    assert!(err.contains("✗ all · score 0.75 (threshold 1.00) · 4 row(s) · "), "{err}");
    assert!(err.ends_with("1 passed, 1 failed\n"), "{err}");

    let out = grenat(&["eval", path.to_str().unwrap(), "major"]);
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    assert!(!text(&out.stderr).contains("all"));
    let out = grenat(&["eval", path.to_str().unwrap(), "nothing"]);
    assert_eq!(code(&out), 1);
    assert!(text(&out.stderr).contains("no eval matches `nothing`"));
}

#[test]
fn the_triage_example_tests_pass_offline() {
    let out = grenat(&["test", "examples/triage.grn"]);
    let err = text(&out.stderr);
    assert_eq!(code(&out), 0, "{err}");
    assert!(err.ends_with("3 passed, 0 failed\n"), "{err}");
}

/// `grenat` run from `dir`.
fn grenat_in(dir: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_grenat"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .unwrap()
}

#[test]
fn a_new_package_runs_tests_and_checks_without_naming_files() {
    let dir = project("packages");
    let out = grenat_in(&dir, &["new", "hello"]);
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    let package = dir.join("hello");
    for file in ["grenat.toml", ".gitignore", "src/main.grn", "src/lib.grn", "tests/lib_test.grn"] {
        assert!(package.join(file).is_file(), "no {file}");
    }
    assert_eq!(text(&grenat_in(&package, &["run"]).stdout), "Hello, world!\n");
    assert_eq!(text(&grenat_in(&package, &["run", "Ada"]).stdout), "Hello, Ada!\n");
    let out = grenat_in(&package, &["test"]);
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    assert!(text(&out.stderr).ends_with("1 passed, 0 failed\n"));
    let out = grenat_in(&package, &["check"]);
    assert!(text(&out.stderr).contains("✓ 3 file(s) OK"), "{}", text(&out.stderr));
    assert_eq!(code(&grenat_in(&package, &["fmt", "--check", "src", "tests"])), 0);

    assert_eq!(code(&grenat_in(&dir, &["new", "hello"])), 1);
    let out = grenat_in(&dir, &["run"]);
    assert_eq!(code(&out), 2);
    assert!(text(&out.stderr).contains("no file given, and no grenat.toml here or above"));
}

#[test]
fn errors_are_shown_in_the_required_file() {
    let dir = project("required");
    std::fs::write(dir.join("main.grn"), "require \"./lib/math\"\n\ndef main\n  puts half(3)\nend\n").unwrap();
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    std::fs::write(dir.join("lib/math.grn"), "def half(n: Int) -> Int\n  raise \"odd\" if n % 2 == 1\n  n / 2\nend\n").unwrap();
    let out = grenat_in(&dir, &["run", "main.grn"]);
    let err = text(&out.stderr);
    assert_eq!(code(&out), 1, "{err}");
    assert!(err.contains("error: RuntimeError: odd\n --> lib/math.grn:2:3\n"), "{err}");
    assert!(err.contains("note: in `main`\n --> main.grn:3:1\n"), "{err}");

    std::fs::write(dir.join("lib/math.grn"), "def half(n: Int) -> Int = n / \"2\"\n").unwrap();
    let err = text(&grenat_in(&dir, &["check", "main.grn"]).stderr);
    assert!(err.contains("--> lib/math.grn:1:"), "{err}");

    std::fs::write(dir.join("main.grn"), "require \"./lib/nope\"\n").unwrap();
    let err = text(&grenat_in(&dir, &["run", "main.grn"]).stderr);
    assert!(err.contains("cannot find `./lib/nope`"), "{err}");
    assert!(err.contains("--> main.grn:1:1"), "{err}");
}

#[test]
fn the_language_server_speaks_over_stdio() {
    use std::io::Write;
    let dir = project("lsp");
    let file = dir.join("a.grn");
    let messages = [
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
        serde_json::json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {
            "uri": format!("file://{}", file.display()), "languageId": "grenat", "version": 1, "text": "puts nope\n"}}}),
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}),
        serde_json::json!({"jsonrpc": "2.0", "method": "exit"}),
    ];
    let mut input = Vec::new();
    for m in &messages {
        let body = m.to_string();
        write!(input, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_grenat"))
        .arg("lsp")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&input).unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = text(&out.stdout);
    assert!(stdout.contains("\"serverInfo\":{\"name\":\"grenat\""), "{stdout}");
    assert!(stdout.contains("textDocument/publishDiagnostics") && stdout.contains("E0100"), "{stdout}");
}

#[test]
fn the_macros_example_runs() {
    let out = grenat(&["run", "examples/macros.grn"]);
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "Ada: 0 EUR\ntrue\nfalse\n");
}

#[test]
fn errors_in_expanded_code_point_at_the_invocation() {
    let path = program("macro_error.grn", "macro broken(name)\n  def {{name}} -> Int = \"text\"\nend\n\nbroken :f\n");
    let out = grenat(&["check", path.to_str().unwrap()]);
    let err = text(&out.stderr);
    assert_eq!(code(&out), 1, "{err}");
    assert!(err.contains("macro_error.grn:5:1\n"), "{err}");
    assert!(err.contains("5 | broken :f\n"), "{err}");
}

#[test]
fn the_use_cases_pass_their_tests() {
    // in a copy: they write their journals and memory where they run
    let dir = project("usecases");
    let source = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/usecases"));
    let mut tests = Vec::new();
    for entry in std::fs::read_dir(source).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            std::fs::create_dir_all(dir.join(entry.file_name())).unwrap();
            for inner in std::fs::read_dir(&path).unwrap().flatten() {
                std::fs::copy(inner.path(), dir.join(entry.file_name()).join(inner.file_name())).unwrap();
            }
        } else {
            std::fs::copy(&path, dir.join(entry.file_name())).unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with("_test.grn") {
                tests.push(name);
            }
        }
    }
    tests.sort();
    assert_eq!(tests.len(), 9);
    for test in &tests {
        let out = Command::new(env!("CARGO_BIN_EXE_grenat"))
            .args(["test", test])
            .current_dir(&dir)
            .env("NO_COLOR", "1")
            .env_remove("ANTHROPIC_API_KEY")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        let err = text(&out.stderr);
        assert_eq!(code(&out), 0, "{test}: {err}");
        assert!(err.contains(", 0 failed"), "{test}: {err}");
    }
}
