//! Valeurs manipulées par l'interpréteur.
//!
//! `'p` est la durée de vie du programme : les fermetures pointent directement
//! vers les blocs de l'AST, sans copie.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt::{self, Write};
use std::rc::Rc;
use std::time::Instant;

use grenat_ast::{Body, Span};

pub type Scope<'p> = Rc<RefCell<ScopeData<'p>>>;

#[derive(Default)]
pub struct ScopeData<'p> {
    pub vars: HashMap<String, Value<'p>>,
    pub parent: Option<Scope<'p>>,
}

pub fn new_scope<'p>(parent: Option<Scope<'p>>) -> Scope<'p> {
    Rc::new(RefCell::new(ScopeData { vars: HashMap::new(), parent }))
}

pub fn scope_get<'p>(scope: &Scope<'p>, name: &str) -> Option<Value<'p>> {
    let data = scope.borrow();
    match data.vars.get(name) {
        Some(v) => Some(v.clone()),
        None => data.parent.as_ref().and_then(|p| scope_get(p, name)),
    }
}

/// Affecte la variable là où elle existe déjà (fermetures), sinon la crée ici.
pub fn scope_set<'p>(scope: &Scope<'p>, name: &str, value: Value<'p>) {
    fn find<'p>(scope: &Scope<'p>, name: &str) -> Option<Scope<'p>> {
        let data = scope.borrow();
        if data.vars.contains_key(name) {
            return Some(scope.clone());
        }
        data.parent.as_ref().and_then(|p| find(p, name))
    }
    let target = find(scope, name).unwrap_or_else(|| scope.clone());
    target.borrow_mut().vars.insert(name.to_string(), value);
}

pub fn scope_define<'p>(scope: &Scope<'p>, name: &str, value: Value<'p>) {
    scope.borrow_mut().vars.insert(name.to_string(), value);
}

pub type Fields<'p> = Vec<(Rc<str>, Value<'p>)>;

pub fn field<'a, 'p>(fields: &'a Fields<'p>, name: &str) -> Option<&'a Value<'p>> {
    fields.iter().find(|(n, _)| &**n == name).map(|(_, v)| v)
}

