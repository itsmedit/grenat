//! Grenat interpreter: direct execution of the AST.
//!
//! The core language, typed `prompt`s, `tool`s, actor agents, budgets, taint
//! `~T` and capabilities are all enforced at run time.
//!
//! Concurrency (phase 3): each task (main program, `parallel_map`, `race`,
//! `tell`) has its own call stack and shares the global state ([`Shared`]).
//! Values are `Arc`/`Mutex`. An agent handles one message at a time: `ask`
//! runs the handler in the caller's task, under the agent's lock; deadlocks
//! are detected.

mod builtins;
mod control;
mod deadlines;
mod eval;
mod evals;
mod http;
mod mail;
mod io;
mod llm;
mod outcome;
mod process;
mod prelude;
mod program;
mod serve;
mod stack;
mod state;
mod task;
mod value;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use grenat_ast::{Program, Span};
pub use grenat_llm::{ModelConfig, Provider, Response, Scripted};
use value::Locked;
pub use value::Value;
pub use evals::{EvalReport, RowOutcome, run_evals};
pub use eval::records::migrate;
pub use serve::serve;

pub(crate) use control::*;
pub(crate) use program::*;
pub(crate) use state::*;
pub(crate) use task::*;

/// Uncaught error, propagated up to the entry point.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeError {
    /// Grenat error type (`NameError`, `TaintError`…).
    pub ty: String,
    pub message: String,
    pub span: Option<Span>,
    /// Call stack, innermost first: (function, definition).
    pub trace: Vec<(String, Span)>,
}

/// Summary of a run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    pub exit_code: i32,
    pub llm_calls: u64,
    pub tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TestOutcome {
    pub name: String,
    pub error: Option<RuntimeError>,
}

pub enum Output {
    Stdout,
    /// Captures standard output (tests).
    Capture(Arc<Mutex<String>>),
}

pub struct Options {
    /// Forced LLM provider; defaults to the Anthropic client (`ANTHROPIC_API_KEY`).
    pub provider: Option<Arc<dyn Provider>>,
    pub output: Output,
    /// Scripted input lines (approvals); defaults to standard input.
    pub input: Option<VecDeque<String>>,
    /// Logs every LLM and tool call to standard error.
    pub log: bool,
    /// Runs eligible functions as native code (see `grenat_codegen`).
    pub jit: bool,
    /// Native code linked into this executable (`grenat build`), instead of the JIT.
    pub linked: Option<&'static grenat_codegen::aot::Image>,
    /// Where workflows keep their journals (default `.grenat/journal`).
    pub journal: Option<std::path::PathBuf>,
    /// The program's directory: `cassettes/` and `fixtures/` are found there
    /// (default: the current directory).
    pub dir: Option<std::path::PathBuf>,
    /// Records every cassette again, with real calls (`GRENAT_RECORD=1`).
    pub record: bool,
    /// No real model: a call neither mocked nor in a cassette is an error
    /// (set by [`run_tests`] when no provider is given).
    pub offline: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            provider: None,
            output: Output::Stdout,
            input: None,
            log: false,
            jit: true,
            linked: None,
            journal: None,
            dir: None,
            record: false,
            offline: false,
        }
    }
}

/// Runs the top-level statements, then `main` if it exists.
/// Waits for every spawned task (`tell`, `race` losers) to finish.
pub fn run_main(program: &Program, args: Vec<String>, options: Options) -> Result<Summary, RuntimeError> {
    on_interpreter_thread(|green| {
        let mut interp = Interp::new(program, options, spawner(green))?;
        let result = interp.run_script().and_then(|()| match interp.fns.get("main").copied() {
            Some(main) => {
                let mut call_args = Args::default();
                if !main.params.is_empty() {
                    call_args.pos.push(Value::array(args.into_iter().map(Value::str).collect()));
                }
                interp.call_fn(main, call_args, None).map(drop)
            }
            None => Ok(()),
        });
        interp.wait_for_tasks();
        let exit_code = match result {
            Ok(()) | Err(Ctrl::Return(_)) => 0,
            Err(Ctrl::Exit(code)) => code,
            Err(other) => return Err(interp.runtime_error(other)),
        };
        Ok(interp.summary(exit_code))
    })
}

/// Runs the script (which registers the `test "…" do … end` blocks), then each test.
pub fn run_tests(program: &Program, options: Options) -> Result<Vec<TestOutcome>, RuntimeError> {
    let options = Options { offline: true, ..options };
    on_interpreter_thread(|green| {
        let mut interp = Interp::new(program, options, spawner(green))?;
        if let Err(ctrl) = interp.run_script() {
            return Err(interp.runtime_error(ctrl));
        }
        let tests = std::mem::take(&mut *interp.tests.borrow_mut());
        let mut outcomes = Vec::new();
        let migrated = !interp.migrations.borrow().is_empty();
        for (name, block) in tests {
            // each test has a database of its own, migrated
            if migrated && let Err(ctrl) = interp.fresh_test_database() {
                outcomes.push(TestOutcome { name, error: Some(interp.runtime_error(ctrl)) });
                continue;
            }
            let error = match interp.call_block(&block, Vec::new()) {
                Ok(_) => None,
                Err(ctrl) => Some(interp.runtime_error(ctrl)),
            };
            interp.wait_for_tasks();
            // each test declares its own mocks
            interp.mocks.borrow_mut().clear();
            interp.http_stubs.borrow_mut().clear();
            interp.shell_stubs.borrow_mut().clear();
            interp.mcp_stubs.borrow_mut().clear();
            interp.deliveries.borrow_mut().clear();
            outcomes.push(TestOutcome { name, error });
        }
        Ok(outcomes)
    })
}

/// Runs `work` as the first green task (with the interpreter's stack), on a
/// pool of worker threads, and returns once every task it spawned has
/// finished: callers need no particular stack themselves.
fn on_interpreter_thread<'e, T, F>(work: F) -> T
where
    T: Send + 'e,
    F: FnOnce(&grenat_green::Spawner<'e>) -> T + Send + 'e,
{
    let config = grenat_green::Config {
        main_stack: stack::MAIN_STACK,
        task_stack: stack::TASK_STACK,
        ..grenat_green::Config::default()
    };
    grenat_green::run(config, work)
}
