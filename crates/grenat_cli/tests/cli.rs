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
    assert!(err.contains("error[E0412]: an untrusted value reaches `send`"), "{err}");
    assert!(err.contains("note: untrusted from here (a model's answer or a network response)"), "{err}");
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
            // the services are stubbed: any key will do
            .env("GITHUB_TOKEN", "test")
            .env("GITHUB_WEBHOOK_SECRET", "test")
            .env("BRAVE_API_KEY", "test")
            .env("LINEAR_TOKEN", "test")
            .env("NOTION_TOKEN", "test")
            .env("SMTP_URL", "smtp://smtp.test")
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

#[test]
fn serve_runs_schedules_and_receives_webhooks() {
    use std::io::{BufRead, BufReader, Read, Write};
    let path = program(
        "served.grn",
        "every 0.2 do\n  puts \"tick\"\nend\non_webhook \"/hook\", token: \"t0k\" do |req|\n  \"got #{req.json[\"n\"].trust!}\"\nend\n",
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_grenat"))
        .args(["serve", "--listen", "127.0.0.1:0", path.to_str().unwrap()])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    stderr.read_line(&mut line).unwrap();
    let address = line.trim().strip_prefix("listening on http://").unwrap_or_else(|| panic!("{line}")).to_string();
    let post = |headers: &str| {
        let mut stream = std::net::TcpStream::connect(&address).unwrap();
        let body = "{\"n\": 7}";
        write!(stream, "POST /hook HTTP/1.1\r\nHost: x\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    };
    let ok = post("Authorization: Bearer t0k\r\n");
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    assert!(ok.ends_with("got 7"), "{ok}");
    assert!(post("").starts_with("HTTP/1.1 401"));
    std::thread::sleep(std::time::Duration::from_millis(700));
    child.kill().unwrap();
    child.wait().unwrap();
    let mut out = String::new();
    child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
    assert!(out.matches("tick").count() >= 2, "{out}");

    let idle = program("idle.grn", "puts 1\n");
    let out = grenat(&["serve", idle.to_str().unwrap()]);
    assert_eq!(code(&out), 1);
    assert!(text(&out.stderr).contains("nothing to serve"), "{}", text(&out.stderr));
}

#[test]
fn serve_answers_routes() {
    use std::io::{BufRead, BufReader, Read, Write};
    let path = program("web.grn", "get \"/hello/:name\" do |req|\n  html \"<b>#{Html.escape(req.params[\"name\"])}</b>\"\nend\n");
    let mut child = Command::new(env!("CARGO_BIN_EXE_grenat"))
        .args(["serve", "--listen", "127.0.0.1:0", path.to_str().unwrap()])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stderr.take().unwrap()).read_line(&mut line).unwrap();
    let address = line.trim().strip_prefix("listening on http://").unwrap().to_string();
    let mut stream = std::net::TcpStream::connect(&address).unwrap();
    write!(stream, "GET /hello/Ada%26Co HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.to_lowercase().contains("content-type: text/html; charset=utf-8"), "{response}");
    assert!(response.ends_with("<b>Ada&amp;Co</b>"), "{response}");
}

#[test]
fn serve_exposes_tools_to_an_mcp_client() {
    use std::io::{BufRead, BufReader};
    let src = "## Adds two numbers.\ntool add(a: Int, b: Int) -> Int\n  a + b\nend\nexpose \"/mcp\", tools: [:add], token: \"t0ken\", name: \"maths\"\n";
    let path = program("exposed.grn", src);
    let mut child = Command::new(env!("CARGO_BIN_EXE_grenat"))
        .args(["serve", "--listen", "127.0.0.1:0", path.to_str().unwrap()])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stderr.take().unwrap()).read_line(&mut line).unwrap();
    let address = line.trim().strip_prefix("listening on http://").unwrap().to_string();
    let url = format!("http://{address}/mcp");
    let connect = |token: &str| {
        let headers = [("Authorization".to_string(), format!("Bearer {token}"))];
        let transport = grenat_mcp::Http::new(&url, &headers, std::time::Duration::from_secs(10));
        grenat_mcp::Client::connect(Box::new(transport))
    };
    let refused = connect("guess").err();
    let mut client = connect("t0ken").unwrap();
    let tools = client.tools().unwrap();
    let sum = client.call("add", serde_json::json!({"a": 2, "b": 40})).unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(refused.is_some_and(|e| e.contains("401")));
    assert_eq!(client.server, "maths");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].description, "Adds two numbers.");
    assert!(tools[0].read_only);
    assert_eq!(sum, grenat_mcp::CallResult { text: "42".into(), is_error: false });
}

#[test]
fn migrate_applies_pending_migrations() {
    let dir = project("migrate");
    let db = dir.join("app.db");
    let src = format!("database \"sqlite://{}\"\nmigration \"001_notes\" do |db|\n  db.migrate(\"CREATE TABLE notes (id INTEGER PRIMARY KEY, text TEXT)\")\nend\n", db.display());
    std::fs::write(dir.join("app.grn"), src).unwrap();
    let path = dir.join("app.grn");
    let out = grenat(&["migrate", path.to_str().unwrap()]);
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("applied 001_notes"), "{}", text(&out.stderr));
    let again = grenat(&["migrate", path.to_str().unwrap()]);
    assert!(text(&again.stderr).contains("the database is up to date"), "{}", text(&again.stderr));
}

