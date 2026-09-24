//! Vérification statique de Grenat : noms, types, effets et teinte `~T`.
//!
//! Le vérificateur est **graduel** : ce qu'il ne sait pas typer devient
//! `Ty::Unknown`, compatible avec tout, et ne produit jamais d'erreur. Ce
//! qu'il prouve, en revanche, il le prouve avant l'exécution :
//!
//! - **teinte** : une valeur produite par un LLM ne peut pas atteindre une
//!   fonction à effet dangereux (`shell`, `net`, `fs.write`, `human`) sans
//!   `.check`, `.approve(by: :human)` ou `.trust!` (E0412) ; l'analyse suit
//!   la teinte à travers les appels (chaque fonction est vérifiée pour la
//!   teinte réelle de ses arguments), les champs, l'interpolation, les
//!   blocs et l'état `@…` des agents ;
//! - **effets** : une fonction qui déclare `uses` doit couvrir tout ce que
//!   son corps fait, et `main` comme les `tool` doivent déclarer (E0300) ;
//! - **noms et types** : variables, champs, méthodes, arité, arguments
//!   nommés, types incompatibles (E0100, E0200) ;
//! - **déclarations** : `prompt`, agents et outils bien formés (E0413, E0500).
//!
//! L'interpréteur garde ses vérifications à l'exécution : défense en profondeur.

mod builtins;
mod ty;

use std::collections::{HashMap, HashSet};

pub use grenat_ast::Diagnostic;
use grenat_ast::{
    Arg, ArmTest, BinOp, Block, Body, Directive, Expr, ExprKind, Field, FnDef, FnKind, Handler, Ident, Item, Member,
    Param, Pattern, PatternKind, Program, Span, StrSeg, Type, TypeDef, TypeKind, UnOp, Variant,
};
use ty::{Ty, V, join, join_v};

pub const E_NAME: &str = "E0100";
pub const E_TYPE: &str = "E0200";
pub const E_EFFECT: &str = "E0300";
pub const E_TAINT: &str = "E0412";
pub const E_TAINT_DECL: &str = "E0413";
pub const E_DECL: &str = "E0500";

const KNOWN_EFFECTS: &[&str] = &["llm", "net", "fs", "fs.read", "fs.write", "shell", "human", "time", "random", "env"];
const DANGEROUS_EFFECTS: &[&str] = &["shell", "net", "fs.write", "human"];
const ERROR_NAMES: &[&str] = &[
    "Exception",
    "StandardError",
    "BudgetExceeded",
    "ApprovalDenied",
    "LlmRefusal",
    "MaxTurnsExceeded",
    "NoMatchingPattern",
];
const TAINT_HELP: &str = "validez-la avec `.check { … }`, `.approve(by: :human)` ou `.trust!`";

/// Vérifie un programme ; renvoie les diagnostics triés par position.
pub fn check(program: &Program) -> Vec<Diagnostic> {
    let mut checker = Checker::new(program);
    checker.collect();
    checker.infer_ivars();
    // l'état `@…` teinté et les effets des fonctions récursives se propagent d'une passe à l'autre
    for _ in 0..4 {
        let before = (checker.ivar_taint.len(), checker.effect_count());
        checker.memo.clear();
        checker.pass();
        if (checker.ivar_taint.len(), checker.effect_count()) == before {
            break;
        }
    }
    let mut diags = checker.diags;
    diags.sort_by_key(|d| (d.span.start, d.span.end));
    diags
}

fn is_error_name(name: &str) -> bool {
    name.ends_with("Error") || ERROR_NAMES.contains(&name)
}

fn type_name(ty: &Type) -> &str {
    match ty {
        Type::Named { path, .. } => &path.last().expect("chemin non vide").name,
        Type::Optional(inner, _) | Type::Tainted(inner, _) => type_name(inner),
    }
}

fn is_tainted_decl(ty: &Type) -> bool {
    match ty {
        Type::Tainted(..) => true,
        Type::Optional(inner, _) => is_tainted_decl(inner),
        Type::Named { .. } => false,
    }
}

/// Distance de Damerau-Levenshtein (une inversion de deux lettres compte pour une erreur).
fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

/// Le nom le plus proche, s'il est assez proche pour être une faute de frappe.
fn suggest<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let limit = (name.chars().count() / 3).max(1);
    candidates
        .into_iter()
        .filter(|c| *c != name)
        .map(|c| (edit_distance(name, c), c))
        .filter(|(d, _)| *d <= limit)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| format!("vouliez-vous `{c}` ?"))
}

/// Effet inféré ou déclaré : `fs.read("./docs")`.
#[derive(Debug, Clone, PartialEq)]
struct Eff {
    path: String,
    arg: Option<String>,
    origin: Span,
}

impl Eff {
    fn label(&self) -> String {
        match &self.arg {
            Some(arg) => format!("{}(\"{arg}\")", self.path),
            None => self.path.clone(),
        }
    }
}

fn normalize_path(p: &str) -> String {
    let trimmed = p.trim_start_matches("./").trim_end_matches('/');
    if trimmed.is_empty() { ".".into() } else { trimmed.to_string() }
}

/// `declared` autorise-t-il `used` ? Une restriction dynamique est vérifiée à l'exécution.
fn covers(declared: &Eff, used: &Eff) -> bool {
    let path_ok = declared.path == used.path || used.path.starts_with(&format!("{}.", declared.path));
    path_ok
        && match (&declared.arg, &used.arg) {
            (None, _) | (Some(_), None) => true,
            (Some(d), Some(u)) if used.path == "net" => d == u,
            (Some(d), Some(u)) => {
                let (d, u) = (normalize_path(d), normalize_path(u));
                d == "." || u == d || u.starts_with(&format!("{d}/"))
            }
        }
}

struct TypeDecl<'p> {
    def: &'p TypeDef,
    fields: Vec<&'p Field>,
    ivars: Vec<&'p Field>,
    methods: HashMap<&'p str, &'p FnDef>,
    statics: HashMap<&'p str, &'p FnDef>,
    variants: Vec<&'p Variant>,
    handlers: HashMap<&'p str, &'p Handler>,
    directives: Vec<&'p Directive>,
    includes: Vec<&'p str>,
}

/// Paramètre, champ ou variante : ce qu'un appel doit lier.
#[derive(Clone, Copy)]
struct Slot<'p> {
    name: &'p str,
    ty: Option<&'p Type>,
    optional: bool,
}

impl<'p> Slot<'p> {
    fn params(params: &'p [Param]) -> Vec<Slot<'p>> {
        params.iter().map(|p| Slot { name: &p.name.name, ty: p.ty.as_ref(), optional: p.default.is_some() }).collect()
    }

    fn fields(fields: &[&'p Field]) -> Vec<Slot<'p>> {
        fields
            .iter()
            .map(|f| Slot {
                name: &f.name.name,
                ty: f.ty.as_ref(),
                optional: f.default.is_some() || matches!(f.ty, Some(Type::Optional(..))),
            })
            .collect()
    }
}

#[derive(Clone, Copy)]
enum Kind<'p> {
    Top,
    Fn(&'p FnDef),
    Handler(&'p str, &'p Handler),
}

/// Contexte d'une fonction en cours de vérification.
struct Ctx<'p> {
    scopes: Vec<HashMap<String, V>>,
    self_ty: Option<Ty>,
    self_taint: Option<Span>,
    kind: Kind<'p>,
    effects: Vec<Eff>,
    returns: Vec<V>,
    run_span: Option<Span>,
}

impl<'p> Ctx<'p> {
    fn new(kind: Kind<'p>, self_ty: Option<Ty>, self_taint: Option<Span>) -> Self {
        Ctx {
            scopes: vec![HashMap::new()],
            self_ty,
            self_taint,
            kind,
            effects: Vec::new(),
            returns: Vec::new(),
            run_span: None,
        }
    }

    fn lookup(&self, name: &str) -> Option<V> {
        self.scopes.iter().rev().find_map(|s| s.get(name).cloned())
    }

    fn assign(&mut self, name: &str, value: V) {
        match self.scopes.iter_mut().rev().find(|s| s.contains_key(name)) {
            Some(scope) => {
                let merged = scope[name].taint.or(value.taint);
                scope.insert(name.to_string(), V { ty: value.ty, taint: merged });
            }
            None => self.define(name, value),
        }
    }

    fn define(&mut self, name: &str, value: V) {
        self.scopes.last_mut().expect("portée").insert(name.to_string(), value);
    }

    fn add_effect(&mut self, effect: Eff) {
        if !self.effects.iter().any(|e| e.path == effect.path && e.arg == effect.arg) {
            self.effects.push(effect);
        }
    }

    fn names(&self) -> Vec<String> {
        self.scopes.iter().flat_map(|s| s.keys().cloned()).collect()
    }
}

struct ArgV {
    name: Option<String>,
    v: V,
    span: Span,
    /// Littéral chaîne sans interpolation (restriction d'effet).
    lit: Option<String>,
}

type Key = (usize, Vec<bool>, bool);

struct Checker<'p> {
    program: &'p Program,
    fns: HashMap<&'p str, &'p FnDef>,
    types: HashMap<&'p str, TypeDecl<'p>>,
    variants: HashMap<&'p str, &'p str>,
    /// Message → agents qui le gèrent.
    messages: HashMap<&'p str, Vec<(&'p str, &'p Handler)>>,
    models: Vec<&'p str>,
    diags: Vec<Diagnostic>,
    seen: HashSet<(u32, u32, String)>,
    memo: HashMap<Key, (V, Vec<Eff>)>,
    in_progress: HashSet<Key>,
    prev_effects: HashMap<usize, Vec<Eff>>,
    /// État `@…` qui a reçu une valeur teintée : (type, nom) → origine.
    ivar_taint: HashMap<(String, String), Span>,
    /// Type de l'état `@…` sans annotation, inféré depuis sa valeur initiale.
    ivar_types: HashMap<(String, String), Ty>,
}

impl<'p> Checker<'p> {
    fn new(program: &'p Program) -> Self {
        Checker {
            program,
            fns: HashMap::new(),
            types: HashMap::new(),
            variants: HashMap::new(),
            messages: HashMap::new(),
            models: Vec::new(),
            diags: Vec::new(),
            seen: HashSet::new(),
            memo: HashMap::new(),
            in_progress: HashSet::new(),
            prev_effects: HashMap::new(),
            ivar_taint: HashMap::new(),
            ivar_types: HashMap::new(),
        }
    }

