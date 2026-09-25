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
        cargo.args(["build", "--quiet", "-p", "grenat_host", "-p", "grenat_standalone"]);
        if !cfg!(debug_assertions) {
            cargo.arg("--release");
        }
        let status = cargo.status().unwrap();
        assert!(status.success(), "cannot build the libraries");
        for name in ["libgrenat_host.a", "libgrenat_standalone.a"] {
            let lib = Path::new(env!("CARGO_BIN_EXE_grenat")).with_file_name(name);
            assert!(lib.exists(), "no {}", lib.display());
        }
    });
}

fn dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("grenat-build-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Builds `src` into an executable; returns its path.
fn build(name: &str, src: &str) -> PathBuf {
    build_with(name, src, &[])
}

fn build_with(name: &str, src: &str, flags: &[&str]) -> PathBuf {
    ensure_host_library();
    let source = dir().join(format!("{name}.grn"));
    std::fs::write(&source, src).unwrap();
    let exe = dir().join(name);
    let mut args = vec!["build"];
    args.extend(flags);
    args.extend([source.to_str().unwrap(), "-o", exe.to_str().unwrap()]);
    let out = grenat(&args);
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

// ── Standalone programs (`--native`) ───────────────────────────

const STANDALONE: &str = "\
struct Point
  x: Float
  y: Float
end

def fib(n: Int) -> Int
  return n if n < 2
  fib(n - 1) + fib(n - 2)
end

def greet(name: String)
  puts \"hello #{name}\"
end

def main(args: Array(String))
  puts \"fib(20) = #{fib(20)}\"
  args.each do |a|
    greet(a)
  end
  p Point(x: 1.0, y: 2.5)
  p [\"a\", \"b\\\"c\"]
  puts [1, 2]
  print \"no newline \", 42, true
  puts
  exit args.length
end
";

#[test]
fn a_native_program_behaves_like_grenat_run() {
    let exe = build_with("standalone", STANDALONE, &["--native"]);
    let source = dir().join("standalone.grn");
    let built = run_exe(&exe, &["Ada", "Linus"], &[]);
    let ran = grenat(&["run", source.to_str().unwrap(), "Ada", "Linus"]);
    assert_eq!(
        text(&built.stdout),
        "fib(20) = 6765\nhello Ada\nhello Linus\nPoint(x: 1.0, y: 2.5)\n[\"a\", \"b\\\"c\"]\n1\n2\nno newline 42true\n"
    );
    assert_eq!(text(&built.stdout), text(&ran.stdout));
    assert_eq!(built.status.code(), Some(2));
    assert_eq!(built.status.code(), ran.status.code());
}

#[test]
fn a_native_program_is_small() {
    let native = build_with("small", STANDALONE, &["--native"]);
    let hosted = build("large", STANDALONE);
    let size = |p: &PathBuf| std::fs::metadata(p).unwrap().len();
    // no interpreter inside
    assert!(size(&native) * 5 < size(&hosted), "{} vs {}", size(&native), size(&hosted));
}

#[test]
fn native_errors_are_reported_where_they_happen() {
    let src = "def div(a: Int, b: Int) -> Int = a / b\ndef main\n  puts div(1, 0)\nend\n";
    let exe = build_with("native_error", src, &["--native"]);
    let out = run_exe(&exe, &[], &[]);
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(err.starts_with("error: ZeroDivisionError: division by zero\n"), "{err}");
    assert!(err.contains("native_error.grn:1:"), "{err}");
    assert!(err.contains("in `div`"), "{err}");
    // where the interpreter would go on with `nil`, a native program stops
    let exe = build_with("native_nil", "def main\n  xs = [1]\n  p xs[3]\nend\n", &["--native"]);
    let err = text(&run_exe(&exe, &[], &[]).stderr);
    assert!(err.starts_with("error: NativeError: an index out of range (`nil`) cannot be represented"), "{err}");
    assert!(err.contains("native_nil.grn:3:5"), "{err}");
}

#[test]
fn a_program_needing_the_interpreter_is_not_built_native() {
    let source = dir().join("hosted_only.grn");
    std::fs::write(&source, "def show(x)\n  puts x\nend\nputs 1\ndef main\n  show(2)\nend\n").unwrap();
    let exe = dir().join("hosted_only");
    let out = grenat(&["build", "--native", source.to_str().unwrap(), "-o", exe.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(err.contains("cannot be compiled without the interpreter"), "{err}");
    assert!(err.contains("top-level statements need the interpreter"), "{err}");
    assert!(err.contains("`show` cannot be compiled: its parameters need types"), "{err}");
    assert!(!exe.exists());
}

#[test]
fn a_program_of_several_files_is_built_with_its_file_table() {
    ensure_host_library();
    let root = dir().join("several");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("lib")).unwrap();
    std::fs::write(root.join("main.grn"), "require \"./lib/div\"\n\ndef main\n  puts div(6, 3)\n  puts div(1, 0)\nend\n").unwrap();
    std::fs::write(root.join("lib/div.grn"), "def div(a: Int, b: Int) -> Int = a / b\n").unwrap();
    let main = root.join("main.grn");
    for flags in [&[][..], &["--native"][..]] {
        let exe = root.join(if flags.is_empty() { "hosted" } else { "native" });
        let mut args = vec!["build"];
        args.extend(flags);
        args.extend([main.to_str().unwrap(), "-o", exe.to_str().unwrap()]);
        let out = grenat(&args);
        assert!(out.status.success(), "{}", text(&out.stderr));
        let out = run_exe(&exe, &[], &[]);
        assert_eq!(text(&out.stdout), "2\n");
        let err = text(&out.stderr);
        assert!(err.contains("ZeroDivisionError: division by zero"), "{err}");
        assert!(err.contains("lib/div.grn:1:"), "{flags:?}: {err}");
    }
}
