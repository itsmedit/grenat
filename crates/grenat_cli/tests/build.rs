//! `grenat build`: executables that behave exactly like `grenat run`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

fn grenat(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_grenat"))
        .args(args)
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .env("NO_COLOR", "1")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("GRENAT_LOG")
        .output()
        .unwrap()
}

/// `grenat build` links with `libgrenat_host.a`, next to the `grenat` binary:
/// build it once (it is a static library, which `cargo test` does not build).
fn ensure_host_library() {
    static BUILT: OnceLock<()> = OnceLock::new();
    BUILT.get_or_init(|| {
        let mut cargo = Command::new(env!("CARGO"));
        cargo.args(["build", "--quiet", "-p", "grenat_host"]);
        if !cfg!(debug_assertions) {
            cargo.arg("--release");
        }
        let status = cargo.status().unwrap();
        assert!(status.success(), "cannot build grenat_host");
        let lib = Path::new(env!("CARGO_BIN_EXE_grenat")).with_file_name("libgrenat_host.a");
        assert!(lib.exists(), "no {}", lib.display());
    });
}

fn dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("grenat-build-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Builds `src` into an executable; returns its path.
fn build(name: &str, src: &str) -> PathBuf {
    ensure_host_library();
    let source = dir().join(format!("{name}.grn"));
    std::fs::write(&source, src).unwrap();
    let exe = dir().join(name);
    let out = grenat(&["build", source.to_str().unwrap(), "-o", exe.to_str().unwrap()]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stderr).starts_with(&format!("✓ built {}", exe.display())), "{}", text(&out.stderr));
    exe
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn run_exe(exe: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(exe);
    command.args(args).env("NO_COLOR", "1").env_remove("GRENAT_LOG").env_remove("ANTHROPIC_API_KEY");
    for (k, v) in env {
        command.env(k, v);
    }
    command.output().unwrap()
}

const PROGRAM: &str = "\
struct Point
  x: Float
  y: Float
end

def fib(n: Int) -> Int
  return n if n < 2
  fib(n - 1) + fib(n - 2)
end

def shout(words: Array(String)) -> String
  out = \"\"
  words.each do |w|
    out = out + w.upcase + \"!\"
  end
  out
end

def far(p: Point, n: Int) -> Point
  n.times do |i|
    p = Point(x: p.x + 1.0, y: p.y * 2.0)
  end
  p
end

def main(args: Array(String))
  puts \"fib(20) = #{fib(20)}\"
  puts shout(args)
  p far(Point(x: 0.0, y: 1.0), 3)
  exit args.length
end
";

#[test]
fn a_built_executable_behaves_like_grenat_run() {
    let exe = build("app", PROGRAM);
    let source = dir().join("app.grn");
    let built = run_exe(&exe, &["a", "b"], &[]);
    let ran = grenat(&["run", source.to_str().unwrap(), "a", "b"]);
    assert_eq!(text(&built.stdout), "fib(20) = 6765\nA!B!\nPoint(x: 3.0, y: 8.0)\n");
    assert_eq!(text(&built.stdout), text(&ran.stdout));
    assert_eq!(built.status.code(), Some(2));
    assert_eq!(built.status.code(), ran.status.code());
}

#[test]
fn the_native_functions_are_linked_not_compiled_at_run_time() {
    let exe = build("logged", PROGRAM);
    let out = run_exe(&exe, &[], &[("GRENAT_LOG", "1")]);
    assert!(text(&out.stderr).starts_with("[aot] native: fib, shout, far\n"), "{}", text(&out.stderr));
    // everything interpreted, still correct
    let out = run_exe(&exe, &["x"], &[("GRENAT_JIT", "0")]);
    assert_eq!(text(&out.stdout), "fib(20) = 6765\nX!\nPoint(x: 3.0, y: 8.0)\n");
}

#[test]
fn runtime_errors_are_reported_by_the_executable() {
    let exe = build("failing", "def div(a: Int, b: Int) -> Int = a / b\ndef main\n  puts div(1, 0)\nend\n");
    let out = run_exe(&exe, &[], &[]);
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(err.contains("ZeroDivisionError: division by zero"), "{err}");
    assert!(err.contains("in `div`"), "{err}");
}

#[test]
fn an_invalid_program_is_not_built() {
    let source = dir().join("invalid.grn");
    std::fs::write(&source, "def f(n: Int) -> Int = n +\n").unwrap();
    let exe = dir().join("invalid");
    let out = grenat(&["build", source.to_str().unwrap(), "-o", exe.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(!exe.exists());
    assert_eq!(grenat(&["build"]).status.code(), Some(2));
}