    fn effect_count(&self) -> usize {
        self.prev_effects.values().map(Vec::len).sum()
    }

    fn report(&mut self, diag: Diagnostic) {
        if self.seen.insert((diag.span.start, diag.span.end, diag.message.clone())) {
            self.diags.push(diag);
        }
    }

    fn error(&mut self, code: &'static str, span: Span, message: impl Into<String>) {
        self.report(Diagnostic::new(span, message).with_code(code));
    }

    fn error_help(&mut self, code: &'static str, span: Span, message: impl Into<String>, help: Option<String>) {
        let mut diag = Diagnostic::new(span, message).with_code(code);
        diag.help = help;
        self.report(diag);
    }

    // ── Déclarations ─────────────────────────────────────────

    fn collect(&mut self) {
        let program = self.program;
        for item in &program.items {
            match item {
                Item::Fn(def) => {
                    if self.fns.insert(&def.name.name, def).is_some() {
                        self.error(E_NAME, def.name.span, format!("fonction `{}` définie deux fois", def.name.name));
                    }
                }
                Item::Type(def) => {
                    let decl = TypeDecl::new(def);
                    for (message, handler) in &decl.handlers {
                        self.messages.entry(message).or_default().push((&def.name.name, handler));
                    }
                    if self.types.insert(&def.name.name, decl).is_some() {
                        self.error(E_NAME, def.name.span, format!("type `{}` défini deux fois", def.name.name));
                    }
                }
                Item::Model(model) => self.models.push(&model.name.name),
                Item::Stmt(_) => {}
            }
        }
        // `include Module`
        let includes: Vec<(&'p str, &'p str, Span)> = self
            .types
            .values()
            .flat_map(|t| {
                t.def.members.iter().filter_map(move |m| match m {
                    Member::Include(ty) => Some((t.def.name.name.as_str(), type_name(ty), ty.span())),
                    _ => None,
                })
            })
            .collect();
        for (owner, module, span) in includes {
            let Some(methods) =
                self.types.get(module).filter(|m| m.def.kind == TypeKind::Module).map(|m| m.methods.clone())
            else {
                self.error(E_NAME, span, format!("module `{module}` inconnu"));
                continue;
            };
            let decl = self.types.get_mut(owner).expect("type déclaré");
            decl.includes.push(module);
            for (name, def) in methods {
                decl.methods.entry(name).or_insert(def);
            }
        }
        for decl in self.types.values() {
            for variant in &decl.variants {
                self.variants.insert(&variant.name.name, &decl.def.name.name);
            }
        }
    }

    /// `@writers = spawn_pool(Writer)` : `@writers` est un `Writer`.
    fn infer_ivars(&mut self) {
        let owners: Vec<(&'p str, Vec<&'p Field>)> =
            self.types.iter().map(|(name, decl)| (*name, decl.ivars.clone())).collect();
        for (owner, ivars) in owners {
            let mut cx = Ctx::new(Kind::Top, Some(Ty::user(owner)), None);
            for field in ivars {
                if let (None, Some(default)) = (&field.ty, &field.default) {
                    let v = self.expr(&mut cx, default);
                    self.ivar_types.insert((owner.to_string(), field.name.name.clone()), v.ty);
                }
            }
        }
    }

    fn resolve(&mut self, ty: &'p Type) -> (Ty, bool) {
        match ty {
            Type::Tainted(inner, _) => (self.resolve(inner).0, true),
            Type::Optional(inner, _) => {
                let (inner, tainted) = self.resolve(inner);
                (Ty::opt(inner), tainted)
            }
            Type::Named { path, args, span } => {
                let name = path.last().expect("chemin non vide").name.as_str();
                let arg = |c: &mut Self, i: usize| args.get(i).map_or(Ty::Unknown, |a| c.resolve(a).0);
                let resolved = match name {
                    "Int" => Ty::Int,
                    "Float" => Ty::Float,
                    "String" | "Path" | "Email" | "Url" => Ty::Str,
                    "Symbol" => Ty::Sym,
                    "Bool" => Ty::Bool,
                    "Unit" | "Nil" => Ty::Nil,
                    "Money" => Ty::Money,
                    "Duration" => Ty::Duration,
                    "Range" => Ty::Range,
                    "Any" => Ty::Unknown,
                    "Array" => Ty::array(arg(self, 0)),
                    "Hash" => Ty::Hash(Box::new(arg(self, 0)), Box::new(arg(self, 1))),
                    "Result" => Ty::Result(Box::new(arg(self, 0)), Box::new(arg(self, 1))),
                    n if self.types.contains_key(n) || is_error_name(n) => Ty::user(n),
                    n => {
                        let known: Vec<&str> =
                            self.types.keys().copied().chain(builtins::TYPE_NAMES.iter().copied()).collect();
                        self.error_help(E_NAME, *span, format!("type inconnu `{n}`"), suggest(n, known));
                        Ty::Unknown
                    }
                };
                (resolved, false)
            }
        }
    }

    /// `actual` peut-il être passé là où `expected` est attendu ?
    fn compat(&self, actual: &Ty, expected: &Ty) -> bool {
        use Ty::*;
        match (actual, expected) {
            (Unknown, _) | (_, Unknown) => true,
            (a, b) if a == b => true,
            (Int, Float) | (Int | Float, Money) | (Sym, Str) => true,
            (Nil, Opt(_)) => true,
            (a, Opt(b)) => self.compat(a, b),
            // tolérant : `T?` accepté là où `T` est attendu (la phase 3 ajoutera la vérification de nil)
            (Opt(a), b) => self.compat(a, b),
            (Array(a), Array(b)) => self.compat(a, b),
            (Hash(k1, v1), Hash(k2, v2)) => self.compat(k1, k2) && self.compat(v1, v2),
            (Result(t1, e1), Result(t2, e2)) => self.compat(t1, t2) && self.compat(e1, e2),
            (User(a), User(b)) => {
                (is_error_name(a) && matches!(b.as_str(), "StandardError" | "Exception"))
                    || self.types.get(a.as_str()).is_some_and(|d| d.includes.contains(&b.as_str()))
            }
            (Range, Array(t)) => self.compat(&Int, t),
            _ => false,
        }
    }

    fn schema_ok(&self, ty: &Ty, depth: usize) -> Result<(), String> {
        if depth > 16 {
            return Err("type récursif : non représentable en JSON Schema".into());
        }
        match ty {
            Ty::Hash(..) => Err("`Hash` ne peut pas sortir d'un LLM : utilisez une `struct`".into()),
            Ty::Array(t) | Ty::Opt(t) => self.schema_ok(t, depth + 1),
            Ty::User(name) => {
                let Some(decl) = self.types.get(name.as_str()) else { return Ok(()) };
                let fields: Vec<&Field> = match decl.def.kind {
                    TypeKind::Struct => decl.fields.clone(),
                    TypeKind::Enum => decl.variants.iter().flat_map(|v| v.fields.iter()).collect(),
                    _ => return Err(format!("`{name}` ({:?}) ne peut pas sortir d'un LLM", decl.def.kind)),
                };
                for f in fields {
                    let Some(t) = &f.ty else { return Err(format!("le champ `{}` n'a pas de type", f.name.name)) };
                    self.schema_ok(&self.peek_ty(t), depth + 1)?;
                }
                Ok(())
            }
            Ty::Type(_) | Ty::Budget => Err(format!("{ty} ne peut pas sortir d'un LLM")),
            _ => Ok(()),
        }
    }

    /// Résolution sans diagnostic (pour les vérifications secondaires).
    fn peek_ty(&self, ty: &Type) -> Ty {
        match ty {
            Type::Tainted(inner, _) => self.peek_ty(inner),
            Type::Optional(inner, _) => Ty::opt(self.peek_ty(inner)),
            Type::Named { path, args, .. } => {
                let arg = |i: usize| args.get(i).map_or(Ty::Unknown, |a| self.peek_ty(a));
                match path.last().expect("chemin").name.as_str() {
                    "Int" => Ty::Int,
                    "Float" => Ty::Float,
                    "String" | "Path" | "Email" | "Url" => Ty::Str,
                    "Bool" => Ty::Bool,
                    "Array" => Ty::array(arg(0)),
                    "Hash" => Ty::Hash(Box::new(arg(0)), Box::new(arg(1))),
                    n if self.types.contains_key(n) => Ty::user(n),
                    _ => Ty::Unknown,
                }
            }
        }
    }

    // ── Passe complète ───────────────────────────────────────

    fn pass(&mut self) {
        let program = self.program;
        for item in &program.items {
            match item {
                Item::Fn(def) => {
                    self.check_fn_decl(def);
                    let taints = declared_taints(&def.params);
                    self.check_fn(def, None, taints, None);
                }
                Item::Type(def) => self.check_type(def),
                Item::Model(model) => self.check_model(model),
                Item::Stmt(_) => {}
            }
        }
        let mut cx = Ctx::new(Kind::Top, None, None);
        for item in &program.items {
            if let Item::Stmt(e) = item {
                self.expr(&mut cx, e);
            }
        }
    }

    fn check_model(&mut self, model: &'p grenat_ast::ModelDecl) {
        for option in &model.options {
            let Arg::Named { name, value: Some(value) } = option else { continue };
            match (name.name.as_str(), &value.kind) {
                ("provider", ExprKind::Symbol(p)) if p != "anthropic" => self.error(
                    E_DECL,
                    value.span,
                    format!("fournisseur `:{p}` non pris en charge (disponible : `:anthropic`)"),
                ),
                ("provider" | "name" | "temperature" | "max_tokens" | "effort" | "fallbacks", _) => {}
                (other, _) => self.error_help(
                    E_DECL,
                    name.span,
                    format!("option de modèle inconnue `{other}:`"),
                    suggest(other, ["provider", "name", "temperature", "max_tokens", "effort", "fallbacks"]),
                ),
            }
        }
    }

