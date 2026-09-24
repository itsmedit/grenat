//! Interpréteur de Grenat : exécution directe de l'AST.
//!
//! Le langage de base, les `prompt` typés, les `tool`, les agents-acteurs,
//! les budgets, la teinte `~T` et les capacités sont vérifiés à l'exécution.
//!
//! Concurrence (phase 3) : chaque tâche (programme principal, `parallel_map`,
//! `race`, `tell`) a sa propre pile d'appels et partage l'état global
//! ([`Shared`]). Les valeurs sont `Arc`/`Mutex`. Un agent traite un message à
//! la fois : `ask` exécute le handler dans la tâche de l'appelant, sous le
//! verrou de l'agent ; les interblocages sont détectés.

mod builtins;
mod control;
mod eval;
mod io;
mod llm;
mod outcome;
mod prelude;
mod program;
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

/// Erreur non rattrapée, remontée jusqu'au point d'entrée.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeError {
    /// Type d'erreur Grenat (`NameError`, `TaintError`…).
    pub ty: String,
    pub message: String,
    pub span: Option<Span>,
    /// Pile d'appels, de l'intérieur vers l'extérieur : (fonction, définition).
    pub trace: Vec<(String, Span)>,
}

/// Bilan d'une exécution.
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
    /// Capture de la sortie standard (tests).
    Capture(Arc<Mutex<String>>),
}

pub struct Options {
    /// Fournisseur LLM imposé ; par défaut, le client Anthropic (`ANTHROPIC_API_KEY`).
    pub provider: Option<Arc<dyn Provider>>,
    pub output: Output,
    /// Lignes d'entrée scriptées (approbations) ; par défaut, l'entrée standard.
    pub input: Option<VecDeque<String>>,
    /// Journalise chaque appel LLM et d'outil sur la sortie d'erreur.
    pub log: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { provider: None, output: Output::Stdout, input: None, log: false }
    }
}

/// Exécute les instructions de niveau supérieur puis `main`, si elle existe.
/// Attend la fin de toutes les tâches lancées (`tell`, perdants de `race`).
pub fn run_main(program: &Program, args: Vec<String>, options: Options) -> Result<Summary, RuntimeError> {
    std::thread::scope(|scope| {
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

/// Exécute le script (qui enregistre les `test "…" do … end`), puis chaque test.
pub fn run_tests(program: &Program, options: Options) -> Result<Vec<TestOutcome>, RuntimeError> {
    std::thread::scope(|scope| {
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
