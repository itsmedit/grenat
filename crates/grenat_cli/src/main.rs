//! `grenat`: entry point of the toolchain.

mod build;
mod fmt;
mod link;

use std::env;
use std::process::ExitCode;

use grenat_driver::{execute, load, log_from_env, native_from_env, read, render_runtime_error, report};

use grenat_lexer::{StrPart, TokenKind};

const USAGE: &str = "\
grenat — an agentic programming language

Usage:
  grenat run [--log] [--unchecked] [--no-jit] <file.grn> [args…]
                                 check, then run the program (and `main`)
  grenat build [--native] <file.grn> [-o <executable>]
                                 compile the program ahead of time into an executable
                                 (--native: the whole program, without the interpreter)
  grenat test <file.grn>...      run the `test \"…\" do … end` blocks
  grenat check <file.grn>...     check names, types, effects and taint
  grenat fmt [--check] <file.grn | dir>...
                                 rewrite in the canonical layout (--check: only report)
  grenat parse <file.grn>        print the syntax tree
  grenat tokens <file.grn>       print the tokens
  grenat --version

Environment variables:
  ANTHROPIC_API_KEY   Claude API key (prompts and agents)
  GRENAT_LOG=1        log every LLM and tool call, and what the JIT compiled (same as --log)
  GRENAT_JIT=0        interpret everything (same as --no-jit; also in built executables)
  GRENAT_HOME         where `grenat build` finds lib/grenat/libgrenat_{host,standalone}.a
  GRENAT_KEEP_OBJECT  keep the object file of `grenat build` (in the temporary directory)
  CC                  the linker used by `grenat build` (default: cc)
";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check") if args.len() > 1 => check(&args[1..]),
        Some("parse") if args.len() == 2 => dump_ast(&args[1]),
        Some("tokens") if args.len() == 2 => dump_tokens(&args[1]),
        Some("run") if args.len() > 1 => run(&args[1..]),
        Some("test") if args.len() > 1 => test(&args[1..]),
        Some("build") if args.len() > 1 => build::build(&args[1..]),
        Some("fmt") => fmt::fmt(&args[1..]),
        Some(cmd @ "eval") => {
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

fn run(args: &[String]) -> ExitCode {
    let mut args = args;
    let (mut log, mut unchecked, mut jit) = (log_from_env(), false, native_from_env());
    while let Some(flag) = args.first().filter(|a| a.starts_with("--")) {
        match flag.as_str() {
            "--log" => log = true,
            "--unchecked" => unchecked = true,
            "--no-jit" => jit = false,
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
    let options = grenat_interp::Options { log, jit, ..Default::default() };
    ExitCode::from(execute(path, &src, &program, args[1..].to_vec(), options))
}

fn test(paths: &[String]) -> ExitCode {
    let (mut passed, mut failed) = (0, 0);
    for path in paths {
        let Some((src, program)) = load(path, false) else {
            failed += 1;
            continue;
        };
        match grenat_interp::run_tests(&program, grenat_interp::Options::default()) {
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
        let (line, col) = grenat_driver::report::line_col(&src, tok.span.start as usize);
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
