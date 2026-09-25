//! Running a program, and reporting how it ended.

use std::path::Path;

use grenat_ast::Program;
use grenat_parser::Diagnostic;
use grenat_report::Sources;

use crate::flags::{log_from_env, record_from_env, use_color};

pub fn render_runtime_error(sources: &Sources, error: &grenat_interp::RuntimeError) {
    let mut diag = Diagnostic::new(error.span.unwrap_or_default(), format!("{}: {}", error.ty, error.message));
    for (function, span) in &error.trace {
        diag = diag.with_note(*span, format!("in `{function}`"));
    }
    eprint!("{}", grenat_report::render_in(sources, &diag, use_color()));
}

/// Options to run the program at `path`: its directory holds `cassettes/`,
/// `fixtures/` and datasets.
pub fn options_for(path: &str) -> grenat_interp::Options {
    let dir = Path::new(path).parent().map(Path::to_path_buf);
    grenat_interp::Options { dir, record: record_from_env(), log: log_from_env(), ..Default::default() }
}

/// Runs `program` (its top-level code, then `main`): prints the LLM usage,
/// or the runtime error, and returns the program's exit status.
pub fn execute(sources: &Sources, program: &Program, args: Vec<String>, options: grenat_interp::Options) -> u8 {
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
            render_runtime_error(sources, &error);
            1
        }
    }
}