#[test]
fn serve_runs_queued_jobs() {
    use std::io::{BufRead, BufReader, Read, Write};
    let dir = project("jobs");
    let db = dir.join("app.db");
    let src = format!(
        "database \"sqlite://{}\"\nstruct Note\n  table :notes\n  id: Int?\n  text: String\nend\nmigration \"001\" do |db|\n  db.migrate(\"CREATE TABLE notes (id #{{db.primary_key}}, text TEXT NOT NULL)\")\nend\ndef note(text: String) uses db = Note.create(text:)\npost \"/notes\" do |req|\n  enqueue(:note, \"queued\")\n  202\nend\nget \"/count\" do |req|\n  Note.count.to_s\nend\n",
        db.display()
    );
    let path = dir.join("app.grn");
    std::fs::write(&path, src).unwrap();
    assert_eq!(code(&grenat(&["migrate", path.to_str().unwrap()])), 0);
    let mut child = Command::new(env!("CARGO_BIN_EXE_grenat"))
        .args(["serve", "--listen", "127.0.0.1:0", path.to_str().unwrap()])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stderr.take().unwrap()).read_line(&mut line).unwrap();
    let address = line.trim().strip_prefix("listening on http://").unwrap().to_string();
    let call = |method: &str, path: &str| {
        let mut stream = std::net::TcpStream::connect(&address).unwrap();
        write!(stream, "{method} {path} HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    };
    assert!(call("POST", "/notes").starts_with("HTTP/1.1 202"));
    let mut count = String::new();
    for _ in 0..50 {
        count = call("GET", "/count");
        if count.ends_with("\r\n\r\n1") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(count.ends_with("\r\n\r\n1"), "the job did not run: {count}");
}

#[test]
fn a_generated_application_checks_passes_its_tests_and_migrates() {
    let base = project("generated");
    let new = grenat_in(&base, &["new", "--app", "desk"]);
    assert_eq!(code(&new), 0, "{}", text(&new.stderr));
    let app = base.join("desk");
    let parts: [&[&str]; 6] = [
        &["generate", "agent", "triage"],
        &["generate", "workflow", "onboard"],
        &["g", "record", "ticket", "subject:String", "priority:Int", "score:Float?", "done:Bool"],
        &["g", "tool", "lookup"],
        &["g", "eval", "triage"],
        &["g", "record", "category", "name:String"],
    ];
    for args in parts {
        let out = grenat_in(&app, args);
        assert_eq!(code(&out), 0, "{args:?}: {}", text(&out.stderr));
        assert!(text(&out.stderr).contains("  create  "), "{}", text(&out.stderr));
    }
    let check = grenat_in(&app, &["check"]);
    assert_eq!(code(&check), 0, "{}", text(&check.stderr));
    let eval = grenat_in(&app, &["check", "evals/triage_eval.grn"]);
    assert_eq!(code(&eval), 0, "{}", text(&eval.stderr));
    let test = grenat_in(&app, &["test"]);
    assert_eq!(code(&test), 0, "{}", text(&test.stderr));
    assert!(text(&test.stderr).contains("7 passed, 0 failed"), "{}", text(&test.stderr));
    let fmt = grenat_in(&app, &["fmt", "--check", "src", "tests", "evals"]);
    assert_eq!(code(&fmt), 0, "generated code is not in the canonical layout: {}", text(&fmt.stderr));
    let migrate = grenat_in(&app, &["migrate"]);
    assert_eq!(code(&migrate), 0, "{}", text(&migrate.stderr));
    assert!(text(&migrate.stderr).contains("2 migration(s) applied"), "{}", text(&migrate.stderr));
    // a part is never generated twice
    let again = grenat_in(&app, &["g", "agent", "triage"]);
    assert_eq!(code(&again), 1);
    assert!(text(&again.stderr).contains("already exists"), "{}", text(&again.stderr));
}

#[test]
fn serve_records_what_fails_for_the_console() {
    use std::io::{BufRead, BufReader, Read, Write};
    let dir = project("events");
    let url = format!("sqlite://{}", dir.join("app.db").display());
    let src = format!(
        "database \"{url}\"\nget \"/boom\" do |req|\n  raise ArgumentError, \"no\"\nend\n\
         agent Pager\n  on Show(text: String) -> String\n    html(text)\n    \"shown\"\n  end\nend\n\
         expose \"/mcp\", agents: [Pager], public: true\n"
    );
    let path = dir.join("app.grn");
    std::fs::write(&path, src).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_grenat"))
        .args(["serve", "--listen", "127.0.0.1:0", path.to_str().unwrap()])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    // standard error is closed after this line: logging a failure must not
    // stop the server from recording it
    BufReader::new(child.stderr.take().unwrap()).read_line(&mut line).unwrap();
    let address = line.trim().strip_prefix("listening on http://").unwrap().to_string();
    let send = |request: String| {
        let mut stream = std::net::TcpStream::connect(&address).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    };
    let boom = send("GET /boom HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n".into());
    let body = "{\"text\": \"<script>\"}";
    let refused = send(format!("POST /mcp/pager_show HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()));
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(boom.starts_with("HTTP/1.1 500"), "{boom}");
    assert!(refused.starts_with("HTTP/1.1 422"), "{refused}");
    let mut db = grenat_db::connect(&url).unwrap();
    let all = grenat_ops::events::latest(db.as_mut(), false, 10).unwrap();
    let seen: Vec<_> = all.iter().map(|e| (e.source.as_str(), e.subject.as_str(), e.error.as_str())).collect();
    assert_eq!(seen, [("mcp", "pager_show", "TaintError"), ("request", "GET /boom", "ArgumentError")]);
    let refusals = grenat_ops::events::latest(db.as_mut(), true, 10).unwrap();
    assert_eq!(refusals.len(), 1, "{refusals:?}");
}
