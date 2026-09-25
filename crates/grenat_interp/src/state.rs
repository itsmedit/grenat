//! Execution state: what tasks share and what belongs to each one.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};

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
    /// (role, content blocks: text, documents, images)
    pub messages: Vec<(&'static str, Vec<serde_json::Value>)>,
}

#[derive(Clone)]
pub(crate) struct AgentFrame<'p> {
    /// Agent state (the handler's `self` object).
    pub agent: Arc<Object<'p>>,
    pub handler: &'p Handler,
}

/// A database connection, used by one task at a time.
pub(crate) type SharedConnection = Arc<grenat_green::Mutex<Box<dyn grenat_db::Connection>>>;

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
    /// Mocked models (`mock`): the model mocked (`None`: any), its replies.
    pub mocks: Mutex<Vec<(Option<ModelConfig>, Arc<grenat_llm::Mock>)>>,
    /// Stubbed programs (`mock_shell`).
    pub shell_stubs: Mutex<Vec<crate::eval::ShellStub>>,
    /// SMTP servers (`Mail.connect`): (URL, host), by number.
    pub mailers: Mutex<Vec<(String, String)>>,
    /// Emails a test sent (never sent for real): `Mail.deliveries`.
    pub deliveries: Mutex<Vec<crate::mail::Email>>,
    /// Triggers declared by the script (`every`, `on_webhook`).
    pub schedules: Mutex<Vec<crate::builtins::Schedule<'p>>>,
    pub webhooks: Mutex<Vec<crate::builtins::Webhook<'p>>>,
    /// Declared MCP servers, by name.
    pub mcp_servers: Mutex<HashMap<String, Arc<crate::eval::mcp::McpServer>>>,
    /// Mocked MCP servers (`mock_mcp`), by name.
    pub mcp_stubs: Mutex<HashMap<String, crate::eval::mcp::McpStub>>,
    /// Conversations (`Conversation.new`), by number.
    pub conversations: Mutex<Vec<Arc<Mutex<crate::eval::conversation::ConversationState>>>>,
    /// Open databases (`Db.connect`), by number.
    pub databases: Mutex<Vec<SharedConnection>>,
    /// Stubbed HTTP requests (`mock_http`): (method, URL), the reply.
    pub http_stubs: Mutex<Vec<crate::eval::HttpStub>>,
    /// See [`Options::offline`] and [`Options::record`].
    pub offline: bool,
    pub record: bool,
    /// Where `cassettes/` and `fixtures/` are.
    pub dir: std::path::PathBuf,
    /// Global spending counter (first budget of every task).
    pub total: Arc<Budget>,
    /// Started supervisor children: (supervisor, agent) → instance.
    pub children: Mutex<HashMap<(String, String), Arc<AgentRef<'p>>>>,
    /// The program's approval handler (`Runtime.on_approval`).
    pub approver: Mutex<Option<Value<'p>>>,
    /// The human's double in a test (`with_human`): wins over `approver`.
    pub human_double: Mutex<Option<Value<'p>>>,
    pub tests: Mutex<Vec<(String, Value<'p>)>>,
    pub evals: Mutex<Vec<crate::evals::EvalDef<'p>>>,
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
    /// Secondary tasks in flight (green primitives: waiting parks the task).
    pub active: grenat_green::Mutex<usize>,
    pub idle: grenat_green::Condvar,
    /// Where workflows keep their journals.
    pub journal_dir: std::path::PathBuf,
    /// Native code for the eligible functions: compiled by the JIT, or
    /// linked into the executable (`grenat build`).
    pub jit: Option<grenat_codegen::Native>,
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
    /// Counts cancellation checks, to yield now and then.
    pub steps: std::cell::Cell<u32>,
    pub task_id: u64,
    /// Turns a real exhaustion of this thread's stack into `StackOverflow`.
    pub stack: crate::stack::StackGuard,
    /// Workflows being run (their steps are journaled), innermost last.
    pub workflows: Vec<Arc<crate::eval::workflow::WorkflowRun>>,
    /// Providers of the enclosing `cassette` blocks, innermost last.
    pub providers: Vec<Arc<dyn Provider>>,
    /// The batch this task's model calls go to (`batch_map`).
    pub batch: Option<Arc<crate::eval::batch::BatchRun>>,
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
            mocks: Mutex::new(Vec::new()),
            http_stubs: Mutex::new(Vec::new()),
            databases: Mutex::new(Vec::new()),
            conversations: Mutex::new(Vec::new()),
            shell_stubs: Mutex::new(Vec::new()),
            mailers: Mutex::new(Vec::new()),
            deliveries: Mutex::new(Vec::new()),
            schedules: Mutex::new(Vec::new()),
            webhooks: Mutex::new(Vec::new()),
            mcp_servers: Mutex::new(HashMap::new()),
            mcp_stubs: Mutex::new(HashMap::new()),
            offline: options.offline,
            record: options.record,
            dir: options.dir.clone().unwrap_or_default(),
            total: total.clone(),
            children: Mutex::new(HashMap::new()),
            approver: Mutex::new(None),
            human_double: Mutex::new(None),
            tests: Mutex::new(Vec::new()),
            evals: Mutex::new(Vec::new()),
            output: options.output,
            input: Mutex::new(options.input),
            human: Mutex::new(()),
            log: options.log,
            llm_calls: AtomicU64::new(0),
            spawner,
            next_id: AtomicU64::new(1),
            pool_pick: Mutex::new(()),
            waits: Mutex::new(HashMap::new()),
            active: grenat_green::Mutex::new(0),
            idle: grenat_green::Condvar::new(),
            jit: None,
            journal_dir: options.journal.clone().unwrap_or_else(|| ".grenat/journal".into()),
        };
        let loaded = shared.load();
        let mut link_error = None;
        shared.jit = match (options.jit, options.linked) {
            (false, _) => None,
            (true, None) => grenat_codegen::Native::compile(program).ok(),
            // SAFETY: `linked` is the image of this executable, whose source `program` comes from
            (true, Some(image)) => unsafe { grenat_codegen::Native::link(program, image) }
                .map_err(|e| link_error = Some(e))
                .ok(),
        };
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
            steps: Default::default(),
            task_id: 0,
            stack: crate::stack::StackGuard::here(crate::stack::MAIN_STACK),
            workflows: Vec::new(),
            providers: Vec::new(),
            batch: None,
        };
        loaded.and_then(|()| interp.load_models()).map_err(|ctrl| interp.runtime_error(ctrl))?;
        if let Some(error) = link_error {
            // everything still runs, interpreted
            interp.write_err(&format!("warning: native code not loaded: {error}\n"));
        }
        if interp.log {
            interp.log_jit(options.linked.is_some());
        }
        Ok(interp)
    }
}
