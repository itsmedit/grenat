//! `grenat`: entry point of the toolchain.

mod report;

use std::io::IsTerminal;
use std::process::ExitCode;
use std::{env, fs};

use grenat_lexer::{StrPart, TokenKind};

const USAGE: &str = "\
grenat — an agentic programming language

Usage:
  grenat run [--log] [--unchecked] <file.grn> [args…]
                                 check, then run the program (and `main`)
  grenat test <file.grn>...      run the `test \"…\" do … end` blocks
  grenat check <file.grn>...     check names, types, effects and taint
  grenat parse <file.grn>        print the syntax tree
  grenat tokens <file.grn>       print the tokens
  grenat --version

Environment variables:
  ANTHROPIC_API_KEY   Claude API key (prompts and agents)
  GRENAT_LOG=1        log every LLM and tool call (same as --log)
";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check") if args.len() > 1 => check(&args[1..]),
        Some("parse") if args.len() == 2 => dump_ast(&args[1]),
        Some("tokens") if args.len() == 2 => dump_tokens(&args[1]),
        Some("run") if args.len() > 1 => run(&args[1..]),
        Some("test") if args.len() > 1 => test(&args[1..]),
        Some(cmd @ ("build" | "eval" | "fmt")) => {
            eprintln!("`grenat {cmd}` is coming in a later phase (see the roadmap in SPEC.md)");
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
    fs::read_to_string(path).map_err(|e| eprintln!("error: cannot read {path}: {e}")).ok()
}

fn use_color() -> bool {
    std::io::stderr().is_terminal() && env::var_os("NO_COLOR").is_none()
}

/// Prints the diagnostics; returns `true` if the file is valid.
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
        if load(path, false).is_none() {
            failed += 1;
        }
    }
    let total = paths.len();
    if failed == 0 {
        eprintln!("✓ {total} file(s) OK");
        ExitCode::SUCCESS
    } else {
        eprintln!("✗ {failed} of {total} file(s) failed");
        ExitCode::FAILURE
    }
}

/// Parses and checks `path` (unless `unchecked`); `None` if the file is invalid.
fn load(path: &str, unchecked: bool) -> Option<(String, grenat_ast::Program)> {
    let src = read(path)?;
    let parsed = grenat_parser::parse(&src);
    if !report(path, &src, &parsed.diagnostics) {
        return None;
    }
    if !unchecked && !report(path, &src, &grenat_types::check(&parsed.program)) {
        return None;
    }
    Some((src, parsed.program))
}

/// The interpreter walks the AST recursively: it runs on a large stack.
fn with_big_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(512 * 1024 * 1024)
            .spawn_scoped(s, f)
            .expect("thread")
            .join()
            .expect("interpreter thread")
    })
}

fn render_runtime_error(path: &str, src: &str, error: &grenat_interp::RuntimeError) {
    let mut diag =
        grenat_parser::Diagnostic::new(error.span.unwrap_or_default(), format!("{}: {}", error.ty, error.message));
    for (function, span) in &error.trace {
        diag = diag.with_note(*span, format!("in `{function}`"));
    }
    eprint!("{}", report::render(path, src, &diag, use_color()));
}

fn run(args: &[String]) -> ExitCode {
    let mut args = args;
    let (mut log, mut unchecked) = (env::var_os("GRENAT_LOG").is_some_and(|v| v != "0"), false);
    while let Some(flag) = args.first().filter(|a| a.starts_with("--")) {
        match flag.as_str() {
            "--log" => log = true,
            "--unchecked" => unchecked = true,
            other => {
                eprintln!("unknown option {other}");
                return ExitCode::from(2);
            }
        }
        args = &args[1..];
    }
    let Some(path) = args.first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let Some((src, program)) = load(path, unchecked) else { return ExitCode::FAILURE };
    let program_args = args[1..].to_vec();
    let run = || grenat_interp::run_main(&program, program_args, grenat_interp::Options { log, ..Default::default() });
    match with_big_stack(run) {
        Ok(summary) => {
            if summary.llm_calls > 0 {
                let line = format!(
                    "— {} LLM call(s) · {} tokens · ${:.4}",
                    summary.llm_calls, summary.tokens, summary.cost_usd
                );
                eprintln!("{}", if use_color() { format!("\x1b[2m{line}\x1b[0m") } else { line });
            }
            ExitCode::from(summary.exit_code.clamp(0, 255) as u8)
        }
        Err(error) => {
            render_runtime_error(path, &src, &error);
            ExitCode::FAILURE
        }
    }
}

fn test(paths: &[String]) -> ExitCode {
    let (mut passed, mut failed) = (0, 0);
    for path in paths {
        let Some((src, program)) = load(path, false) else {
            failed += 1;
            continue;
        };
        match with_big_stack(|| grenat_interp::run_tests(&program, grenat_interp::Options::default())) {
            Ok(outcomes) => {
                for outcome in outcomes {
                    match outcome.error {
                        None => {
                            passed += 1;
                            eprintln!("✓ {}", outcome.name);
                        }
                        Some(error) => {
                            failed += 1;
                            eprintln!("✗ {}", outcome.name);
                            render_runtime_error(path, &src, &error);
                        }
                    }
                }
            }
            Err(error) => {
                failed += 1;
                render_runtime_error(path, &src, &error);
            }
        }
    }
    eprintln!("\n{passed} passed, {failed} failed");
    if failed == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
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
    format!("string \"{text}\"")
}
