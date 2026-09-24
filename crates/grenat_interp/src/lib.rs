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
mod eval;
mod io;
mod llm;
mod outcome;
mod prelude;
mod program;
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
}

impl Default for Options {
    fn default() -> Self {
        Options { provider: None, output: Output::Stdout, input: None, log: false, jit: true, linked: None }
    }
}

/// Runs the top-level statements, then `main` if it exists.
/// Waits for every spawned task (`tell`, `race` losers) to finish.
pub fn run_main(program: &Program, args: Vec<String>, options: Options) -> Result<Summary, RuntimeError> {
    on_interpreter_thread(|scope| {
        let mut interp = Interp::new(program, options, spawner(scope))?;
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
    on_interpreter_thread(|scope| {
        let mut interp = Interp::new(program, options, spawner(scope))?;
        if let Err(ctrl) = interp.run_script() {
            return Err(interp.runtime_error(ctrl));
        }
        let tests = std::mem::take(&mut *interp.tests.borrow_mut());
        let mut outcomes = Vec::new();
        for (name, block) in tests {
            let error = match interp.call_block(&block, Vec::new()) {
                Ok(_) => None,
                Err(ctrl) => Some(interp.runtime_error(ctrl)),
            };
            interp.wait_for_tasks();
            outcomes.push(TestOutcome { name, error });
        }
        Ok(outcomes)
    })
}

/// Runs `work` on a thread with the interpreter's stack, inside a scope that
/// outlives every task it spawns: callers need no particular stack themselves.
fn on_interpreter_thread<'e, T, F>(work: F) -> T
where
    T: Send + 'e,
    F: for<'s> FnOnce(&'s std::thread::Scope<'s, 'e>) -> T + Send + 'e,
{
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(stack::MAIN_STACK)
            .spawn_scoped(scope, move || work(scope))
            .expect("spawning the interpreter thread")
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}
