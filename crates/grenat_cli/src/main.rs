//! `grenat`: entry point of the toolchain.

mod build;
mod fmt;
mod generate;
mod link;
mod migrate;
mod package;
mod serve;
mod testing;

use std::env;
use std::process::ExitCode;

use grenat_driver::{Sources, execute, load, log_from_env, native_from_env, read, report};

use grenat_lexer::{StrPart, TokenKind};

const USAGE: &str = "\
grenat — an agentic programming language

Usage:
  grenat new <name>              create a package: grenat.toml, src/, tests/
  grenat new --app <name>        create an application: database, models, routes,
                                 src/app.grn requiring its parts, tests/
  grenat generate agent|workflow|record|tool|eval <name> [field:Type…]
                                 add a part to the application, with its tests
                                 (a record: its fields, and the migration of its table)
  grenat run [--log] [--unchecked] [--no-jit] [<file.grn>] [args…]
                                 check, then run the program (and `main`);
                                 without a file, the current package's
  grenat build [--native] [--release] [<file.grn>] [-o <executable>]
                                 compile the program ahead of time into an executable
                                 (--native: the whole program, without the interpreter;
                                 --release: optimized by LLVM, needs clang)
  grenat test [<file.grn>...]    run the `test \"…\" do … end` blocks (never a real model:
                                 `mock`, or `cassette` recorded once); without a
                                 file, those of the package's src/ and tests/
  grenat serve [--listen host:port] [<file.grn>]
                                 serve the program: routes, `expose`d tools and agents,
                                 `on_webhook` handlers (127.0.0.1:3000 by default),
                                 `every` schedules, and job workers
  grenat migrate [<file.grn>]    apply the migrations the database has not seen yet
  grenat eval <file.grn> [name]  run the `eval` blocks (those whose name contains `name`)
  grenat check [<file.grn>...]   check names, types, effects and taint
  grenat update                  fetch the latest commits of git dependencies (grenat.lock)
  grenat fmt [--check] <file.grn | dir>...
                                 rewrite in the canonical layout (--check: only report)
  grenat lsp                     the language server, over standard input and output
  grenat parse <file.grn>        print the syntax tree
  grenat tokens <file.grn>       print the tokens
  grenat --version

Environment variables:
  ANTHROPIC_API_KEY   Claude API key (prompts and agents)
  GRENAT_LOG=1        log every LLM and tool call, and what the JIT compiled (same as --log)
  GRENAT_RECORD=1     record every cassette again, with real calls
  GRENAT_JIT=0        interpret everything (same as --no-jit; also in built executables)
  GRENAT_HOME         where `grenat build` finds lib/grenat/libgrenat_{host,standalone}.a
  GRENAT_KEEP_OBJECT  keep the object file of `grenat build` (in the temporary directory)
  CC                  the linker used by `grenat build` (default: cc)
";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check") => check(&args[1..]),
        Some("parse") if args.len() == 2 => dump_ast(&args[1]),
        Some("tokens") if args.len() == 2 => dump_tokens(&args[1]),
        Some("run") => run(&args[1..]),
        Some("test") => testing::test(&args[1..]),
        Some("eval") if args.len() > 1 => testing::eval(&args[1..]),
        Some("build") => build::build(&args[1..]),
        Some("new") if args.get(1).is_some_and(|a| a == "--app") && args.len() == 3 => generate::new_app(&args[2]),
        Some("new") => package::new(&args[1..]),
        Some("generate" | "g") => generate::generate(&args[1..]),
        Some("serve") => serve::serve(&args[1..]),
        Some("migrate") => migrate::migrate(&args[1..]),
        Some("update") if args.len() == 1 => package::update(),
        Some("fmt") => fmt::fmt(&args[1..]),
        Some("lsp") if args.len() == 1 => {
            let code = grenat_lsp::serve(std::io::stdin().lock(), std::io::stdout().lock());
            ExitCode::from(code as u8)
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
    let paths = match package::or_package_files(paths) {
        Ok(paths) => paths,
        Err(code) => return code,
    };
    let mut failed = 0;
    for path in &paths {
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
    // a program, or else the current package's
    let (path, args) = match args.split_first() {
        Some((file, rest)) if file.ends_with(".grn") => (file.clone(), rest),
        _ => match package::main_file() {
            Ok(main) => (main, args),
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::from(2);
            }
        },
    };
    let Some(loaded) = load(&path, unchecked) else { return ExitCode::FAILURE };
    let options = grenat_interp::Options { log, jit, ..grenat_driver::options_for(&path) };
    ExitCode::from(execute(&loaded.sources, &loaded.program, args.to_vec(), options))
}

fn dump_ast(path: &str) -> ExitCode {
    let Some(src) = read(path) else { return ExitCode::FAILURE };
    let parsed = grenat_parser::parse(&src);
    println!("{:#?}", parsed.program);
    if report(&Sources::single(path, &src), &parsed.diagnostics) { ExitCode::SUCCESS } else { ExitCode::FAILURE }
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
    if report(&Sources::single(path, &src), &diagnostics) { ExitCode::SUCCESS } else { ExitCode::FAILURE }
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