#[derive(Clone)]
pub enum Value<'p> {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Montant en dollars (coûts LLM).
    Money(f64),
    /// Durée en secondes (`10.min`).
    Duration(f64),
    Str(Rc<str>),
    Symbol(Rc<str>),
    Array(Rc<RefCell<Vec<Value<'p>>>>),
    Hash(Rc<RefCell<Vec<(Value<'p>, Value<'p>)>>>),
    Range(i64, i64, bool),
    /// Instance de `struct` ou message d'agent : valeur immuable.
    Record(Rc<Record<'p>>),
    /// Instance de `class` ou d'`agent` : référence mutable.
    Object(Rc<Object<'p>>),
    /// Variante d'`enum` (dont `Ok` / `Err`).
    Variant(Rc<Variant<'p>>),
    Closure(Rc<Closure<'p>>),
    /// Un type ou module utilisé comme valeur (`Researcher`, `File`).
    Type(Rc<str>),
    Error(Rc<ErrorVal<'p>>),
    Budget(Rc<Budget>),
    /// Produit par un LLM, non validé : `~T`.
    Tainted(Rc<Value<'p>>),
}

pub struct Record<'p> {
    pub ty: Rc<str>,
    pub fields: Fields<'p>,
}

pub struct Object<'p> {
    pub ty: Rc<str>,
    pub fields: RefCell<Fields<'p>>,
    pub is_agent: bool,
}

pub struct Variant<'p> {
    pub enum_name: Rc<str>,
    pub name: Rc<str>,
    pub fields: Fields<'p>,
}

pub struct Closure<'p> {
    pub params: Vec<String>,
    pub body: &'p Body,
    pub scope: Scope<'p>,
    pub self_val: Option<Value<'p>>,
    /// Bloc appelé sur une valeur teintée : ses arguments le sont aussi.
    pub taint_args: bool,
}

pub struct ErrorVal<'p> {
    pub ty: Rc<str>,
    pub message: String,
    pub fields: Fields<'p>,
    pub span: Cell<Option<Span>>,
    /// Pile d'appels : (fonction, site d'appel).
    pub trace: RefCell<Vec<(String, Span)>>,
}

impl<'p> ErrorVal<'p> {
    pub fn new(ty: &str, message: impl Into<String>) -> Self {
        ErrorVal {
            ty: ty.into(),
            message: message.into(),
            fields: Vec::new(),
            span: Cell::new(None),
            trace: RefCell::new(Vec::new()),
        }
    }
}

/// Plafond de dépense : tokens, dollars, temps.
pub struct Budget {
    pub max_usd: Option<f64>,
    pub max_tokens: Option<u64>,
    pub max_seconds: Option<f64>,
    pub spent_usd: Cell<f64>,
    pub tokens: Cell<u64>,
    pub started: Cell<Instant>,
}

impl Budget {
    pub fn unlimited() -> Self {
        Budget {
            max_usd: None,
            max_tokens: None,
            max_seconds: None,
            spent_usd: Cell::new(0.0),
            tokens: Cell::new(0),
            started: Cell::new(Instant::now()),
        }
    }

    /// Description du dépassement, s'il y en a un.
    pub fn exceeded(&self) -> Option<String> {
        if let Some(max) = self.max_usd
            && self.spent_usd.get() > max
        {
            return Some(format!("{} dépensés, plafond {}", money(self.spent_usd.get()), money(max)));
        }
        if let Some(max) = self.max_tokens
            && self.tokens.get() > max
        {
            return Some(format!("{} tokens consommés, plafond {max}", self.tokens.get()));
        }
        if let Some(max) = self.max_seconds
            && self.started.get().elapsed().as_secs_f64() > max
        {
            return Some(format!("temps écoulé, plafond {}", duration(max)));
        }
        None
    }
}

pub fn money(usd: f64) -> String {
    if usd < 0.01 && usd > 0.0 { format!("${usd:.4}") } else { format!("${usd:.2}") }
}

pub fn duration(seconds: f64) -> String {
    if seconds >= 86_400.0 && seconds % 86_400.0 == 0.0 {
        format!("{}d", seconds / 86_400.0)
    } else if seconds >= 3600.0 && seconds % 3600.0 == 0.0 {
        format!("{}h", seconds / 3600.0)
    } else if seconds >= 60.0 && seconds % 60.0 == 0.0 {
        format!("{}min", seconds / 60.0)
    } else {
        format!("{seconds}s")
    }
}

fn float(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e16 { format!("{f:.1}") } else { f.to_string() }
}

impl<'p> Value<'p> {
    pub fn str(s: impl AsRef<str>) -> Self {
        Value::Str(s.as_ref().into())
    }

    pub fn array(items: Vec<Value<'p>>) -> Self {
        Value::Array(Rc::new(RefCell::new(items)))
    }

    pub fn record(ty: &str, fields: Fields<'p>) -> Self {
        Value::Record(Rc::new(Record { ty: ty.into(), fields }))
    }

    pub fn ok(value: Value<'p>) -> Self {
        Value::Variant(Rc::new(Variant {
            enum_name: "Result".into(),
            name: "Ok".into(),
            fields: vec![("value".into(), value)],
        }))
    }

    pub fn err(error: Value<'p>) -> Self {
        Value::Variant(Rc::new(Variant {
            enum_name: "Result".into(),
            name: "Err".into(),
            fields: vec![("error".into(), error)],
        }))
    }

    pub fn truthy(&self) -> bool {
        match self {
            Value::Nil | Value::Bool(false) => false,
            Value::Tainted(inner) => inner.truthy(),
            _ => true,
        }
    }

    pub fn is_tainted(&self) -> bool {
        matches!(self, Value::Tainted(_))
    }

    /// Teinte présente n'importe où dans la valeur (tableaux, champs…).
    pub fn contains_taint(&self) -> bool {
        match self {
            Value::Tainted(_) => true,
            Value::Array(items) => items.borrow().iter().any(Value::contains_taint),
            Value::Hash(entries) => entries.borrow().iter().any(|(k, v)| k.contains_taint() || v.contains_taint()),
            Value::Record(r) => r.fields.iter().any(|(_, v)| v.contains_taint()),
            Value::Variant(v) => v.fields.iter().any(|(_, v)| v.contains_taint()),
            _ => false,
        }
    }

    pub fn untainted(&self) -> &Value<'p> {
        match self {
            Value::Tainted(inner) => inner.untainted(),
            other => other,
        }
    }

    pub fn taint(self) -> Self {
        match self {
            Value::Tainted(_) | Value::Nil => self,
            other => Value::Tainted(Rc::new(other)),
        }
    }

    /// Nom du type, pour les messages d'erreur et `is_a?`.
    pub fn type_name(&self) -> String {
        match self {
            Value::Nil => "Nil".into(),
            Value::Bool(_) => "Bool".into(),
            Value::Int(_) => "Int".into(),
            Value::Float(_) => "Float".into(),
            Value::Money(_) => "Money".into(),
            Value::Duration(_) => "Duration".into(),
            Value::Str(_) => "String".into(),
            Value::Symbol(_) => "Symbol".into(),
            Value::Array(_) => "Array".into(),
            Value::Hash(_) => "Hash".into(),
            Value::Range(..) => "Range".into(),
            Value::Record(r) => r.ty.to_string(),
            Value::Object(o) => o.ty.to_string(),
            Value::Variant(v) => v.enum_name.to_string(),
            Value::Closure(_) => "Block".into(),
            Value::Type(_) => "Type".into(),
            Value::Error(e) => e.ty.to_string(),
            Value::Budget(_) => "Budget".into(),
            Value::Tainted(inner) => format!("~{}", inner.type_name()),
        }
    }

    /// Représentation pour `puts` et l'interpolation.
    pub fn to_display(&self) -> String {
        match self {
            Value::Nil => String::new(),
            Value::Str(s) | Value::Symbol(s) => s.to_string(),
            Value::Tainted(inner) => inner.to_display(),
            Value::Error(e) => e.message.clone(),
            _ => self.inspect(),
        }
    }

    /// Représentation pour `p` et les messages de débogage.
    pub fn inspect(&self) -> String {
        let mut out = String::new();
        self.write_inspect(&mut out);
        out
    }

    fn write_inspect(&self, out: &mut String) {
        let fields = |out: &mut String, fields: &Fields<'p>| {
            out.push('(');
            for (i, (name, value)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                let _ = write!(out, "{name}: ");
                value.write_inspect(out);
            }
            out.push(')');
        };
        match self {
            Value::Nil => out.push_str("nil"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Int(n) => out.push_str(&n.to_string()),
            Value::Float(f) => out.push_str(&float(*f)),
            Value::Money(m) => out.push_str(&money(*m)),
            Value::Duration(d) => out.push_str(&duration(*d)),
            Value::Str(s) => {
                let _ = write!(out, "\"{}\"", s.escape_debug());
            }
            Value::Symbol(s) => {
                let _ = write!(out, ":{s}");
            }
            Value::Array(items) => {
                out.push('[');
                for (i, item) in items.borrow().iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    item.write_inspect(out);
                }
                out.push(']');
            }
            Value::Hash(entries) => {
                out.push('{');
                for (i, (k, v)) in entries.borrow().iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    match k {
                        Value::Symbol(s) => {
                            let _ = write!(out, "{s}: ");
                        }
                        other => {
                            other.write_inspect(out);
                            out.push_str(" => ");
                        }
                    }
                    v.write_inspect(out);
                }
                out.push('}');
            }
            Value::Range(lo, hi, inclusive) => {
                let _ = write!(out, "{lo}{}{hi}", if *inclusive { ".." } else { "..." });
            }
            Value::Record(r) => {
                out.push_str(&r.ty);
                fields(out, &r.fields);
            }
            Value::Object(o) => {
                let _ = write!(out, "#<{}", o.ty);
                for (name, value) in o.fields.borrow().iter() {
                    let _ = write!(out, " @{name}=");
                    value.write_inspect(out);
                }
                out.push('>');
            }
            Value::Variant(v) => {
                out.push_str(&v.name);
                if !v.fields.is_empty() {
                    fields(out, &v.fields);
                }
            }
            Value::Closure(_) => out.push_str("#<Block>"),
            Value::Type(name) => out.push_str(name),
            Value::Error(e) => {
                let _ = write!(out, "{}(\"{}\")", e.ty, e.message.escape_debug());
            }
            Value::Budget(b) => {
                let _ = write!(out, "#<Budget {} / {} tokens>", money(b.spent_usd.get()), b.tokens.get());
            }
            Value::Tainted(inner) => {
                out.push('~');
                inner.write_inspect(out);
            }
        }
    }
}