    fn check_fn_decl(&mut self, def: &'p FnDef) {
        for effect in &def.effects {
            let path: Vec<&str> = effect.path.iter().map(|i| i.name.as_str()).collect();
            let path = path.join(".");
            if !KNOWN_EFFECTS.contains(&path.as_str()) {
                self.error_help(
                    E_DECL,
                    effect.span,
                    format!("effet inconnu `{path}`"),
                    suggest(&path, KNOWN_EFFECTS.iter().copied()),
                );
            }
        }
        if def.kind == FnKind::Tool {
            for param in &def.params {
                match &param.ty {
                    None => self.error(
                        E_DECL,
                        param.span,
                        format!("le paramètre `{}` d'un outil doit être typé", param.name.name),
                    ),
                    Some(t) => {
                        let ty = self.resolve(t).0;
                        if let Err(e) = self.schema_ok(&ty, 0) {
                            self.error(E_DECL, t.span(), e);
                        }
                    }
                }
            }
        }
        if def.kind == FnKind::Prompt {
            if let Some(ret) = &def.ret {
                if !is_tainted_decl(ret) {
                    self.report(
                        Diagnostic::new(
                            ret.span(),
                            "le résultat d'un `prompt` vient d'un LLM : son type doit être teinté",
                        )
                        .with_code(E_TAINT_DECL)
                        .with_help(format!("écrivez `-> ~{}`", type_name(ret))),
                    );
                }
                let ty = self.resolve(ret).0;
                if let Err(e) = self.schema_ok(&ty, 0) {
                    self.error(E_DECL, ret.span(), e);
                }
            }
            self.check_model_ref(def.model.as_ref(), def.name.span);
        }
    }

