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
