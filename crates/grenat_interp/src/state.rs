//! Execution state: what tasks share and what belongs to each one.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Condvar, Mutex};

use grenat_ast::{FnDef, Handler, Program};
use grenat_llm::{ModelConfig, Provider};

use crate::value::{AgentRef, Budget, Object, Scope, Value, new_scope};
use crate::*;

/// Capabilities declared by `uses`: (effect, evaluated restriction).
pub(crate) type Capabilities = Vec<(String, Option<String>)>;

pub(crate) struct Frame<'p> {
    pub self_val: Option<Value<'p>>,
    pub scope: Scope<'p>,
}

#[derive(Default)]
pub(crate) struct PromptCtx {
    pub system: Vec<String>,
    /// (role, text)
    pub messages: Vec<(&'static str, String)>,
}

#[derive(Clone)]
pub(crate) struct AgentFrame<'p> {
    /// Agent state (the handler's `self` object).
    pub agent: Arc<Object<'p>>,
    pub handler: &'p Handler,
}

/// Maximum call depth (the interpreter runs on a 512 MB stack).
pub(crate) const MAX_DEPTH: usize = 20_000;

/// State shared by every task of a run.
pub(crate) struct Shared<'p> {
    pub program: &'p Program,
    pub fns: HashMap<&'p str, &'p FnDef>,
    pub types: HashMap<&'p str, TypeInfo<'p>>,
    /// Variant name → enum name.
    pub variants: HashMap<&'p str, &'p str>,
    /// Names of the messages handled by at least one agent (`on Research`).
    pub messages: HashSet<&'p str>,
    pub models: Mutex<Vec<(String, ModelConfig)>>,
    pub provider: Mutex<Option<Arc<dyn Provider>>>,
    /// Global spending counter (first budget of every task).
    pub total: Arc<Budget>,
    /// Started supervisor children: (supervisor, agent) → instance.
    pub children: Mutex<HashMap<(String, String), Arc<AgentRef<'p>>>>,
    pub approver: Mutex<Option<Value<'p>>>,
    pub tests: Mutex<Vec<(String, Value<'p>)>>,
    pub output: Output,
    pub input: Mutex<Option<VecDeque<String>>>,
    /// Only one question is asked to the human at a time.
    pub human: Mutex<()>,
    pub log: bool,
    pub llm_calls: AtomicU64,
    pub spawner: Spawner<'p>,
    pub next_id: AtomicU64,
    /// Picking an agent in a pool (atomic pick and reservation).
    pub pool_pick: Mutex<()>,
    /// Deadlock detection: task → agent it waits for.
    pub waits: Mutex<HashMap<u64, Arc<AgentRef<'p>>>>,
    /// Secondary tasks in flight.
    pub active: Mutex<usize>,
    pub idle: Condvar,
}

/// An execution task: its call stack, budgets and capabilities.
pub(crate) struct Interp<'p> {
    pub shared: Arc<Shared<'p>>,
    pub frames: Vec<Frame<'p>>,
    pub prompts: Vec<PromptCtx>,
    pub agents: Vec<AgentFrame<'p>>,
    /// Active budgets; the first one counts the whole run.
    pub budgets: Vec<Arc<Budget>>,
    /// Capabilities declared by the running functions: (function, [(effect, restriction)]).
    pub capabilities: Vec<(String, Capabilities)>,
    pub depth: usize,
    pub max_depth: usize,
    /// Cancellation flags of this task and its ancestors (`race`, `parallel_map`).
    pub cancel: Vec<Arc<AtomicBool>>,
    pub task_id: u64,
}

impl<'p> Deref for Interp<'p> {
    type Target = Shared<'p>;

    fn deref(&self) -> &Shared<'p> {
        &self.shared
    }
}

impl<'p> Interp<'p> {
    pub(crate) fn new(program: &'p Program, options: Options, spawner: Spawner<'p>) -> Result<Self, RuntimeError> {
        let total = Arc::new(Budget::unlimited());
        let mut shared = Shared {
            program,
            fns: HashMap::new(),
            types: HashMap::new(),
            variants: HashMap::new(),
            messages: HashSet::new(),
            models: Mutex::new(Vec::new()),
            provider: Mutex::new(options.provider),
            total: total.clone(),
            children: Mutex::new(HashMap::new()),
            approver: Mutex::new(None),
            tests: Mutex::new(Vec::new()),
            output: options.output,
            input: Mutex::new(options.input),
            human: Mutex::new(()),
            log: options.log,
            llm_calls: AtomicU64::new(0),
            spawner,
            next_id: AtomicU64::new(1),
            pool_pick: Mutex::new(()),
            waits: Mutex::new(HashMap::new()),
            active: Mutex::new(0),
            idle: Condvar::new(),
        };
        let loaded = shared.load();
        let mut interp = Interp {
            shared: Arc::new(shared),
            frames: vec![Frame { self_val: None, scope: new_scope(None) }],
            prompts: Vec::new(),
            agents: Vec::new(),
            budgets: vec![total],
            capabilities: Vec::new(),
            depth: 0,
            max_depth: MAX_DEPTH,
            cancel: Vec::new(),
            task_id: 0,
        };
        loaded.and_then(|()| interp.load_models()).map_err(|ctrl| interp.runtime_error(ctrl))?;
        Ok(interp)
    }
}