impl fmt::Debug for Value<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.inspect())
    }
}

/// Égalité structurelle (identité pour les objets).
pub fn equal<'p>(a: &Value<'p>, b: &Value<'p>) -> bool {
    use Value::*;
    match (a.untainted(), b.untainted()) {
        (Nil, Nil) => true,
        (Bool(x), Bool(y)) => x == y,
        (Int(x), Int(y)) => x == y,
        (Float(x), Float(y)) | (Money(x), Money(y)) | (Duration(x), Duration(y)) => x == y,
        (Int(x), Float(y)) | (Float(y), Int(x)) => (*x as f64) == *y,
        (Str(x), Str(y)) | (Symbol(x), Symbol(y)) | (Type(x), Type(y)) => x == y,
        (Array(x), Array(y)) => {
            let (x, y) = (x.borrow(), y.borrow());
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| equal(a, b))
        }
        (Hash(x), Hash(y)) => {
            let (x, y) = (x.borrow(), y.borrow());
            x.len() == y.len() && x.iter().all(|(k, v)| y.iter().any(|(k2, v2)| equal(k, k2) && equal(v, v2)))
        }
        (Range(a, b, c), Range(x, y, z)) => (a, b, c) == (x, y, z),
        (Record(x), Record(y)) => x.ty == y.ty && fields_equal(&x.fields, &y.fields),
        (Variant(x), Variant(y)) => {
            x.enum_name == y.enum_name && x.name == y.name && fields_equal(&x.fields, &y.fields)
        }
        (Object(x), Object(y)) => Rc::ptr_eq(x, y),
        (Closure(x), Closure(y)) => Rc::ptr_eq(x, y),
        (Error(x), Error(y)) => x.ty == y.ty && x.message == y.message,
        (Budget(x), Budget(y)) => Rc::ptr_eq(x, y),
        _ => false,
    }
}

fn fields_equal<'p>(a: &Fields<'p>, b: &Fields<'p>) -> bool {
    a.len() == b.len() && a.iter().zip(b.iter()).all(|((n1, v1), (n2, v2))| n1 == n2 && equal(v1, v2))
}
