//! État d'une exécution : ce qui est partagé entre tâches et ce qui est propre à chacune.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Condvar, Mutex};

use grenat_ast::{FnDef, Handler, Program};
use grenat_llm::{ModelConfig, Provider};

use crate::value::{AgentRef, Budget, Object, Scope, Value, new_scope};
use crate::*;

/// Capacités déclarées par `uses` : (effet, restriction évaluée).
pub(crate) type Capabilities = Vec<(String, Option<String>)>;

pub(crate) struct Frame<'p> {
    pub self_val: Option<Value<'p>>,
    pub scope: Scope<'p>,
}

#[derive(Default)]
pub(crate) struct PromptCtx {
    pub system: Vec<String>,
    /// (rôle, texte)
    pub messages: Vec<(&'static str, String)>,
}

#[derive(Clone)]
pub(crate) struct AgentFrame<'p> {
    /// État de l'agent (l'objet `self` du handler).
    pub agent: Arc<Object<'p>>,
    pub handler: &'p Handler,
}

/// Profondeur d'appel maximale (l'interpréteur tourne sur une pile de 512 Mo).
pub(crate) const MAX_DEPTH: usize = 20_000;

/// État partagé par toutes les tâches d'une exécution.
pub(crate) struct Shared<'p> {
    pub program: &'p Program,
    pub fns: HashMap<&'p str, &'p FnDef>,
    pub types: HashMap<&'p str, TypeInfo<'p>>,
    /// Nom de variante → nom de l'enum.
    pub variants: HashMap<&'p str, &'p str>,
    /// Noms des messages gérés par au moins un agent (`on Research`).
    pub messages: HashSet<&'p str>,
    pub models: Mutex<Vec<(String, ModelConfig)>>,
    pub provider: Mutex<Option<Arc<dyn Provider>>>,
    /// Compteur global de dépense (premier budget de chaque tâche).
    pub total: Arc<Budget>,
    /// Enfants de superviseur démarrés : (superviseur, agent) → instance.
    pub children: Mutex<HashMap<(String, String), Arc<AgentRef<'p>>>>,
    pub approver: Mutex<Option<Value<'p>>>,
    pub tests: Mutex<Vec<(String, Value<'p>)>>,
    pub output: Output,
    pub input: Mutex<Option<VecDeque<String>>>,
    /// Une seule question posée à l'humain à la fois.
    pub human: Mutex<()>,
    pub log: bool,
    pub llm_calls: AtomicU64,
    pub spawner: Spawner<'p>,
    pub next_id: AtomicU64,
    /// Détection d'interblocage : tâche → agent qu'elle attend.
    pub waits: Mutex<HashMap<u64, Arc<AgentRef<'p>>>>,
    /// Tâches secondaires en cours.
    pub active: Mutex<usize>,
    pub idle: Condvar,
}

/// Une tâche d'exécution : sa pile d'appels, ses budgets, ses capacités.
pub(crate) struct Interp<'p> {
    pub shared: Arc<Shared<'p>>,
    pub frames: Vec<Frame<'p>>,
    pub prompts: Vec<PromptCtx>,
    pub agents: Vec<AgentFrame<'p>>,
    /// Budgets actifs ; le premier compte toute l'exécution.
    pub budgets: Vec<Arc<Budget>>,
    /// Capacités déclarées par les fonctions en cours d'exécution : (fonction, [(effet, restriction)]).
    pub capabilities: Vec<(String, Capabilities)>,
    pub depth: usize,
    pub max_depth: usize,
    /// Drapeaux d'annulation de cette tâche et de ses ancêtres (`race`, `parallel_map`).
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
