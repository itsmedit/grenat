//! Le binaire `grenat` : commandes, codes de sortie, rendu des diagnostics.

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

/// Écrit un programme dans un fichier temporaire unique.
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
    assert!(text(&out.stderr).contains("Usage :"));
    assert!(text(&grenat(&["--help"]).stdout).contains("grenat check"));
}

#[test]
fn check_accepts_the_examples() {
    let out = grenat(&["check", "examples/bases.grn", "examples/explorateur.grn", "examples/support_desk.grn"]);
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("✓ 3 fichier(s) valide(s)"));
}

#[test]
fn check_reports_syntax_errors_with_location() {
    let path = program("syntaxe.grn", "def f\n  1\n");
    let out = grenat(&["check", path.to_str().unwrap()]);
    assert_eq!(code(&out), 1);
    let err = text(&out.stderr);
    assert!(err.contains("erreur: `end` attendu pour fermer `def`"), "{err}");
    assert!(err.contains("syntaxe.grn:3:1"), "{err}");
    assert!(err.contains("note: `def` ouvert ici"), "{err}");
}

#[test]
fn check_reports_taint_errors_with_their_code_and_origin() {
    let path = program(
        "fuite.grn",
        "model :m, name: \"claude-haiku-4-5\"\nprompt p(x: String) -> ~String using :m\n  user x\nend\ntool send(b: String) -> Unit uses net\n  puts b\nend\ndef main uses llm, net\n  send(p(\"x\"))\nend\n",
    );
    let out = grenat(&["check", path.to_str().unwrap()]);
    assert_eq!(code(&out), 1);
    let err = text(&out.stderr);
    assert!(err.contains("erreur[E0412]: une valeur produite par un LLM atteint `send`"), "{err}");
    assert!(err.contains("note: produite ici par un LLM"), "{err}");
    assert!(err.contains("= aide : validez-la"), "{err}");
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
    let out = grenat(&["run", "examples/bases.grn"]);
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    assert!(text(&out.stdout).starts_with("fib(25) = 75025\n"));
}

#[test]
fn run_reports_runtime_errors_with_a_trace() {
    let path = program("crash.grn", "def inner = [1, 2].fetch_all\ndef main\n  inner\nend\n");
    let out = grenat(&["run", "--unchecked", path.to_str().unwrap()]);
    assert_eq!(code(&out), 1);
    let err = text(&out.stderr);
    assert!(err.contains("erreur: NoMethodError : méthode `fetch_all` inconnue pour Array"), "{err}");
    assert!(err.contains("note: dans `inner`"), "{err}");
    assert!(err.contains("note: dans `main`"), "{err}");
}

#[test]
fn run_refuses_a_program_that_does_not_check_unless_unchecked() {
    let path = program("refus.grn", "def f uses fs.raed\n  1\nend\nputs 42\n");
    let refused = grenat(&["run", path.to_str().unwrap()]);
    assert_eq!(code(&refused), 1);
    assert!(text(&refused.stderr).contains("erreur[E0500]: effet inconnu `fs.raed`"));
    assert!(text(&refused.stdout).is_empty());

    let forced = grenat(&["run", "--unchecked", path.to_str().unwrap()]);
    assert_eq!(code(&forced), 0);
    assert_eq!(text(&forced.stdout), "42\n");
}

#[test]
fn run_propagates_the_exit_code() {
    let path = program("exit.grn", "puts \"avant\"\nexit 3\nputs \"après\"\n");
    let out = grenat(&["run", path.to_str().unwrap()]);
    assert_eq!(code(&out), 3);
    assert_eq!(text(&out.stdout), "avant\n");
}

#[test]
fn run_without_api_key_explains_what_to_do() {
    let out = grenat(&["run", "examples/explorateur.grn", "crates"]);
    assert_eq!(code(&out), 1);
    assert!(text(&out.stderr).contains("LlmError : ANTHROPIC_API_KEY n'est pas définie"));
}

#[test]
fn unknown_options_are_rejected() {
    let out = grenat(&["run", "--vite", "examples/bases.grn"]);
    assert_eq!(code(&out), 2);
    assert!(text(&out.stderr).contains("option inconnue --vite"));
}

#[test]
fn missing_files_are_reported() {
    let out = grenat(&["check", "nulle_part.grn"]);
    assert_eq!(code(&out), 1);
    assert!(text(&out.stderr).contains("lecture de nulle_part.grn impossible"));
}

#[test]
fn test_command_reports_each_test() {
    let path = program(
        "tests.grn",
        "test \"addition\" do\n  assert_equal 4, 2 + 2\nend\ntest \"échec\" do\n  assert 1 > 2, \"faux\"\nend\n",
    );
    let out = grenat(&["test", path.to_str().unwrap()]);
    assert_eq!(code(&out), 1);
    let err = text(&out.stderr);
    assert!(err.contains("✓ addition"), "{err}");
    assert!(err.contains("✗ échec"), "{err}");
    assert!(err.contains("AssertionError : faux"), "{err}");
    assert!(err.contains("1 réussi(s), 1 échoué(s)"), "{err}");
}

#[test]
fn tokens_and_parse_dump_the_front_end() {
    let path = program("dump.grn", "x = \"a#{1}\"\n");
    let tokens = text(&grenat(&["tokens", path.to_str().unwrap()]).stdout);
    assert!(tokens.contains("identifiant `x`"), "{tokens}");
    assert!(tokens.contains("chaîne \"a#{…}\""), "{tokens}");
    let ast = text(&grenat(&["parse", path.to_str().unwrap()]).stdout);
    assert!(ast.contains("Assign"), "{ast}");
}

/// Chaque méthode intégrée, exécutée par `grenat run` — qui vérifie d'abord le programme :
/// le vérificateur et l'interpréteur doivent connaître exactement les mêmes méthodes.
#[test]
fn standard_library_tour_matches_its_reference_output() {
    let out = grenat(&["run", "crates/grenat_cli/tests/fixtures/stdlib.grn"]);
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    let expected = include_str!("fixtures/stdlib.out");
    for (i, (got, want)) in text(&out.stdout).lines().zip(expected.lines()).enumerate() {
        assert_eq!(got, want, "ligne {}", i + 1);
    }
    assert_eq!(text(&out.stdout).lines().count(), expected.lines().count());
}
