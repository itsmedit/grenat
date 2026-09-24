//! Interpréteur de Grenat (phase 1) : exécution directe de l'AST.
//!
//! Ce qui fonctionne déjà : le langage de base (fonctions, blocs, structs,
//! classes, enums, `case/in`, `rescue`), les `prompt` typés (sortie
//! structurée validée), les `tool`, les agents (exécution synchrone, boucle
//! agentique avec outils), les budgets et la teinte `~T` vérifiée à l'exécution.
//!
//! Ce qui viendra : vérification statique des types et des effets (phase 2),
//! agents concurrents et supervision (phase 3), code natif (phase 4).

mod builtins;
mod eval;
mod llm;
mod value;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write as _;
use std::rc::Rc;

use grenat_ast::{Arg, Directive, Field, FnDef, Handler, Item, Member, Program, Span, TypeDef, TypeKind, Variant};
pub use grenat_llm::{ModelConfig, Provider, Response, Scripted};
pub use value::Value;
use value::{Budget, ErrorVal, Object, Scope, new_scope};

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
    Capture(Rc<RefCell<String>>),
}

pub struct Options {
    /// Fournisseur LLM imposé ; par défaut, le client Anthropic (`ANTHROPIC_API_KEY`).
    pub provider: Option<Rc<dyn Provider>>,
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
pub fn run_main(program: &Program, args: Vec<String>, options: Options) -> Result<Summary, RuntimeError> {
    let mut interp = Interp::new(program, options)?;
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
    let exit_code = match result {
        Ok(()) | Err(Ctrl::Return(_)) => 0,
        Err(Ctrl::Exit(code)) => code,
        Err(other) => return Err(interp.runtime_error(other)),
    };
    Ok(interp.summary(exit_code))
}

/// Exécute le script (qui enregistre les `test "…" do … end`), puis chaque test.
pub fn run_tests(program: &Program, options: Options) -> Result<Vec<TestOutcome>, RuntimeError> {
    let mut interp = Interp::new(program, options)?;
    if let Err(ctrl) = interp.run_script() {
        return Err(interp.runtime_error(ctrl));
    }
    let tests = std::mem::take(&mut interp.tests);
    let mut outcomes = Vec::new();
    for (name, block) in tests {
        let error = match interp.call_block(&block, Vec::new()) {
            Ok(_) => None,
            Err(ctrl) => Some(interp.runtime_error(ctrl)),
        };
        outcomes.push(TestOutcome { name, error });
    }
    Ok(outcomes)
}

// ── Interne ──────────────────────────────────────────────────

pub(crate) enum Ctrl<'p> {
    Raise(Rc<ErrorVal<'p>>),
    Return(Value<'p>),
    Break(Value<'p>),
    Next(Value<'p>),
    Exit(i32),
}

pub(crate) type R<'p> = Result<Value<'p>, Ctrl<'p>>;

pub(crate) fn raise<'p, T>(ty: &str, message: impl Into<String>) -> Result<T, Ctrl<'p>> {
    Err(Ctrl::Raise(Rc::new(ErrorVal::new(ty, message))))
}

#[derive(Default)]
pub(crate) struct Args<'p> {
    pub pos: Vec<Value<'p>>,
    pub named: Vec<(String, Value<'p>)>,
    pub block: Option<Value<'p>>,
}

impl Args<'_> {
    pub fn is_empty(&self) -> bool {
        self.pos.is_empty() && self.named.is_empty() && self.block.is_none()
    }
}

pub(crate) struct TypeInfo<'p> {
    pub def: &'p TypeDef,
    pub fields: Vec<&'p Field>,
    pub methods: HashMap<&'p str, &'p FnDef>,
    pub statics: HashMap<&'p str, &'p FnDef>,
    pub variants: Vec<&'p Variant>,
    pub handlers: HashMap<&'p str, &'p Handler>,
    pub directives: Vec<&'p Directive>,
}

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

pub(crate) struct AgentFrame<'p> {
    pub agent: Rc<Object<'p>>,
    pub handler: &'p Handler,
}

/// Profondeur d'appel maximale (l'interpréteur tourne sur une pile de 512 Mo).
pub(crate) const MAX_DEPTH: usize = 20_000;

pub(crate) struct Interp<'p> {
    pub program: &'p Program,
    pub fns: HashMap<&'p str, &'p FnDef>,
    pub types: HashMap<&'p str, TypeInfo<'p>>,
    /// Nom de variante → nom de l'enum.
    pub variants: HashMap<&'p str, &'p str>,
    /// Noms des messages gérés par au moins un agent (`on Research`).
    pub messages: HashSet<&'p str>,
    pub models: Vec<(String, ModelConfig)>,
    pub provider: Option<Rc<dyn Provider>>,
    /// Budgets actifs ; le premier compte toute l'exécution.
    pub budgets: Vec<Rc<Budget>>,
    pub agent_budgets: HashMap<usize, Rc<Budget>>,
    pub frames: Vec<Frame<'p>>,
    pub prompts: Vec<PromptCtx>,
    pub agents: Vec<AgentFrame<'p>>,
    /// Enfants de superviseur démarrés : (superviseur, agent) → instance.
    pub children: HashMap<(String, String), Value<'p>>,
    pub approver: Option<Value<'p>>,
    pub tests: Vec<(String, Value<'p>)>,
    pub output: Output,
    pub input: Option<VecDeque<String>>,
    pub log: bool,
    pub llm_calls: u64,
    pub depth: usize,
}

impl<'p> Interp<'p> {
    fn new(program: &'p Program, options: Options) -> Result<Self, RuntimeError> {
        let mut interp = Interp {
            program,
            fns: HashMap::new(),
            types: HashMap::new(),
            variants: HashMap::new(),
            messages: HashSet::new(),
            models: Vec::new(),
            provider: options.provider,
            budgets: vec![Rc::new(Budget::unlimited())],
            agent_budgets: HashMap::new(),
            frames: vec![Frame { self_val: None, scope: new_scope(None) }],
            prompts: Vec::new(),
            agents: Vec::new(),
            children: HashMap::new(),
            approver: None,
            tests: Vec::new(),
            output: options.output,
            input: options.input,
            log: options.log,
            llm_calls: 0,
            depth: 0,
        };
        interp.load().map_err(|ctrl| interp.runtime_error(ctrl))?;
        Ok(interp)
    }