    fn check_model_ref(&mut self, selector: Option<&'p Expr>, span: Span) {
        match selector.map(|e| (&e.kind, e.span)) {
            Some((ExprKind::Symbol(name), span)) if !self.models.contains(&name.as_str()) => {
                let models = self.models.clone();
                self.error_help(E_NAME, span, format!("modèle `:{name}` non déclaré"), suggest(name, models));
            }
            None if self.models.is_empty() => self.report(
                Diagnostic::new(span, "aucun modèle déclaré")
                    .with_code(E_DECL)
                    .with_help("ajoutez `model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"`"),
            ),
            _ => {}
        }
    }

    fn check_type(&mut self, def: &'p TypeDef) {
        let name: &'p str = &def.name.name;
        let decl = &self.types[name];
        let (fields, ivars, variants) = (decl.fields.clone(), decl.ivars.clone(), decl.variants.clone());
        let methods: Vec<&'p FnDef> = def
            .members
            .iter()
            .filter_map(|m| match m {
                Member::Method(f) => Some(f),
                _ => None,
            })
            .collect();
        let handlers: Vec<&'p Handler> = decl.handlers.values().copied().collect();
        let directives = decl.directives.clone();

        for f in fields.iter().chain(variants.iter().flat_map(|v| v.fields.iter()).collect::<Vec<_>>().iter()) {
            if let Some(t) = &f.ty {
                self.resolve(t);
            }
        }
        let self_ty = Ty::user(name);
        let mut cx = Ctx::new(Kind::Top, Some(self_ty.clone()), None);
        for f in fields.iter().chain(ivars.iter()) {
            let declared = f.ty.as_ref().map(|t| self.resolve(t).0);
            if let Some(default) = &f.default {
                let v = self.expr(&mut cx, default);
                if let Some(expected) = &declared
                    && !self.compat(&v.ty, expected)
                {
                    self.error(E_TYPE, default.span, format!("`{}` attend `{expected}`, reçu `{}`", f.name.name, v.ty));
                }
            }
        }
        for def in methods {
            self.check_fn_decl(def);
            let self_ty = if def.on_self { Ty::Type(name.into()) } else { self_ty.clone() };
            self.check_fn(def, Some(self_ty), declared_taints(&def.params), None);
        }
        match def.kind {
            TypeKind::Agent => {
                self.check_agent_directives(name, &directives);
                for handler in handlers {
                    self.check_handler(name, handler, declared_taints(&handler.params));
                }
            }
            TypeKind::Supervisor => {
                for option in &def.options {
                    if let Arg::Named { value: Some(v), .. } | Arg::Pos(v) = option {
                        self.expr(&mut cx, v);
                    }
                }
                for d in directives {
                    match (d.name.name.as_str(), d.args.first()) {
                        ("child", Some(Arg::Pos(Expr { kind: ExprKind::Const(path), span }))) => {
                            let child = path.last().expect("chemin").name.as_str();
                            if !self.types.get(child).is_some_and(|t| t.def.kind == TypeKind::Agent) {
                                self.error(E_NAME, *span, format!("agent inconnu `{child}`"));
                            }
                        }
                        ("child", _) => self.error(E_DECL, d.span, "`child` attend un agent : `child Writer`"),
                        (other, _) => {
                            self.error(E_DECL, d.name.span, format!("directive de superviseur inconnue `{other}`"))
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn check_agent_directives(&mut self, agent: &'p str, directives: &[&'p Directive]) {
        let mut cx = Ctx::new(Kind::Top, Some(Ty::user(agent)), None);
        for d in directives {
            let first = d.args.iter().find_map(|a| match a {
                Arg::Pos(e) => Some(e),
                _ => None,
            });
            match d.name.name.as_str() {
                "model" => self.check_model_ref(first, d.span),
                "tools" => {
                    for arg in &d.args {
                        let Arg::Pos(Expr { kind: ExprKind::Var(tool), span }) = arg else {
                            self.error(E_DECL, d.span, "`tools` attend des noms d'outils : `tools lire, chercher`");
                            continue;
                        };
                        match self.fns.get(tool.as_str()) {
                            Some(def) if def.kind == FnKind::Tool => {}
                            Some(_) => self.report(
                                Diagnostic::new(*span, format!("`{tool}` n'est pas un `tool`"))
                                    .with_code(E_DECL)
                                    .with_help("déclarez-le avec `tool` : seuls les outils sont confiés à un LLM"),
                            ),
                            None => {
                                let tools: Vec<&str> =
                                    self.fns.iter().filter(|(_, f)| f.kind == FnKind::Tool).map(|(n, _)| *n).collect();
                                self.error_help(E_NAME, *span, format!("outil inconnu `{tool}`"), suggest(tool, tools));
                            }
                        }
                    }
                }
                "max_turns" => {
                    if let Some(e) = first {
                        let v = self.expr(&mut cx, e);
                        if !self.compat(&v.ty, &Ty::Int) {
                            self.error(E_TYPE, e.span, format!("`max_turns` attend un `Int`, reçu `{}`", v.ty));
                        }
                    }
                }
                "instructions" => {
                    if let Some(e) = first {
                        self.expr(&mut cx, e);
                    }
                }
                "budget" => {
                    let args = self.args(&mut cx, &d.args);
                    self.check_budget_options(&args);
                }
                other => self.error_help(
                    E_DECL,
                    d.name.span,
                    format!("directive d'agent inconnue `{other}`"),
                    suggest(other, ["model", "tools", "max_turns", "instructions", "budget"]),
                ),
            }
        }
    }

    fn check_budget_options(&mut self, args: &[ArgV]) {
        for a in args {
            let expected = match a.name.as_deref() {
                Some("usd") => Ty::Money,
                Some("tokens") => Ty::Int,
                Some("time") => Ty::Duration,
                Some(other) => {
                    self.error_help(
                        E_TYPE,
                        a.span,
                        format!("option de budget inconnue `{other}:`"),
                        suggest(other, ["usd", "tokens", "time"]),
                    );
                    continue;
                }
                None => {
                    self.error(
                        E_TYPE,
                        a.span,
                        "`budget` n'accepte que des options nommées : `usd:`, `tokens:`, `time:`",
                    );
                    continue;
                }
            };
            let ok = self.compat(&a.v.ty, &expected) || (expected == Ty::Duration && self.compat(&a.v.ty, &Ty::Int));
            if !ok {
                self.error(
                    E_TYPE,
                    a.span,
                    format!("`{}:` attend `{expected}`, reçu `{}`", a.name.as_deref().unwrap_or(""), a.v.ty),
                );
            }
        }
    }

    // ── Fonctions et handlers ────────────────────────────────

    /// Vérifie `def` pour une teinte donnée de ses arguments (analyse sensible au contexte).
    fn check_fn(
        &mut self,
        def: &'p FnDef,
        self_ty: Option<Ty>,
        taints: Vec<Option<Span>>,
        self_taint: Option<Span>,
    ) -> (V, Vec<Eff>) {
        let key: Key =
            (def as *const FnDef as usize, taints.iter().map(Option::is_some).collect(), self_taint.is_some());
        if let Some(result) = self.memo.get(&key) {
            return result.clone();
        }
        let declared_ret = def.ret.as_ref().map(|t| self.resolve(t));
        if self.in_progress.contains(&key) {
            let ty = declared_ret.map_or(Ty::Unknown, |r| r.0);
            let taint = taints.iter().flatten().next().copied().or(self_taint);
            return (V { ty, taint }, self.prev_effects.get(&key.0).cloned().unwrap_or_default());
        }
        self.in_progress.insert(key.clone());

        let mut cx = Ctx::new(Kind::Fn(def), self_ty, self_taint);
        self.bind_params(&mut cx, &def.params, &taints);
        let body = self.body(&mut cx, &def.body);
        let mut ret = cx.returns.iter().fold(body, |acc, r| join_v(&acc, r));

        match (&declared_ret, def.kind) {
            (_, FnKind::Prompt) => {
                ret = V { ty: declared_ret.map_or(Ty::Str, |r| r.0), taint: Some(def.span) };
            }
            (Some((ty, tainted)), _) => {
                let checkable = !def.is_abstract && *ty != Ty::Nil;
                if checkable && !self.compat(&ret.ty, ty) {
                    let span = def.body.stmts.last().map_or(def.span, |e| e.span);
                    self.error(E_TYPE, span, format!("`{}` doit renvoyer `{ty}`, renvoie `{}`", def.name.name, ret.ty));
                }
                ret.ty = ty.clone();
                if *tainted {
                    ret.taint = ret.taint.or(Some(def.ret.as_ref().expect("déclaré").span()));
                }
            }
            (None, _) => {}
        }

        let effects = self.finish_effects(def, &cx);
        self.in_progress.remove(&key);
        self.prev_effects.insert(key.0, effects.clone());
        self.memo.insert(key, (ret.clone(), effects.clone()));
        (ret, effects)
    }

    fn bind_params(&mut self, cx: &mut Ctx<'p>, params: &'p [Param], taints: &[Option<Span>]) {
        for (i, p) in params.iter().enumerate() {
            let mut v = match &p.ty {
                Some(t) => {
                    let (ty, tainted) = self.resolve(t);
                    V { ty, taint: tainted.then_some(p.span) }
                }
                None => V::unknown(),
            };
            if let Some(default) = &p.default {
                let d = self.expr(cx, default);
                if p.ty.is_none() {
                    v.ty = d.ty;
                }
            }
            v.taint = v.taint.or(taints.get(i).copied().flatten());
            cx.define(&p.name.name, v);
        }
    }

    /// Effets exportés : les déclarés (vérifiés), sinon les inférés.
    fn finish_effects(&mut self, def: &'p FnDef, cx: &Ctx<'p>) -> Vec<Eff> {
        let mut inferred = cx.effects.clone();
        if def.kind == FnKind::Prompt {
            inferred.push(Eff { path: "llm".into(), arg: None, origin: def.span });
        }
        let is_main = def.name.name == "main" && self.fns.get("main").is_some_and(|m| std::ptr::eq(*m, def));
        if def.effects.is_empty() {
            if (def.kind == FnKind::Tool || is_main) && !inferred.is_empty() {
                let what = if is_main { "`main` est la racine des capacités" } else { "un outil" };
                let list: Vec<String> =
                    inferred.iter().map(|e| e.path.clone()).collect::<HashSet<_>>().into_iter().collect();
                let mut list = list;
                list.sort();
                self.report(
                    Diagnostic::new(
                        inferred[0].origin,
                        format!("`{}` utilise l'effet `{}` sans le déclarer", def.name.name, inferred[0].label()),
                    )
                    .with_code(E_EFFECT)
                    .with_note(def.name.span, format!("{what} : ses effets doivent être déclarés"))
                    .with_help(format!("ajoutez `uses {}`", list.join(", "))),
                );
            }
            return inferred;
        }
        let declared: Vec<Eff> = def
            .effects
            .iter()
            .map(|e| Eff {
                path: e.path.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join("."),
                arg: e.args.first().and_then(literal_string),
                origin: e.span,
            })
            .collect();
        for used in &inferred {
            if !declared.iter().any(|d| covers(d, used)) {
                let list: Vec<String> = declared.iter().map(Eff::label).collect();
                self.report(
                    Diagnostic::new(
                        used.origin,
                        format!("`{}` utilise l'effet `{}` sans le déclarer", def.name.name, used.label()),
                    )
                    .with_code(E_EFFECT)
                    .with_note(def.name.span, format!("`{}` déclare : uses {}", def.name.name, list.join(", ")))
                    .with_help(format!("ajoutez `{}` à `uses`", used.label())),
                );
            }
        }
        declared
    }

    fn check_handler(&mut self, agent: &'p str, handler: &'p Handler, taints: Vec<Option<Span>>) -> (V, Vec<Eff>) {
        let key: Key = (handler as *const Handler as usize, taints.iter().map(Option::is_some).collect(), false);
        if let Some(result) = self.memo.get(&key) {
            return result.clone();
        }
        let declared_ret = handler.ret.as_ref().map(|t| self.resolve(t));
        if self.in_progress.contains(&key) {
            let ty = declared_ret.map_or(Ty::Unknown, |r| r.0);
            return (
                V { ty, taint: taints.iter().flatten().next().copied() },
                self.prev_effects.get(&key.0).cloned().unwrap_or_default(),
            );
        }
        self.in_progress.insert(key.clone());

        let mut cx = Ctx::new(Kind::Handler(agent, handler), Some(Ty::user(agent)), None);
        self.bind_params(&mut cx, &handler.params, &taints);
        let body = self.body(&mut cx, &handler.body);
        let mut ret = cx.returns.iter().fold(body, |acc, r| join_v(&acc, r));
        if let Some((ty, tainted)) = &declared_ret {
            if cx.run_span.is_none() && *ty != Ty::Nil && !self.compat(&ret.ty, ty) {
                let span = handler.body.stmts.last().map_or(handler.span, |e| e.span);
                self.error(
                    E_TYPE,
                    span,
                    format!("`on {}` doit renvoyer `{ty}`, renvoie `{}`", handler.message.name, ret.ty),
                );
            }
            ret.ty = ty.clone();
            if *tainted {
                ret.taint = ret.taint.or(cx.run_span).or(Some(handler.span));
            }
        }
        let effects = cx.effects.clone();
        self.in_progress.remove(&key);
        self.prev_effects.insert(key.0, effects.clone());
        self.memo.insert(key, (ret.clone(), effects.clone()));
        (ret, effects)
    }

    // ── Corps ────────────────────────────────────────────────

    fn stmts(&mut self, cx: &mut Ctx<'p>, stmts: &'p [Expr]) -> V {
        let mut last = V::new(Ty::Nil);
        for s in stmts {
            last = self.expr(cx, s);
        }
        last
    }

    fn body(&mut self, cx: &mut Ctx<'p>, body: &'p Body) -> V {
        let mut result = self.stmts(cx, &body.stmts);
        for rescue in &body.rescues {
            if let Some(binding) = &rescue.binding {
                let ty = match rescue.types.as_slice() {
                    [single] => Ty::user(type_name(single)),
                    _ => Ty::user("StandardError"),
                };
                cx.define(&binding.name, V::new(ty));
            }
            for t in &rescue.types {
                let name = type_name(t);
                if !is_error_name(name) && !self.types.contains_key(name) {
                    self.error(E_NAME, t.span(), format!("type d'erreur inconnu `{name}`"));
                }
            }
            let r = self.stmts(cx, &rescue.body);
            result = join_v(&result, &r);
        }
        if let Some(ensure) = &body.ensure {
            self.stmts(cx, ensure);
        }
        result
    }

    fn block(&mut self, cx: &mut Ctx<'p>, block: &'p Block, params: &[V]) -> V {
        cx.scopes.push(HashMap::new());
        let destructure = params.len() == 1 && block.params.len() > 1;
        for (i, p) in block.params.iter().enumerate() {
            let v = if destructure {
                match &params[0].ty {
                    Ty::Array(t) => V { ty: (**t).clone(), taint: params[0].taint },
                    _ => V { ty: Ty::Unknown, taint: params[0].taint },
                }
            } else {
                params.get(i).cloned().unwrap_or_else(V::unknown)
            };
            let v = match &p.ty {
                Some(t) => V { ty: self.resolve(t).0, taint: v.taint },
                None => v,
            };
            cx.define(&p.name.name, v);
        }
        let result = self.body(cx, &block.body);
        cx.scopes.pop();
        result
    }

    // ── Expressions ──────────────────────────────────────────

    fn expr(&mut self, cx: &mut Ctx<'p>, e: &'p Expr) -> V {
        match &e.kind {
            ExprKind::Int(_) => V::new(Ty::Int),
            ExprKind::Float(_) => V::new(Ty::Float),
            ExprKind::Bool(_) => V::new(Ty::Bool),
            ExprKind::Nil => V::new(Ty::Nil),
            ExprKind::Symbol(_) => V::new(Ty::Sym),
            ExprKind::Str(segs) => {
                let mut v = V::new(Ty::Str);
                for seg in segs {
                    if let StrSeg::Interp(e) = seg {
                        let part = self.expr(cx, e);
                        v.taint = v.taint.or(part.taint);
                    }
                }
                v
            }
            ExprKind::SelfRef => match &cx.self_ty {
                Some(ty) => V { ty: ty.clone(), taint: cx.self_taint },
                None => {
                    self.error(E_NAME, e.span, "`self` utilisé hors d'une méthode");
                    V::unknown()
                }
            },
            ExprKind::Array(items) => {
                let mut elem: Option<Ty> = None;
                let mut taint = None;
                for item in items {
                    let v = self.expr(cx, item);
                    taint = taint.or(v.taint);
                    elem = Some(match elem {
                        None => v.ty,
                        Some(t) => join(&t, &v.ty),
                    });
                }
                V { ty: Ty::array(elem.unwrap_or(Ty::Unknown)), taint }
            }
            ExprKind::Hash(entries) => {
                let (mut k, mut val, mut taint) = (None::<Ty>, None::<Ty>, None);
                for (key, value) in entries {
                    let (kv, vv) = (self.expr(cx, key), self.expr(cx, value));
                    taint = taint.or(kv.taint).or(vv.taint);
                    k = Some(k.map_or(kv.ty.clone(), |t| join(&t, &kv.ty)));
                    val = Some(val.map_or(vv.ty.clone(), |t| join(&t, &vv.ty)));
                }
                V { ty: Ty::Hash(Box::new(k.unwrap_or(Ty::Unknown)), Box::new(val.unwrap_or(Ty::Unknown))), taint }
            }
            ExprKind::Range { lo, hi, .. } => {
                for bound in [lo, hi] {
                    let v = self.expr(cx, bound);
                    if !self.compat(&v.ty, &Ty::Int) {
                        self.error(
                            E_TYPE,
                            bound.span,
                            format!("les bornes d'un intervalle sont des `Int`, reçu `{}`", v.ty),
                        );
                    }
                }
                V::new(Ty::Range)
            }
            ExprKind::Var(name) => self.var(cx, name, e.span),
            ExprKind::It => cx.lookup("it").unwrap_or_else(V::unknown),
            ExprKind::Const(path) => self.constant(path, e.span),
            ExprKind::IVar(name) => self.ivar(cx, name, e.span),
            ExprKind::Call { recv, name, args, block, safe, .. } => {
                self.call(cx, e.span, recv.as_deref(), name, args, block.as_deref(), *safe)
            }
            ExprKind::Index { recv, args } => {
                let target = self.expr(cx, recv);
                let index: Vec<V> = args.iter().map(|a| self.expr(cx, a)).collect();
                self.index(target, &index, e.span)
            }
            ExprKind::Unary { op, expr } => {
                let v = self.expr(cx, expr);
                match op {
                    UnOp::Not => V { ty: Ty::Bool, taint: v.taint },
                    UnOp::Neg => {
                        if !matches!(v.ty, Ty::Int | Ty::Float | Ty::Money | Ty::Unknown) {
                            self.error(E_TYPE, e.span, format!("`-` non défini pour `{}`", v.ty));
                        }
                        v
                    }
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let (l, r) = (self.expr(cx, lhs), self.expr(cx, rhs));
                self.binary(cx, *op, l, r, e.span, lhs)
            }
            ExprKind::Try(inner) => {
                let v = self.expr(cx, inner);
                let ty = match v.ty {
                    Ty::Result(t, _) => *t,
                    Ty::Opt(t) => *t,
                    other => other,
                };
                V { ty, taint: v.taint }
            }
            ExprKind::Assign { target, value } => {
                let v = self.expr(cx, value);
                self.assign(cx, target, v.clone());
                v
            }
            ExprKind::OpAssign { op, target, value } => {
                let current = match &target.kind {
                    ExprKind::Var(name) => cx.lookup(name).unwrap_or_else(|| V::new(Ty::Nil)),
                    _ => self.expr(cx, target),
                };
                let rhs = self.expr(cx, value);
                let v = match op {
                    BinOp::Or | BinOp::And => join_v(&current, &rhs),
                    op => self.binary(cx, *op, current, rhs, value.span, target),
                };
                self.assign(cx, target, v.clone());
                v
            }
            ExprKind::MultiAssign { targets, value } => {
                let v = self.expr(cx, value);
                let item = match &v.ty {
                    Ty::Array(t) => (**t).clone(),
                    _ => Ty::Unknown,
                };
                for t in targets {
                    self.assign(cx, t, V { ty: item.clone(), taint: v.taint });
                }
                v
            }
            ExprKind::If { cond, then, else_ } => {
                self.expr(cx, cond);
                let a = self.stmts(cx, then);
                let b = match else_ {
                    Some(else_) => self.stmts(cx, else_),
                    None => V::new(Ty::Nil),
                };
                join_v(&a, &b)
            }
            ExprKind::While { cond, body } => {
                self.expr(cx, cond);
                self.stmts(cx, body);
                V::new(Ty::Nil)
            }
            ExprKind::Case { subject, arms, else_ } => {
                let subject = subject.as_ref().map(|s| self.expr(cx, s));
                let mut result: Option<V> = None;
                for arm in arms {
                    match &arm.test {
                        ArmTest::In(pattern) => {
                            let s = subject.clone().unwrap_or_else(V::unknown);
                            self.pattern(cx, pattern, &s);
                        }
                        ArmTest::When(values) => {
                            for v in values {
                                self.expr(cx, v);
                            }
                        }
                    }
                    if let Some(guard) = &arm.guard {
                        self.expr(cx, guard);
                    }
                    let v = self.stmts(cx, &arm.body);
                    result = Some(result.map_or(v.clone(), |r| join_v(&r, &v)));
                }
                if let Some(else_) = else_ {
                    let v = self.stmts(cx, else_);
                    result = Some(result.map_or(v.clone(), |r| join_v(&r, &v)));
                }
                result.unwrap_or_else(|| V::new(Ty::Nil))
            }
            ExprKind::Begin(body) => self.body(cx, body),
            ExprKind::Return(value) => {
                let v = value.as_ref().map_or(V::new(Ty::Nil), |v| self.expr(cx, v));
                cx.returns.push(v);
                V::unknown()
            }
            ExprKind::Break(value) | ExprKind::Next(value) => {
                if let Some(v) = value {
                    self.expr(cx, v);
                }
                V::unknown()
            }
        }
    }

    fn var(&mut self, cx: &mut Ctx<'p>, name: &str, span: Span) -> V {
        if let Some(v) = cx.lookup(name) {
            return v;
        }
        if let Some(self_ty) = cx.self_ty.clone() {
            if let Some(field) = self.field_of(&self_ty, name) {
                return V { ty: field, taint: cx.self_taint };
            }
            if let Some(def) = self.method_def(&self_ty, name) {
                let recv = V { ty: self_ty, taint: cx.self_taint };
                return self.user_method(cx, span, recv, def, Vec::new(), None);
            }
        }
        if let Some(def) = self.fns.get(name).copied() {
            return self.fn_call(cx, span, def, None, Vec::new(), None);
        }
        match name {
            "budget" => return V::new(Ty::Budget),
            "deny_all" | "approve_all" => return V::new(Ty::Sym),
            "puts" | "print" | "p" | "warn" => return V::new(Ty::Nil),
            _ => {}
        }
        if let Some(base) = name.strip_suffix('?')
            && let Some(v) = cx.lookup(base)
        {
            // `r?` : opérateur `?` sur une variable
            let ty = match v.ty {
                Ty::Result(t, _) | Ty::Opt(t) => *t,
                other => other,
            };
            return V { ty, taint: v.taint };
        }
        let mut candidates = cx.names();
        candidates.extend(self.fns.keys().map(|s| s.to_string()));
        self.error_help(
            E_NAME,
            span,
            format!("variable ou fonction inconnue `{name}`"),
            suggest(name, candidates.iter().map(String::as_str)),
        );
        V::unknown()
    }

    fn constant(&mut self, path: &'p [Ident], span: Span) -> V {
        let name = path.last().expect("chemin").name.as_str();
        if path.len() >= 2 {
            let owner = path[path.len() - 2].name.as_str();
            if self.variants.get(name) == Some(&owner) {
                return self.variant_value(name);
            }
        }
        if self.types.contains_key(name)
            || builtins::MODULES.contains(&name)
            || builtins::TYPE_NAMES.contains(&name)
            || is_error_name(name)
            || self.messages.contains_key(name)
        {
            return V::new(Ty::Type(name.into()));
        }
        if self.variants.contains_key(name) {
            return self.variant_value(name);
        }
        let known: Vec<&str> =
            self.types.keys().chain(self.variants.keys()).copied().chain(builtins::MODULES.iter().copied()).collect();
        self.error_help(E_NAME, span, format!("constante inconnue `{name}`"), suggest(name, known));
        V::unknown()
    }

    fn variant_value(&self, name: &str) -> V {
        let enum_name = self.variants[name];
        let has_fields = self.types[enum_name].variants.iter().any(|v| v.name.name == name && !v.fields.is_empty());
        V::new(if has_fields { Ty::Type(name.into()) } else { Ty::user(enum_name) })
    }

    fn ivar(&mut self, cx: &Ctx<'p>, name: &str, span: Span) -> V {
        let Some(Ty::User(owner)) = &cx.self_ty else {
            self.error(E_NAME, span, format!("`@{name}` utilisé hors d'une classe ou d'un agent"));
            return V::unknown();
        };
        let Some(field) =
            self.types.get(owner.as_str()).and_then(|t| t.ivars.iter().find(|f| f.name.name == name).copied())
        else {
            let known: Vec<&str> = self
                .types
                .get(owner.as_str())
                .map(|t| t.ivars.iter().map(|f| f.name.name.as_str()).collect())
                .unwrap_or_default();
            self.error_help(
                E_NAME,
                span,
                format!("état `@{name}` non déclaré dans `{owner}`"),
                suggest(name, known).or_else(|| Some(format!("déclarez-le : `@{name}: Type = valeur`"))),
            );
            return V::unknown();
        };
        let ty = match &field.ty {
            Some(t) => self.peek_ty(t),
            None => self.ivar_types.get(&(owner.clone(), name.to_string())).cloned().unwrap_or(Ty::Unknown),
        };
        let taint = self.ivar_taint.get(&(owner.clone(), name.to_string())).copied();
        V { ty, taint }
    }

    fn assign(&mut self, cx: &mut Ctx<'p>, target: &'p Expr, value: V) {
        match &target.kind {
            ExprKind::Var(name) => cx.assign(name, value),
            ExprKind::IVar(name) => {
                let current = self.ivar(cx, name, target.span);
                if !self.compat(&value.ty, &current.ty) {
                    self.error(
                        E_TYPE,
                        target.span,
                        format!("`@{name}` est de type `{}`, reçu `{}`", current.ty, value.ty),
                    );
                }
                if let (Some(origin), Some(Ty::User(owner))) = (value.taint, &cx.self_ty) {
                    self.ivar_taint.entry((owner.clone(), name.clone())).or_insert(origin);
                }
            }
            ExprKind::Index { recv, args } => {
                self.expr(cx, recv);
                for a in args {
                    self.expr(cx, a);
                }
                self.taint_container(cx, recv, value.taint);
            }
            ExprKind::Call { recv: Some(recv), name, .. } => {
                let r = self.expr(cx, recv);
                if let Ty::User(t) = &r.ty
                    && self.types.get(t.as_str()).is_some_and(|d| d.def.kind == TypeKind::Struct)
                {
                    self.report(
                        Diagnostic::new(target.span, format!("`{t}` est une struct immuable"))
                            .with_code(E_TYPE)
                            .with_help(format!("créez une copie : `.with({}: …)`", name.name)),
                    );
                }
            }
            _ => {}
        }
    }

    /// `xs << v`, `h[k] = v` : le conteneur devient teinté si `v` l'est.
    fn taint_container(&mut self, cx: &mut Ctx<'p>, container: &'p Expr, taint: Option<Span>) {
        let Some(origin) = taint else { return };
        match &container.kind {
            ExprKind::Var(name) => {
                if let Some(v) = cx.lookup(name) {
                    cx.assign(name, v.tainted(Some(origin)));
                }
            }
            ExprKind::IVar(name) => {
                if let Some(Ty::User(owner)) = &cx.self_ty {
                    self.ivar_taint.entry((owner.clone(), name.clone())).or_insert(origin);
                }
            }
            _ => {}
        }
    }

    fn binary(&mut self, cx: &mut Ctx<'p>, op: BinOp, l: V, r: V, span: Span, lhs: &'p Expr) -> V {
        use Ty::*;
        let taint = l.taint.or(r.taint);
        let (lt, rt) = (l.ty.base().clone(), r.ty.base().clone());
        let ty = match op {
            BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Match => Bool,
            BinOp::Cmp => Int,
            BinOp::And | BinOp::Or => join(&l.ty, &r.ty),
            BinOp::Shl if matches!(lt, Array(_)) => {
                self.taint_container(cx, lhs, r.taint);
                l.ty.clone()
            }
            _ if lt.is_unknown() || rt.is_unknown() => Unknown,
            BinOp::Add if lt == Str && rt == Str => Str,
            BinOp::Add if lt == Str => {
                self.report(
                    Diagnostic::new(span, format!("impossible d'ajouter `{rt}` à une chaîne"))
                        .with_code(E_TYPE)
                        .with_help("utilisez l'interpolation : \"…#{valeur}\""),
                );
                Str
            }
            BinOp::Mul if lt == Str && rt == Int => Str,
            BinOp::Add | BinOp::Sub if matches!(lt, Array(_)) && matches!(rt, Array(_)) => join(&lt, &rt),
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem | BinOp::Pow if lt == Int && rt == Int => {
                Int
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem | BinOp::Pow
                if lt.is_numeric() && rt.is_numeric() =>
            {
                Float
            }
            BinOp::Add | BinOp::Sub if lt == Money && rt == Money => Money,
            BinOp::Add if lt == Duration && rt == Duration => Duration,
            BinOp::Mul if lt == Duration && rt == Int => Duration,
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr if lt == Int && rt == Int => Int,
            _ => {
                self.error(E_TYPE, span, format!("opérateur `{}` non défini entre `{lt}` et `{rt}`", op_str(op)));
                Unknown
            }
        };
        V { ty, taint }
    }

    fn index(&mut self, target: V, index: &[V], span: Span) -> V {
        let key = index.first().map_or(Ty::Unknown, |v| v.ty.clone());
        let ty = match (target.ty.base(), &key) {
            (Ty::Array(t), Ty::Range) => Ty::Array(t.clone()),
            (Ty::Array(t), _) => (**t).clone(),
            (Ty::Hash(_, v), _) => (**v).clone(),
            (Ty::Str, _) => Ty::Str,
            (Ty::Type(sup), Ty::Type(agent)) => {
                let declared = self.types.get(sup.as_str()).is_some_and(|t| {
                    t.def.kind == TypeKind::Supervisor
                        && t.directives.iter().any(|d| {
                            d.name.name == "child"
                                && matches!(d.args.first(), Some(Arg::Pos(Expr { kind: ExprKind::Const(p), .. }))
                                    if p.last().is_some_and(|i| &i.name == agent))
                        })
                });
                if !declared {
                    self.error(E_NAME, span, format!("`{agent}` n'est pas un enfant du superviseur `{sup}`"));
                }
                Ty::user(agent)
            }
            (Ty::Unknown, _) => Ty::Unknown,
            (other, _) => {
                self.error(E_TYPE, span, format!("`{other}` ne peut pas être indexé"));
                Ty::Unknown
            }
        };
        V { ty, taint: target.taint }
    }

    fn pattern(&mut self, cx: &mut Ctx<'p>, pattern: &'p Pattern, subject: &V) {
        match &pattern.kind {
            PatternKind::Wildcard => {}
            PatternKind::Bind(name) => cx.define(name, subject.clone()),
            PatternKind::Lit(e) => {
                self.expr(cx, e);
            }
            PatternKind::Const { path, fields } => {
                let name = path.last().expect("chemin").name.as_str();
                let slots: Option<Vec<Slot<'p>>> = if let Some(enum_name) = self.variants.get(name).copied() {
                    if let Ty::User(subject_enum) = subject.ty.base()
                        && self.types.get(subject_enum.as_str()).is_some_and(|t| t.def.kind == TypeKind::Enum)
                        && subject_enum != enum_name
                    {
                        self.error(
                            E_TYPE,
                            pattern.span,
                            format!("`{name}` est une variante de `{enum_name}`, pas de `{subject_enum}`"),
                        );
                    }
                    let variant =
                        self.types[enum_name].variants.iter().find(|v| v.name.name == name).copied().expect("variante");
                    Some(Slot::fields(&variant.fields.iter().collect::<Vec<_>>()))
                } else if let Some(decl) = self.types.get(name) {
                    Some(Slot::fields(&decl.fields))
                } else if is_error_name(name) || builtins::TYPE_NAMES.contains(&name) {
                    None
                } else {
                    let known: Vec<&str> = self.variants.keys().chain(self.types.keys()).copied().collect();
                    self.error_help(E_NAME, pattern.span, format!("motif inconnu `{name}`"), suggest(name, known));
                    None
                };
                let Some(fields) = fields else { return };
                for (i, pf) in fields.iter().enumerate() {
                    let slot = match (&pf.name, &slots) {
                        (Some(n), Some(slots)) => {
                            let found = slots.iter().find(|s| s.name == n.name).copied();
                            if found.is_none() {
                                self.error_help(
                                    E_NAME,
                                    n.span,
                                    format!("`{name}` n'a pas de champ `{}`", n.name),
                                    suggest(&n.name, slots.iter().map(|s| s.name)),
                                );
                            }
                            found
                        }
                        (None, Some(slots)) => slots.get(i).copied(),
                        _ => None,
                    };
                    let ty = slot.and_then(|s| s.ty).map_or(Ty::Unknown, |t| self.peek_ty(t));
                    self.pattern(cx, &pf.pattern, &V { ty, taint: subject.taint });
                }
            }
            PatternKind::Array(items) => {
                let item = match subject.ty.base() {
                    Ty::Array(t) => (**t).clone(),
                    _ => Ty::Unknown,
                };
                for p in items {
                    self.pattern(cx, p, &V { ty: item.clone(), taint: subject.taint });
                }
            }
            PatternKind::Or(alts) => {
                for alt in alts {
                    self.pattern(cx, alt, subject);
                }
            }
        }
    }

    // ── Appels ───────────────────────────────────────────────

    fn args(&mut self, cx: &mut Ctx<'p>, args: &'p [Arg]) -> Vec<ArgV> {
        let mut out = Vec::new();
        for arg in args {
            match arg {
                Arg::Pos(e) => {
                    let v = self.expr(cx, e);
                    out.push(ArgV { name: None, v, span: e.span, lit: literal_string(e) });
                }
                Arg::Named { name, value } => {
                    let (v, span, lit) = match value {
                        Some(e) => (self.expr(cx, e), e.span, literal_string(e)),
                        None => (self.var(cx, &name.name, name.span), name.span, None),
                    };
                    out.push(ArgV { name: Some(name.name.clone()), v, span, lit });
                }
                Arg::BlockPass(e) => {
                    self.expr(cx, e);
                }
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn call(
        &mut self,
        cx: &mut Ctx<'p>,
        span: Span,
        recv: Option<&'p Expr>,
        name: &'p Ident,
        args: &'p [Arg],
        block: Option<&'p Block>,
        safe: bool,
    ) -> V {
        if recv.is_none()
            && name.name == "race"
            && let Some(b) = block
        {
            return self.block(cx, b, &[]);
        }
        let receiver = recv.map(|r| self.expr(cx, r));
        let argv = self.args(cx, args);
        // `xs.push(v)` : le tableau devient teinté si `v` l'est
        if let Some(r) = recv
            && matches!(name.name.as_str(), "push" | "append" | "unshift")
        {
            let taint = argv.iter().find_map(|a| a.v.taint);
            self.taint_container(cx, r, taint);
        }
        let result = match receiver {
            Some(r) => self.method(cx, span, r, name, argv, block),
            None => self.function(cx, span, name, argv, block),
        };
        if safe { V { ty: Ty::opt(result.ty), taint: result.taint } } else { result }
    }

    fn walk_block(&mut self, cx: &mut Ctx<'p>, block: Option<&'p Block>, params: &[V]) -> Option<V> {
        block.map(|b| self.block(cx, b, params))
    }

    fn function(
        &mut self,
        cx: &mut Ctx<'p>,
        span: Span,
        name: &'p Ident,
        argv: Vec<ArgV>,
        block: Option<&'p Block>,
    ) -> V {
        let n = name.name.as_str();
        if n.starts_with(|c: char| c.is_uppercase()) {
            self.walk_block(cx, block, &[]);
            return self.construct(span, n, argv);
        }
        if n == "run"
            && let Kind::Handler(agent, handler) = cx.kind
        {
            return self.run(cx, span, agent, handler, argv);
        }
        if matches!(n, "system" | "user" | "assistant") && matches!(cx.kind, Kind::Fn(f) if f.kind == FnKind::Prompt) {
            return V::new(Ty::Nil);
        }
        if let Some(self_ty) = cx.self_ty.clone()
            && let Some(def) = self.method_def(&self_ty, n)
        {
            let recv = V { ty: self_ty, taint: cx.self_taint };
            return self.user_method(cx, span, recv, def, argv, block);
        }
        if let Some(def) = self.fns.get(n).copied() {
            return self.fn_call(cx, span, def, None, argv, block);
        }
        if let Some(v) = self.builtin_function(cx, span, n, &argv, block) {
            return v;
        }
        if n == "run" {
            self.report(
                Diagnostic::new(name.span, "`run` n'est utilisable que dans un handler d'agent (`on Message … end`)")
                    .with_code(E_DECL),
            );
            return V::unknown();
        }
        self.walk_block(cx, block, &[]);
        let mut candidates = cx.names();
        candidates.extend(self.fns.keys().map(|s| s.to_string()));
        candidates.extend(builtins::GLOBALS.iter().map(|s| s.to_string()));
        self.error_help(
            E_NAME,
            name.span,
            format!("fonction inconnue `{n}`"),
            suggest(n, candidates.iter().map(String::as_str)),
        );
        V::unknown()
    }

    /// Lie les arguments aux paramètres ; renvoie la teinte de chaque paramètre.
    fn bind_args(&mut self, owner: &str, slots: &[Slot<'p>], args: &[ArgV], span: Span) -> Vec<Option<Span>> {
        let mut taints = vec![None; slots.len()];
        let mut used = vec![false; args.len()];
        let mut positional = args.iter().enumerate().filter(|(_, a)| a.name.is_none());
        for (i, slot) in slots.iter().enumerate() {
            let found = args
                .iter()
                .enumerate()
                .find(|(_, a)| a.name.as_deref() == Some(slot.name))
                .or_else(|| positional.next());
            match found {
                Some((j, arg)) => {
                    used[j] = true;
                    taints[i] = arg.v.taint;
                    if let Some(t) = slot.ty {
                        let expected = self.peek_ty(t);
                        if !self.compat(&arg.v.ty, &expected) {
                            self.error(
                                E_TYPE,
                                arg.span,
                                format!("`{owner}` attend `{expected}` pour `{}`, reçu `{}`", slot.name, arg.v.ty),
                            );
                        }
                    }
                }
                None if slot.optional => {}
                None => self.error(E_TYPE, span, format!("argument `{}` manquant pour `{owner}`", slot.name)),
            }
        }
        for (j, arg) in args.iter().enumerate() {
            if used[j] {
                continue;
            }
            match &arg.name {
                Some(n) => self.error_help(
                    E_TYPE,
                    arg.span,
                    format!("argument nommé inconnu `{n}:` pour `{owner}`"),
                    suggest(n, slots.iter().map(|s| s.name)),
                ),
                None => {
                    self.error(E_TYPE, arg.span, format!("trop d'arguments pour `{owner}` ({} attendus)", slots.len()))
                }
            }
        }
        taints
    }

    /// Une valeur teintée qui atteint un effet dangereux : E0412.
    fn taint_violation(&mut self, arg_span: Span, origin: Span, target: &str, effect: &str) {
        self.report(
            Diagnostic::new(
                arg_span,
                format!("une valeur produite par un LLM atteint `{target}` (effet `{effect}`) sans validation"),
            )
            .with_code(E_TAINT)
            .with_note(origin, "produite ici par un LLM")
            .with_help(TAINT_HELP),
        );
    }

    fn fn_call(
        &mut self,
        cx: &mut Ctx<'p>,
        span: Span,
        def: &'p FnDef,
        recv: Option<V>,
        argv: Vec<ArgV>,
        block: Option<&'p Block>,
    ) -> V {
        if block.is_some() {
            self.error(E_TYPE, span, format!("`{}` ne prend pas de bloc", def.name.name));
            self.walk_block(cx, block, &[]);
        }
        let mut taints = self.bind_args(&def.name.name, &Slot::params(&def.params), &argv, span);
        let mut recv = recv;
        if let Some(effect) = dangerous_effect(def) {
            for arg in &argv {
                if let Some(origin) = arg.v.taint {
                    self.taint_violation(arg.span, origin, &def.name.name, &effect);
                }
            }
            if let Some(origin) = recv.as_ref().and_then(|r| r.taint) {
                self.taint_violation(span, origin, &def.name.name, &effect);
            }
            // l'appel est refusé ici (et à l'exécution) : inutile de signaler la suite dans l'appelé
            taints = declared_taints(&def.params);
            if let Some(r) = recv.as_mut() {
                r.taint = None;
            }
        }
        let self_ty = recv.as_ref().map(|r| r.ty.clone());
        let self_taint = recv.and_then(|r| r.taint);
        let (ret, effects) = self.check_fn(def, self_ty, taints, self_taint);
        for e in effects {
            cx.add_effect(Eff { origin: span, ..e });
        }
        if def.kind == FnKind::Prompt {
            return V { ty: ret.ty, taint: Some(span) };
        }
        ret
    }

    fn user_method(
        &mut self,
        cx: &mut Ctx<'p>,
        span: Span,
        recv: V,
        def: &'p FnDef,
        argv: Vec<ArgV>,
        block: Option<&'p Block>,
    ) -> V {
        self.fn_call(cx, span, def, Some(recv), argv, block)
    }

    fn method_def(&self, ty: &Ty, name: &str) -> Option<&'p FnDef> {
        match ty.base() {
            Ty::User(t) => self.types.get(t.as_str()).and_then(|d| d.methods.get(name).copied()),
            Ty::Type(t) => self.types.get(t.as_str()).and_then(|d| d.statics.get(name).copied()),
            _ => None,
        }
    }

    /// Champ de struct (ou champ commun de variante d'enum).
    fn field_of(&self, ty: &Ty, name: &str) -> Option<Ty> {
        let Ty::User(t) = ty.base() else { return None };
        let decl = self.types.get(t.as_str())?;
        let field = match decl.def.kind {
            TypeKind::Struct => decl.fields.iter().find(|f| f.name.name == name).copied(),
            TypeKind::Enum => decl.variants.iter().flat_map(|v| v.fields.iter()).find(|f| f.name.name == name),
            _ => None,
        }?;
        Some(field.ty.as_ref().map_or(Ty::Unknown, |t| self.peek_ty(t)))
    }

    fn method(
        &mut self,
        cx: &mut Ctx<'p>,
        span: Span,
        recv: V,
        name: &'p Ident,
        argv: Vec<ArgV>,
        block: Option<&'p Block>,
    ) -> V {
        let n = name.name.as_str();
        let taint = recv.taint;
        // contrôle de la teinte
        match n {
            "trust!" => return V::new(recv.ty),
            "check" => {
                let clean = V::new(recv.ty.clone());
                self.walk_block(cx, block, &[clean]);
                return V::new(Ty::Result(Box::new(recv.ty), Box::new(Ty::user("CheckError"))));
            }
            "approve" => {
                cx.add_effect(Eff { path: "human".into(), arg: None, origin: span });
                return V::new(recv.ty);
            }
            _ => {}
        }
        if let Some(ty) = builtins::universal(n) {
            self.walk_block(cx, block, &[]);
            let taint = if n == "tainted?" { None } else { taint };
            return V { ty, taint };
        }
        match recv.ty.base().clone() {
            Ty::Unknown => {
                self.walk_block(cx, block, &[V { ty: Ty::Unknown, taint }]);
                V { ty: Ty::Unknown, taint }
            }
            Ty::Type(t) => self.static_call(cx, span, &t, name, argv, block),
            Ty::User(t) => self.user_type_method(cx, span, recv, &t, name, argv, block),
            base => {
                let arg0 = argv.first().map(|a| a.v.ty.clone());
                let params: Vec<V> =
                    builtins::block_params(&base, n, arg0.as_ref()).into_iter().map(|ty| V { ty, taint }).collect();
                let block_v = self.walk_block(cx, block, &params);
                match builtins::method(&base, n, argv.len(), block_v.as_ref().map(|v| &v.ty)) {
                    Some(ty) => {
                        let taint = taint.or(block_v
                            .and_then(|b| b.taint)
                            .filter(|_| matches!(n, "map" | "flat_map" | "sum" | "reduce" | "inject" | "or_else")));
                        V { ty, taint }
                    }
                    None => {
                        self.error(E_TYPE, name.span, format!("méthode `{n}` inconnue pour `{base}`"));
                        V { ty: Ty::Unknown, taint }
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn user_type_method(
        &mut self,
        cx: &mut Ctx<'p>,
        span: Span,
        recv: V,
        t: &str,
        name: &'p Ident,
        argv: Vec<ArgV>,
        block: Option<&'p Block>,
    ) -> V {
        let n = name.name.as_str();
        let taint = recv.taint;
        if let Some(def) = self.method_def(&recv.ty, n) {
            return self.user_method(cx, span, recv, def, argv, block);
        }
        let kind = self.types.get(t).map(|d| d.def.kind);
        if kind == Some(TypeKind::Agent) && matches!(n, "ask" | "tell") {
            let v = self.ask(cx, span, t, argv);
            return if n == "tell" { V::new(Ty::Nil) } else { v };
        }
        if argv.is_empty() && block.is_none() {
            if let Some(ty) = self.field_of(&recv.ty, n) {
                return V { ty, taint };
            }
            if kind == Some(TypeKind::Enum) && n == "name" {
                return V { ty: Ty::Str, taint };
            }
            if kind.is_none() && is_error_name(t) {
                // erreurs intégrées ou sans déclaration : champs libres
                return V { ty: builtins::error_field(t, n).unwrap_or(Ty::Unknown), taint };
            }
            if self.messages.contains_key(t) {
                return V { ty: Ty::Unknown, taint };
            }
        }
        if kind == Some(TypeKind::Struct) {
            match n {
                "with" => {
                    let fields = Slot::fields(&self.types[t].fields.clone());
                    for arg in &argv {
                        let Some(field) = &arg.name else { continue };
                        if !fields.iter().any(|s| s.name == field) {
                            self.error_help(
                                E_TYPE,
                                arg.span,
                                format!("champ inconnu `{field}:` pour `{t}`"),
                                suggest(field, fields.iter().map(|s| s.name)),
                            );
                        }
                    }
                    let taint = taint.or(argv.iter().find_map(|a| a.v.taint));
                    return V { ty: recv.ty, taint };
                }
                "to_h" => return V { ty: Ty::Hash(Box::new(Ty::Sym), Box::new(Ty::Unknown)), taint },
                _ => {}
            }
        }
        self.walk_block(cx, block, &[]);
        let mut candidates: Vec<&str> = Vec::new();
        if let Some(decl) = self.types.get(t) {
            candidates.extend(decl.methods.keys().copied());
            candidates.extend(decl.fields.iter().map(|f| f.name.name.as_str()));
        }
        let what = if candidates.is_empty() || kind == Some(TypeKind::Class) || kind == Some(TypeKind::Agent) {
            "méthode"
        } else {
            "champ ou méthode"
        };
        self.error_help(E_TYPE, name.span, format!("{what} `{n}` inconnu pour `{t}`"), suggest(n, candidates));
        V { ty: Ty::Unknown, taint }
    }

    fn static_call(
        &mut self,
        cx: &mut Ctx<'p>,
        span: Span,
        t: &str,
        name: &'p Ident,
        argv: Vec<ArgV>,
        block: Option<&'p Block>,
    ) -> V {
        let n = name.name.as_str();
        if let Some(decl) = self.types.get(t) {
            if n == "new" {
                self.walk_block(cx, block, &[]);
                return self.construct(span, t, argv);
            }
            if let Some(def) = decl.statics.get(n).copied() {
                return self.fn_call(cx, span, def, Some(V::new(Ty::Type(t.into()))), argv, block);
            }
            if self.variants.get(n).is_some_and(|e| *e == t) {
                return self.construct(span, n, argv);
            }
        }
        if let Some((ty, effect)) = builtins::static_method(t, n) {
            let block_v = self.walk_block(cx, block, &[]);
            if let Some(path) = effect {
                let arg = argv.first().and_then(|a| a.lit.clone());
                cx.add_effect(Eff { path: path.into(), arg, origin: span });
            }
            if effect == Some("fs.write") {
                for arg in &argv {
                    if let Some(origin) = arg.v.taint {
                        self.taint_violation(arg.span, origin, &format!("{t}.{n}"), "fs.write");
                    }
                }
            }
            let _ = block_v;
            return V::new(ty);
        }
        self.walk_block(cx, block, &[]);
        let mut candidates: Vec<&str> = Vec::new();
        if let Some(decl) = self.types.get(t) {
            candidates.extend(decl.statics.keys().copied());
            candidates.push("new");
        }
        self.error_help(E_TYPE, name.span, format!("méthode `{t}.{n}` inconnue"), suggest(n, candidates));
        V::unknown()
    }

    /// `Nom(…)` : struct, variante, erreur, message d'agent, `Ok`/`Err`.
    fn construct(&mut self, span: Span, name: &str, argv: Vec<ArgV>) -> V {
        let taint = argv.iter().find_map(|a| a.v.taint);
        match name {
            "Ok" => {
                return V {
                    ty: Ty::Result(Box::new(argv.first().map_or(Ty::Nil, |a| a.v.ty.clone())), Box::new(Ty::Unknown)),
                    taint,
                };
            }
            "Err" => {
                return V {
                    ty: Ty::Result(
                        Box::new(Ty::Unknown),
                        Box::new(argv.first().map_or(Ty::Unknown, |a| a.v.ty.clone())),
                    ),
                    taint,
                };
            }
            _ => {}
        }
        if let Some(decl) = self.types.get(name) {
            let (kind, fields) = (decl.def.kind, decl.fields.clone());
            let init = decl.methods.get("initialize").copied();
            let ivars = decl.ivars.clone();
            match kind {
                TypeKind::Struct => {
                    self.bind_args(name, &Slot::fields(&fields), &argv, span);
                }
                TypeKind::Class => match init {
                    Some(def) => {
                        self.bind_args(name, &Slot::params(&def.params), &argv, span);
                    }
                    None => {
                        let slots: Vec<Slot<'p>> =
                            Slot::fields(&ivars).into_iter().map(|s| Slot { optional: true, ..s }).collect();
                        self.bind_args(name, &slots, &argv, span);
                    }
                },
                TypeKind::Agent => self.report(
                    Diagnostic::new(span, format!("`{name}` est un agent"))
                        .with_code(E_TYPE)
                        .with_help(format!("démarrez-le avec `spawn {name}`")),
                ),
                _ => self.error(E_TYPE, span, format!("`{name}` ne peut pas être instancié")),
            }
            return V { ty: Ty::user(name), taint };
        }
        if let Some(enum_name) = self.variants.get(name).copied() {
            let variant =
                self.types[enum_name].variants.iter().find(|v| v.name.name == name).copied().expect("variante");
            let fields: Vec<&'p Field> = variant.fields.iter().collect();
            self.bind_args(name, &Slot::fields(&fields), &argv, span);
            return V { ty: Ty::user(enum_name), taint };
        }
        if is_error_name(name) {
            return V { ty: Ty::user(name), taint };
        }
        if let Some(handlers) = self.messages.get(name).cloned() {
            let (_, handler) = handlers[0];
            self.bind_args(name, &Slot::params(&handler.params), &argv, span);
            return V { ty: Ty::user(name), taint };
        }
        let known: Vec<&str> =
            self.types.keys().chain(self.variants.keys()).chain(self.messages.keys()).copied().collect();
        self.error_help(E_NAME, span, format!("type inconnu `{name}`"), suggest(name, known));
        V { ty: Ty::Unknown, taint }
    }

    fn ask(&mut self, cx: &mut Ctx<'p>, span: Span, agent: &str, argv: Vec<ArgV>) -> V {
        let Some(message) = argv.first() else {
            self.error(E_TYPE, span, "`ask` attend un message : `ask(Research(topic: t))`");
            return V::unknown();
        };
        let Ty::User(msg) = &message.v.ty else { return V::unknown() };
        let decl = &self.types[agent];
        let Some(handler) = decl.handlers.get(msg.as_str()).copied() else {
            let known: Vec<&str> = decl.handlers.keys().copied().collect();
            self.error_help(
                E_TYPE,
                message.span,
                format!("l'agent `{agent}` ne gère pas `{msg}`"),
                suggest(msg, known.clone()).or_else(|| Some(format!("messages gérés : {}", known.join(", ")))),
            );
            return V::unknown();
        };
        let agent: &'p str = decl.def.name.name.as_str();
        let taints = vec![message.v.taint; handler.params.len()];
        let (ret, effects) = self.check_handler(agent, handler, taints);
        for e in effects {
            cx.add_effect(Eff { origin: span, ..e });
        }
        ret
    }

    /// `run "consigne"` : boucle agentique ; résultat teinté, effets de ses outils.
    fn run(&mut self, cx: &mut Ctx<'p>, span: Span, agent: &'p str, handler: &'p Handler, argv: Vec<ArgV>) -> V {
        if argv.is_empty() {
            self.error(E_TYPE, span, "`run` attend une consigne : `run \"…\"`");
        }
        cx.run_span = Some(span);
        match &handler.ret {
            Some(ret) if !is_tainted_decl(ret) => self.report(
                Diagnostic::new(
                    ret.span(),
                    "ce handler renvoie le résultat de `run`, qui vient d'un LLM : son type doit être teinté",
                )
                .with_code(E_TAINT_DECL)
                .with_note(span, "résultat produit ici")
                .with_help(format!("écrivez `-> ~{}`", type_name(ret))),
            ),
            Some(ret) => {
                let ty = self.peek_ty(ret);
                if let Err(e) = self.schema_ok(&ty, 0) {
                    self.error(E_DECL, ret.span(), e);
                }
            }
            None => {}
        }
        cx.add_effect(Eff { path: "llm".into(), arg: None, origin: span });
        let tools: Vec<&'p FnDef> = self.types[agent]
            .directives
            .iter()
            .filter(|d| d.name.name == "tools")
            .flat_map(|d| d.args.iter())
            .filter_map(|a| match a {
                Arg::Pos(Expr { kind: ExprKind::Var(name), .. }) => self.fns.get(name.as_str()).copied(),
                _ => None,
            })
            .filter(|f| f.kind == FnKind::Tool)
            .collect();
        for tool in tools {
            // les arguments donnés par le LLM sont validés par le schéma : non teintés
            let (_, effects) = self.check_fn(tool, None, declared_taints(&tool.params), None);
            for e in effects {
                cx.add_effect(Eff { origin: span, ..e });
            }
        }
        let ty = handler.ret.as_ref().map_or(Ty::Str, |t| self.peek_ty(t));
        V { ty, taint: Some(span) }
    }

    fn builtin_function(
        &mut self,
        cx: &mut Ctx<'p>,
        span: Span,
        name: &str,
        argv: &[ArgV],
        block: Option<&'p Block>,
    ) -> Option<V> {
        let first = argv.first().map(|a| a.v.clone());
        let v = match name {
            "puts" | "print" | "warn" | "test" | "assert" | "assert_equal" => {
                self.walk_block(cx, block, &[]);
                V::new(Ty::Nil)
            }
            "p" => first.unwrap_or_else(|| V::new(Ty::Nil)),
            "raise" | "exit" => V::unknown(),
            "spawn" | "spawn_pool" => match first.map(|v| v.ty) {
                Some(Ty::Type(agent))
                    if self.types.get(agent.as_str()).is_some_and(|t| t.def.kind == TypeKind::Agent) =>
                {
                    V::new(Ty::User(agent))
                }
                Some(Ty::Unknown) => V::unknown(),
                Some(other) => {
                    self.error(E_TYPE, span, format!("`{name}` attend un type d'agent, reçu `{other}`"));
                    V::unknown()
                }
                None => {
                    self.error(E_TYPE, span, format!("`{name}` attend un agent : `{name} Researcher`"));
                    V::unknown()
                }
            },
            "budget" => {
                self.check_budget_options(argv);
                V::new(Ty::Budget)
            }
            "within" => {
                if let Some(v) = &first
                    && !self.compat(&v.ty, &Ty::Budget)
                {
                    self.error(E_TYPE, argv[0].span, format!("`within` attend un budget, reçu `{}`", v.ty));
                }
                self.walk_block(cx, block, &[]).unwrap_or_else(V::unknown)
            }
            "step" | "with_human" | "loop" => self.walk_block(cx, block, &[]).unwrap_or_else(V::unknown),
            "assert_raises" => {
                self.walk_block(cx, block, &[]);
                V::unknown()
            }
            "approve!" => {
                cx.add_effect(Eff { path: "human".into(), arg: None, origin: span });
                V::new(Ty::Nil)
            }
            "sleep" => {
                cx.add_effect(Eff { path: "time".into(), arg: None, origin: span });
                V::new(Ty::Nil)
            }
            "deny_all" | "approve_all" => V::new(Ty::Sym),
            _ => return None,
        };
        Some(v)
    }
}

impl<'p> TypeDecl<'p> {
    fn new(def: &'p TypeDef) -> Self {
        let mut decl = TypeDecl {
            def,
            fields: Vec::new(),
            ivars: Vec::new(),
            methods: HashMap::new(),
            statics: HashMap::new(),
            variants: Vec::new(),
            handlers: HashMap::new(),
            directives: Vec::new(),
            includes: Vec::new(),
        };
        for member in &def.members {
            match member {
                Member::Field(f) if f.is_ivar => decl.ivars.push(f),
                Member::Field(f) => decl.fields.push(f),
                Member::Method(m) if m.on_self => {
                    decl.statics.insert(&m.name.name, m);
                }
                Member::Method(m) => {
                    decl.methods.insert(&m.name.name, m);
                }
                Member::Variant(v) => decl.variants.push(v),
                Member::Handler(h) => {
                    decl.handlers.insert(&h.message.name, h);
                }
                Member::Directive(d) => decl.directives.push(d),
                Member::Include(_) => {}
            }
        }
        decl
    }
}

fn declared_taints(params: &[Param]) -> Vec<Option<Span>> {
    params.iter().map(|p| p.ty.as_ref().filter(|t| is_tainted_decl(t)).map(|_| p.span)).collect()
}

fn dangerous_effect(def: &FnDef) -> Option<String> {
    def.effects.iter().find_map(|e| {
        let path = e.path.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join(".");
        DANGEROUS_EFFECTS.contains(&path.as_str()).then_some(path)
    })
}

fn literal_string(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Str(segs) => segs
            .iter()
            .map(|s| match s {
                StrSeg::Lit(t) => Some(t.as_str()),
                StrSeg::Interp(_) => None,
            })
            .collect(),
        _ => None,
    }
}

fn op_str(op: BinOp) -> &'static str {
    use BinOp::*;
    match op {
        Add => "+",
        Sub => "-",
        Mul => "*",
        Div => "/",
        Rem => "%",
        Pow => "**",
        Eq => "==",
        NotEq => "!=",
        Lt => "<",
        Le => "<=",
        Gt => ">",
        Ge => ">=",
        Cmp => "<=>",
        Match => "=~",
        And => "&&",
        Or => "||",
        BitAnd => "&",
        BitOr => "|",
        BitXor => "^",
        Shl => "<<",
        Shr => ">>",
    }
}
