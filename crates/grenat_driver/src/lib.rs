//! Loading and running Grenat programs: what `grenat run` does, shared with
//! the executables built by `grenat build`, which must behave identically.

pub mod report;

use std::io::IsTerminal;
use std::{env, fs};

use grenat_ast::Program;
use grenat_parser::Diagnostic;

pub fn read(path: &str) -> Option<String> {
    fs::read_to_string(path).map_err(|e| eprintln!("error: cannot read {path}: {e}")).ok()
}

pub fn use_color() -> bool {
    std::io::stderr().is_terminal() && env::var_os("NO_COLOR").is_none()
}

/// Prints the diagnostics; returns `true` if there are none.
pub fn report(path: &str, src: &str, diagnostics: &[Diagnostic]) -> bool {
    let color = use_color();
    for diag in diagnostics {
        eprint!("{}", report::render(path, src, diag, color));
    }
    diagnostics.is_empty()
}

/// Parses `src`, then checks it (unless `unchecked`); `None` if it is invalid.
pub fn parse(path: &str, src: &str, unchecked: bool) -> Option<Program> {
    let parsed = grenat_parser::parse(src);
    if !report(path, src, &parsed.diagnostics) {
        return None;
    }
    if !unchecked && !report(path, src, &grenat_types::check(&parsed.program)) {
        return None;
    }
    Some(parsed.program)
}

/// Reads, parses and checks `path`.
pub fn load(path: &str, unchecked: bool) -> Option<(String, Program)> {
    let src = read(path)?;
    let program = parse(path, &src, unchecked)?;
    Some((src, program))
}

pub fn render_runtime_error(path: &str, src: &str, error: &grenat_interp::RuntimeError) {
    let mut diag = Diagnostic::new(error.span.unwrap_or_default(), format!("{}: {}", error.ty, error.message));
    for (function, span) in &error.trace {
        diag = diag.with_note(*span, format!("in `{function}`"));
    }
    eprint!("{}", report::render(path, src, &diag, use_color()));
}

/// `GRENAT_LOG`: log LLM and tool calls, and what runs natively.
pub fn log_from_env() -> bool {
    env::var_os("GRENAT_LOG").is_some_and(|v| v != "0")
}

/// `GRENAT_JIT=0`: interpret everything.
pub fn native_from_env() -> bool {
    env::var_os("GRENAT_JIT").is_none_or(|v| v != "0")
}

/// Runs `program` (its top-level code, then `main`): prints the LLM usage,
/// or the runtime error, and returns the program's exit status.
pub fn execute(path: &str, src: &str, program: &Program, args: Vec<String>, options: grenat_interp::Options) -> u8 {
    match grenat_interp::run_main(program, args, options) {
        Ok(summary) => {
            if summary.llm_calls > 0 {
                let line = format!(
                    "— {} LLM call(s) · {} tokens · ${:.4}",
                    summary.llm_calls, summary.tokens, summary.cost_usd
                );
                eprintln!("{}", if use_color() { format!("\x1b[2m{line}\x1b[0m") } else { line });
            }
            summary.exit_code.clamp(0, 255) as u8
        }
        Err(error) => {
            render_runtime_error(path, src, &error);
            1
        }
    }
}
