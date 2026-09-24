//! `grenat` : point d'entrée de la chaîne d'outils.

mod report;

use std::io::IsTerminal;
use std::process::ExitCode;
use std::{env, fs};

use grenat_lexer::{StrPart, TokenKind};

const USAGE: &str = "\
grenat — langage de programmation agentique

Usage :
  grenat check <fichier.grn>...   vérifie la syntaxe
  grenat parse <fichier.grn>      affiche l'arbre syntaxique
  grenat tokens <fichier.grn>     affiche les tokens
  grenat --version
";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check") if args.len() > 1 => check(&args[1..]),
        Some("parse") if args.len() == 2 => dump_ast(&args[1]),
        Some("tokens") if args.len() == 2 => dump_tokens(&args[1]),
        Some(cmd @ ("run" | "build" | "test" | "eval" | "fmt")) => {
            eprintln!("`grenat {cmd}` arrive dans une prochaine phase (voir SPEC.md, feuille de route)");
            ExitCode::FAILURE
        }
        Some("-V" | "--version") => {
            println!("grenat {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn read(path: &str) -> Option<String> {
    fs::read_to_string(path).map_err(|e| eprintln!("erreur : lecture de {path} impossible : {e}")).ok()
}

fn use_color() -> bool {
    std::io::stderr().is_terminal() && env::var_os("NO_COLOR").is_none()
}

/// Affiche les diagnostics ; renvoie `true` si le fichier est valide.
fn report(path: &str, src: &str, diagnostics: &[grenat_parser::Diagnostic]) -> bool {
    let color = use_color();
    for diag in diagnostics {
        eprint!("{}", report::render(path, src, diag, color));
    }
    diagnostics.is_empty()
}

fn check(paths: &[String]) -> ExitCode {
    let mut failed = 0;
    for path in paths {
        let ok = read(path).is_some_and(|src| report(path, &src, &grenat_parser::parse(&src).diagnostics));
        if !ok {
            failed += 1;
        }
    }
    let total = paths.len();
    if failed == 0 {
        eprintln!("✓ {total} fichier(s) valide(s)");
        ExitCode::SUCCESS
    } else {
        eprintln!("✗ {failed} fichier(s) en erreur sur {total}");
        ExitCode::FAILURE
    }
}

fn dump_ast(path: &str) -> ExitCode {
    let Some(src) = read(path) else { return ExitCode::FAILURE };
    let parsed = grenat_parser::parse(&src);
    println!("{:#?}", parsed.program);
    if report(path, &src, &parsed.diagnostics) { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn dump_tokens(path: &str) -> ExitCode {
    let Some(src) = read(path) else { return ExitCode::FAILURE };
    let lexed = grenat_lexer::lex(&src);
    for tok in &lexed.tokens {
        let (line, col) = report::line_col(&src, tok.span.start as usize);
        println!("{line:>4}:{col:<4} {}", describe(&tok.kind));
    }
    let diagnostics: Vec<_> =
        lexed.errors.into_iter().map(|e| grenat_parser::Diagnostic::new(e.span, e.message)).collect();
    if report(path, &src, &diagnostics) { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn describe(kind: &TokenKind) -> String {
    let TokenKind::Str(parts) = kind else { return kind.describe() };
    let text: String = parts
        .iter()
        .map(|part| match part {
            StrPart::Lit(s) => s.escape_debug().to_string(),
            StrPart::Interp(..) => "#{…}".into(),
        })
        .collect();
    format!("chaîne \"{text}\"")
}