    /// Enregistre fonctions, types et modèles avant toute exécution.
    fn load(&mut self) -> Result<(), Ctrl<'p>> {
        let program = self.program;
        for item in &program.items {
            match item {
                Item::Fn(def) => {
                    if self.fns.insert(&def.name.name, def).is_some() {
                        return raise("NameError", format!("fonction `{}` définie deux fois", def.name.name));
                    }
                }
                Item::Type(def) => {
                    let info = TypeInfo::new(def);
                    self.messages.extend(info.handlers.keys().copied());
                    if self.types.insert(&def.name.name, info).is_some() {
                        return raise("NameError", format!("type `{}` défini deux fois", def.name.name));
                    }
                }
                Item::Model(_) | Item::Stmt(_) => {}
            }
        }

        // `include Module` : copie des méthodes non redéfinies
        let includes: Vec<(&'p str, &'p str)> = self
            .types
            .values()
            .flat_map(|info| {
                info.def.members.iter().filter_map(move |m| match m {
                    Member::Include(ty) => Some((info.def.name.name.as_str(), llm::type_name(ty))),
                    _ => None,
                })
            })
            .collect();
        for (owner, module) in includes {
            let Some(methods) = self.types.get(module).map(|m| m.methods.clone()) else {
                return raise("NameError", format!("module `{module}` inconnu (inclus par `{owner}`)"));
            };
            let info = self.types.get_mut(owner).expect("type chargé");
            for (name, def) in methods {
                info.methods.entry(name).or_insert(def);
            }
        }

        for info in self.types.values() {
            for variant in &info.variants {
                self.variants.insert(&variant.name.name, &info.def.name.name);
            }
        }

        for item in &program.items {
            if let Item::Model(decl) = item {
                let config = self.model_config(decl)?;
                self.models.push((decl.name.name.clone(), config));
            }
        }
        Ok(())
    }

    fn model_config(&mut self, decl: &'p grenat_ast::ModelDecl) -> Result<ModelConfig, Ctrl<'p>> {
        let mut config = ModelConfig::new("anthropic", "");
        let mut fallbacks = None;
        for option in &decl.options {
            let Arg::Named { name, value: Some(expr) } = option else {
                return raise("ArgumentError", format!("option invalide pour le modèle `:{}`", decl.name.name));
            };
            let value = self.eval(expr)?;
            match (name.name.as_str(), value) {
                ("provider", Value::Symbol(s) | Value::Str(s)) => config.provider = s.to_string(),
                ("name", Value::Str(s)) => config.name = s.to_string(),
                ("temperature", Value::Float(f)) => config.temperature = Some(f),
                ("temperature", Value::Int(n)) => config.temperature = Some(n as f64),
                ("max_tokens", Value::Int(n)) if n > 0 => config.max_tokens = n as u32,
                ("effort", Value::Symbol(s) | Value::Str(s)) => config.effort = Some(s.to_string()),
                ("fallbacks", Value::Bool(b)) => fallbacks = Some(b),
                (option, value) => {
                    return raise(
                        "ArgumentError",
                        format!("option `{option}: {}` invalide pour le modèle `:{}`", value.inspect(), decl.name.name),
                    );
                }
            }
        }
        if config.name.is_empty() {
            return raise("ArgumentError", format!("le modèle `:{}` n'a pas de `name:`", decl.name.name));
        }
        config.fallbacks = fallbacks.unwrap_or_else(|| ModelConfig::new("", config.name.as_str()).fallbacks);
        Ok(config)
    }

    fn run_script(&mut self) -> Result<(), Ctrl<'p>> {
        for item in &self.program.items {
            if let Item::Stmt(expr) = item {
                self.eval(expr)?;
            }
        }
        Ok(())
    }

    pub(crate) fn runtime_error(&self, ctrl: Ctrl<'p>) -> RuntimeError {
        match ctrl {
            Ctrl::Raise(e) => RuntimeError {
                ty: e.ty.to_string(),
                message: e.message.clone(),
                span: e.span.get(),
                trace: e.trace.borrow().clone(),
            },
            Ctrl::Break(_) | Ctrl::Next(_) => RuntimeError {
                ty: "LocalJumpError".into(),
                message: "`break` ou `next` hors d'une boucle ou d'un bloc".into(),
                span: None,
                trace: Vec::new(),
            },
            Ctrl::Return(_) | Ctrl::Exit(_) => unreachable!("traité par l'appelant"),
        }
    }

    fn summary(&self, exit_code: i32) -> Summary {
        let total = &self.budgets[0];
        Summary { exit_code, llm_calls: self.llm_calls, tokens: total.tokens.get(), cost_usd: total.spent_usd.get() }
    }

    pub(crate) fn write_out(&mut self, text: &str) {
        match &self.output {
            Output::Stdout => {
                let mut stdout = std::io::stdout().lock();
                let _ = stdout.write_all(text.as_bytes());
                let _ = stdout.flush();
            }
            Output::Capture(buffer) => buffer.borrow_mut().push_str(text),
        }
    }

    pub(crate) fn write_err(&mut self, text: &str) {
        match &self.output {
            Output::Stdout => eprint!("{text}"),
            Output::Capture(buffer) => buffer.borrow_mut().push_str(text),
        }
    }

    pub(crate) fn read_line(&mut self) -> Option<String> {
        if let Some(input) = &mut self.input {
            return input.pop_front();
        }
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim_end_matches(['\n', '\r']).to_string()),
        }
    }
}

impl<'p> TypeInfo<'p> {
    fn new(def: &'p TypeDef) -> Self {
        let mut info = TypeInfo {
            def,
            fields: Vec::new(),
            methods: HashMap::new(),
            statics: HashMap::new(),
            variants: Vec::new(),
            handlers: HashMap::new(),
            directives: Vec::new(),
        };
        for member in &def.members {
            match member {
                Member::Field(f) => info.fields.push(f),
                Member::Method(m) if m.on_self => {
                    info.statics.insert(&m.name.name, m);
                }
                Member::Method(m) => {
                    info.methods.insert(&m.name.name, m);
                }
                Member::Variant(v) => info.variants.push(v),
                Member::Handler(h) => {
                    info.handlers.insert(&h.message.name, h);
                }
                Member::Directive(d) => info.directives.push(d),
                Member::Include(_) => {}
            }
        }
        info
    }

    pub fn is(&self, kind: TypeKind) -> bool {
        self.def.kind == kind
    }
}
