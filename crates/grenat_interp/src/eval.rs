//! Évaluation des expressions, appels de fonctions et de méthodes, motifs, agents.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

use grenat_ast::{
    ArmTest, BinOp, Block, Body, Expr, ExprKind, FnDef, FnKind, Ident, Pattern, PatternKind, StrSeg, TypeKind, UnOp,
};

use crate::value::{
    Budget, Closure, ErrorVal, Fields, Object, Value, Variant, equal, field, new_scope, scope_define, scope_get,
    scope_set,
};
use crate::{AgentFrame, Args, Ctrl, Frame, Interp, MAX_DEPTH, R, builtins, llm, raise};

/// Types d'erreur « intégrés », utilisables sans déclaration (`raise ApprovalDenied`).
const ERROR_NAMES: &[&str] = &[
    "Exception",
    "StandardError",
    "BudgetExceeded",
    "ApprovalDenied",
    "LlmRefusal",
    "MaxTurnsExceeded",
    "NoMatchingPattern",
];

/// Modules et types intégrés utilisables comme valeurs.
const BUILTIN_TYPES: &[&str] = &[
    "File", "Dir", "Math", "Env", "Json", "Runtime", "Cli", "Time", "Int", "Float", "String", "Bool", "Array", "Hash",
    "Symbol", "Nil", "Range", "Money", "Duration",
];

pub(crate) fn is_error_name(name: &str) -> bool {
    name.ends_with("Error") || ERROR_NAMES.contains(&name)
}

pub(crate) fn error_is_a(ty: &str, target: &str) -> bool {
    ty == target || matches!(target, "StandardError" | "Exception") || (ty == "LlmRefusal" && target == "LlmError")
}

/// Effet qui interdit de recevoir une valeur teintée.
fn dangerous_effect(def: &FnDef) -> Option<String> {
    def.effects.iter().find_map(|effect| {
        let path: Vec<&str> = effect.path.iter().map(|i| i.name.as_str()).collect();
        let name = path.join(".");
        matches!(name.as_str(), "shell" | "net" | "fs.write" | "human").then_some(name)
    })
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

pub(crate) fn compare<'p>(a: &Value<'p>, b: &Value<'p>) -> Option<Ordering> {
    use Value::*;
    match (a.untainted(), b.untainted()) {
        (Int(x), Int(y)) => Some(x.cmp(y)),
        (Int(x), Float(y)) => (*x as f64).partial_cmp(y),
        (Float(x), Int(y)) => x.partial_cmp(&(*y as f64)),
        (Float(x), Float(y)) | (Money(x), Money(y)) | (Duration(x), Duration(y)) => x.partial_cmp(y),
        (Money(x), Float(y)) | (Duration(x), Float(y)) => x.partial_cmp(y),
        (Str(x), Str(y)) | (Symbol(x), Symbol(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

impl<'p> Interp<'p> {
    // ── Contexte ─────────────────────────────────────────────

    pub(crate) fn scope(&self) -> &crate::value::Scope<'p> {
        &self.frames.last().expect("au moins une frame").scope
    }

    pub(crate) fn self_val(&self) -> Option<Value<'p>> {
        self.frames.last().and_then(|f| f.self_val.clone())
    }

    fn self_object(&self, ivar: &str) -> Result<Rc<Object<'p>>, Ctrl<'p>> {
        match self.self_val() {
            Some(Value::Object(o)) => Ok(o),
            _ => raise("NameError", format!("`@{ivar}` utilisé hors d'une classe ou d'un agent")),
        }
    }

    fn push_frame(&mut self, self_val: Option<Value<'p>>, scope: crate::value::Scope<'p>) -> Result<(), Ctrl<'p>> {
        if self.depth >= MAX_DEPTH {
            return raise("StackOverflow", "récursion trop profonde");
        }
        self.depth += 1;
        self.frames.push(Frame { self_val, scope });
        Ok(())
    }

    fn pop_frame(&mut self) {
        self.depth -= 1;
        self.frames.pop();
    }

    // ── Instructions ─────────────────────────────────────────

    pub(crate) fn eval_stmts(&mut self, stmts: &'p [Expr]) -> R<'p> {
        let mut last = Value::Nil;
        for stmt in stmts {
            last = self.eval(stmt)?;
        }
        Ok(last)
    }

    pub(crate) fn eval_body(&mut self, body: &'p Body) -> R<'p> {
        let mut result = self.eval_stmts(&body.stmts);
        if let Err(Ctrl::Raise(err)) = &result {
            let err = err.clone();
            let clause = body
                .rescues
                .iter()
                .find(|r| r.types.is_empty() || r.types.iter().any(|t| error_is_a(&err.ty, llm::type_name(t))));
            if let Some(clause) = clause {
                if let Some(binding) = &clause.binding {
                    scope_set(self.scope(), &binding.name, Value::Error(err));
                }
                result = self.eval_stmts(&clause.body);
            }
        }
        if let Some(ensure) = &body.ensure {
            self.eval_stmts(ensure)?;
        }
        result
    }

    // ── Expressions ──────────────────────────────────────────

    pub(crate) fn eval(&mut self, e: &'p Expr) -> R<'p> {
        self.eval_kind(e).inspect_err(|ctrl| {
            if let Ctrl::Raise(err) = ctrl
                && err.span.get().is_none()
            {
                err.span.set(Some(e.span));
            }
        })
    }

    fn eval_kind(&mut self, e: &'p Expr) -> R<'p> {
        match &e.kind {
            ExprKind::Int(n) => Ok(Value::Int(*n)),
            ExprKind::Float(f) => Ok(Value::Float(*f)),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            ExprKind::Nil => Ok(Value::Nil),
            ExprKind::Symbol(s) => Ok(Value::Symbol(s.as_str().into())),
            ExprKind::Str(segs) => self.string(segs),
            ExprKind::SelfRef => match self.self_val() {
                Some(v) => Ok(v),
                None => raise("NameError", "`self` utilisé hors d'une méthode"),
            },
            ExprKind::Array(items) => {
                let values = items.iter().map(|item| self.eval(item)).collect::<Result<_, _>>()?;
                Ok(Value::array(values))
            }
            ExprKind::Hash(entries) => {
                let mut pairs = Vec::with_capacity(entries.len());
                for (k, v) in entries {
                    pairs.push((self.eval(k)?, self.eval(v)?));
                }
                Ok(Value::Hash(Rc::new(RefCell::new(pairs))))
            }
            ExprKind::Range { lo, hi, inclusive } => {
                let (lo, hi) = (self.eval(lo)?, self.eval(hi)?);
                match (lo.untainted(), hi.untainted()) {
                    (Value::Int(a), Value::Int(b)) => Ok(Value::Range(*a, *b, *inclusive)),
                    _ => raise("TypeError", "les bornes d'un intervalle doivent être des entiers"),
                }
            }
            ExprKind::Var(name) => self.lookup_var(name),
            ExprKind::It => self.lookup_var("it"),
            ExprKind::Const(path) => self.resolve_const(path),
            ExprKind::IVar(name) => {
                let obj = self.self_object(name)?;
                let fields = obj.fields.borrow();
                Ok(field(&fields, name).cloned().unwrap_or(Value::Nil))
            }
            ExprKind::Call { recv, name, args, block, safe, .. } => self.eval_call(recv, name, args, block, *safe),
            ExprKind::Index { recv, args } => {
                let target = self.eval(recv)?;
                let index = args.iter().map(|a| self.eval(a)).collect::<Result<Vec<_>, _>>()?;
                self.index(target, index)
            }
            ExprKind::Unary { op, expr } => {
                let value = self.eval(expr)?;
                match op {
                    UnOp::Not => Ok(Value::Bool(!value.truthy())),
                    UnOp::Neg => {
                        let tainted = value.is_tainted();
                        let result = match value.untainted() {
                            Value::Int(n) => match n.checked_neg() {
                                Some(n) => Value::Int(n),
                                None => return raise("OverflowError", "dépassement d'entier"),
                            },
                            Value::Float(f) => Value::Float(-f),
                            Value::Money(m) => Value::Money(-m),
                            other => return raise("TypeError", format!("`-` non défini pour {}", other.type_name())),
                        };
                        Ok(if tainted { result.taint() } else { result })
                    }
                }
            }
            ExprKind::Binary { op: BinOp::And, lhs, rhs } => {
                let l = self.eval(lhs)?;
                if l.truthy() { self.eval(rhs) } else { Ok(l) }
            }
            ExprKind::Binary { op: BinOp::Or, lhs, rhs } => {
                let l = self.eval(lhs)?;
                if l.truthy() { Ok(l) } else { self.eval(rhs) }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let (l, r) = (self.eval(lhs)?, self.eval(rhs)?);
                self.binop(*op, l, r)
            }
            ExprKind::Try(inner) => {
                let value = self.eval(inner)?;
                self.try_unwrap(value)
            }
            ExprKind::Assign { target, value } => {
                let v = self.eval(value)?;
                self.assign(target, v.clone())?;
                Ok(v)
            }
            ExprKind::OpAssign { op, target, value } => self.op_assign(*op, target, value),
            ExprKind::MultiAssign { targets, value } => {
                let v = self.eval(value)?;
                let tainted = v.is_tainted();
                let items = match v.untainted() {
                    Value::Array(items) => items.borrow().clone(),
                    other => vec![other.clone()],
                };
                for (i, target) in targets.iter().enumerate() {
                    let item = items.get(i).cloned().unwrap_or(Value::Nil);
                    self.assign(target, if tainted { item.taint() } else { item })?;
                }
                Ok(v)
            }
            ExprKind::If { cond, then, else_ } => {
                if self.eval(cond)?.truthy() {
                    self.eval_stmts(then)
                } else if let Some(else_) = else_ {
                    self.eval_stmts(else_)
                } else {
                    Ok(Value::Nil)
                }
            }
            ExprKind::While { cond, body } => {
                while self.eval(cond)?.truthy() {
                    match self.eval_stmts(body) {
                        Ok(_) | Err(Ctrl::Next(_)) => {}
                        Err(Ctrl::Break(v)) => return Ok(v),
                        Err(other) => return Err(other),
                    }
                }
                Ok(Value::Nil)
            }
            ExprKind::Case { subject, arms, else_ } => self.case(subject.as_deref(), arms, else_.as_deref()),
            ExprKind::Begin(body) => self.eval_body(body),
            ExprKind::Return(value) => Err(Ctrl::Return(self.eval_opt(value.as_deref())?)),
            ExprKind::Break(value) => Err(Ctrl::Break(self.eval_opt(value.as_deref())?)),
            ExprKind::Next(value) => Err(Ctrl::Next(self.eval_opt(value.as_deref())?)),
        }
    }

    fn eval_opt(&mut self, e: Option<&'p Expr>) -> R<'p> {
        e.map_or(Ok(Value::Nil), |e| self.eval(e))
    }

    fn string(&mut self, segs: &'p [StrSeg]) -> R<'p> {
        let mut text = String::new();
        let mut tainted = false;
        for seg in segs {
            match seg {
                StrSeg::Lit(s) => text.push_str(s),
                StrSeg::Interp(e) => {
                    let v = self.eval(e)?;
                    tainted |= v.contains_taint();
                    text.push_str(&self.display(&v)?);
                }
            }
        }
        let v = Value::str(text);
        Ok(if tainted { v.taint() } else { v })
    }

    /// `to_s`, en tenant compte d'une méthode `to_s` définie par l'utilisateur.
    pub(crate) fn display(&mut self, v: &Value<'p>) -> Result<String, Ctrl<'p>> {
        if let Some(def) = self.method_of(v.untainted(), "to_s") {
            return Ok(self.call_fn(def, Args::default(), Some(v.untainted().clone()))?.to_display());
        }
        Ok(v.to_display())
    }

    fn try_unwrap(&mut self, value: Value<'p>) -> R<'p> {
        let tainted = value.is_tainted();
        match value.untainted() {
            Value::Variant(var) if &*var.enum_name == "Result" => {
                let payload = var.fields[0].1.clone();
                if &*var.name == "Ok" {
                    Ok(if tainted { payload.taint() } else { payload })
                } else {
                    match payload {
                        Value::Error(e) => Err(Ctrl::Raise(e)),
                        other => raise("ResultError", other.to_display()),
                    }
                }
            }
            Value::Nil => raise("NilError", "`?` appliqué à nil"),
            _ => Ok(value),
        }
    }

    fn lookup_var(&mut self, name: &str) -> R<'p> {
        if let Some(v) = scope_get(self.scope(), name) {
            return Ok(v);
        }
        if let Some(receiver) = self.self_val() {
            if let Some(v) = self.self_field(&receiver, name) {
                return Ok(v);
            }
            if self.method_of(&receiver, name).is_some() {
                return self.call_method(receiver, name, Args::default());
            }
        }
        if let Some(def) = self.fns.get(name).copied() {
            return self.call_fn(def, Args::default(), None);
        }
        if let Some(result) = builtins::call_global(self, name, Args::default()) {
            return result;
        }
        // `r?` : le lexer lit un nom de prédicat ; sur une variable, c'est l'opérateur `?`
        if let Some(base) = name.strip_suffix('?')
            && let Some(value) = scope_get(self.scope(), base)
        {
            return self.try_unwrap(value);
        }
        raise("NameError", format!("variable ou fonction inconnue `{name}`"))
    }

    fn self_field(&self, receiver: &Value<'p>, name: &str) -> Option<Value<'p>> {
        match receiver {
            Value::Record(r) => field(&r.fields, name).cloned(),
            Value::Variant(v) => field(&v.fields, name).cloned(),
            _ => None,
        }
    }

    fn resolve_const(&mut self, path: &'p [Ident]) -> R<'p> {
        let name = path.last().expect("chemin non vide").name.as_str();
        if path.len() >= 2 {
            let owner = path[path.len() - 2].name.as_str();
            if self.variants.get(name) == Some(&owner) {
                return self.variant_value(name);
            }
        }
        if self.types.contains_key(name)
            || BUILTIN_TYPES.contains(&name)
            || is_error_name(name)
            || self.messages.contains(name)
        {
            return Ok(Value::Type(name.into()));
        }
        if self.variants.contains_key(name) {
            return self.variant_value(name);
        }
        raise("NameError", format!("constante inconnue `{name}`"))
    }

    /// Variante sans champ (`Positive`) ; une variante à champs se construit avec `Nom(…)`.
    fn variant_value(&mut self, name: &str) -> R<'p> {
        let enum_name = self.variants[name];
        let has_fields = self.types[enum_name].variants.iter().any(|v| v.name.name == name && !v.fields.is_empty());
        if has_fields {
            return Ok(Value::Type(name.into()));
        }
        Ok(Value::Variant(Rc::new(Variant { enum_name: enum_name.into(), name: name.into(), fields: Vec::new() })))
    }

    // ── Opérateurs ───────────────────────────────────────────

    pub(crate) fn binop(&mut self, op: BinOp, l: Value<'p>, r: Value<'p>) -> R<'p> {
        if l.is_tainted() || r.is_tainted() {
            let result = self.binop(op, l.untainted().clone(), r.untainted().clone())?;
            return Ok(result.taint());
        }
        use Value::*;
        let type_error = |l: &Value, r: &Value| {
            raise(
                "TypeError",
                format!("opérateur `{}` non défini entre {} et {}", op_str(op), l.type_name(), r.type_name()),
            )
        };
        match (op, &l, &r) {
            (BinOp::Eq, ..) => Ok(Bool(equal(&l, &r))),
            (BinOp::NotEq, ..) => Ok(Bool(!equal(&l, &r))),
            (BinOp::Shl, Array(items), _) => {
                items.borrow_mut().push(r.clone());
                Ok(l.clone())
            }
            (BinOp::Add, Str(a), Str(b)) => Ok(Value::str(format!("{a}{b}"))),
            (BinOp::Add, Str(_), _) => raise(
                "TypeError",
                format!("impossible d'ajouter {} à une chaîne : utilisez l'interpolation \"#{{…}}\"", r.type_name()),
            ),
            (BinOp::Mul, Str(s), Int(n)) if *n >= 0 => Ok(Value::str(s.repeat(*n as usize))),
            (BinOp::Add, Array(a), Array(b)) => {
                Ok(Value::array(a.borrow().iter().chain(b.borrow().iter()).cloned().collect()))
            }
            (BinOp::Sub, Array(a), Array(b)) => {
                let b = b.borrow();
                Ok(Value::array(a.borrow().iter().filter(|x| !b.iter().any(|y| equal(x, y))).cloned().collect()))
            }
            (op, Int(a), Int(b)) => int_op(op, *a, *b).unwrap_or_else(|| type_error(&l, &r)),
            (op, Int(_) | Float(_), Int(_) | Float(_)) => {
                let as_f = |v: &Value| match v {
                    Int(n) => *n as f64,
                    Float(f) => *f,
                    _ => unreachable!(),
                };
                float_op(op, as_f(&l), as_f(&r)).unwrap_or_else(|| type_error(&l, &r))
            }
            (BinOp::Add, Money(a), Money(b)) => Ok(Money(a + b)),
            (BinOp::Sub, Money(a), Money(b)) => Ok(Money(a - b)),
            (BinOp::Add, Duration(a), Duration(b)) => Ok(Duration(a + b)),
            (BinOp::Mul, Duration(a), Int(n)) => Ok(Duration(a * *n as f64)),
            (BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Cmp, ..) => match compare(&l, &r) {
                Some(ord) => Ok(match op {
                    BinOp::Lt => Bool(ord.is_lt()),
                    BinOp::Le => Bool(ord.is_le()),
                    BinOp::Gt => Bool(ord.is_gt()),
                    BinOp::Ge => Bool(ord.is_ge()),
                    _ => Int(ord as i64),
                }),
                None => type_error(&l, &r),
            },
            _ => type_error(&l, &r),
        }
    }

    // ── Affectation ──────────────────────────────────────────

    fn assign(&mut self, target: &'p Expr, value: Value<'p>) -> Result<(), Ctrl<'p>> {
        match &target.kind {
            ExprKind::Var(name) => {
                scope_set(self.scope(), name, value);
                Ok(())
            }
            ExprKind::IVar(name) => {
                let obj = self.self_object(name)?;
                set_field(&mut obj.fields.borrow_mut(), name, value);
                Ok(())
            }
            ExprKind::Index { recv, args } => {
                let target = self.eval(recv)?;
                let index = args.iter().map(|a| self.eval(a)).collect::<Result<Vec<_>, _>>()?;
                self.index_set(target, index, value)
            }
            ExprKind::Call { recv: Some(recv), name, .. } => match self.eval(recv)? {
                Value::Object(obj) => {
                    set_field(&mut obj.fields.borrow_mut(), &name.name, value);
                    Ok(())
                }
                Value::Record(r) => raise(
                    "TypeError",
                    format!("`{}` est une struct immuable : créez une copie avec `.with({}: …)`", r.ty, name.name),
                ),
                other => raise("TypeError", format!("impossible d'affecter `{}` sur {}", name.name, other.type_name())),
            },
            _ => raise("TypeError", "cible d'affectation invalide"),
        }
    }

    fn op_assign(&mut self, op: BinOp, target: &'p Expr, value: &'p Expr) -> R<'p> {
        let current = match &target.kind {
            ExprKind::Var(name) => scope_get(self.scope(), name).unwrap_or(Value::Nil),
            ExprKind::IVar(name) => {
                let obj = self.self_object(name)?;
                let fields = obj.fields.borrow();
                field(&fields, name).cloned().unwrap_or(Value::Nil)
            }
            _ => self.eval(target)?,
        };
        let new = match op {
            BinOp::Or if current.truthy() => return Ok(current),
            BinOp::And if !current.truthy() => return Ok(current),
            BinOp::Or | BinOp::And => self.eval(value)?,
            op => {
                let rhs = self.eval(value)?;
                self.binop(op, current, rhs)?
            }
        };
        self.assign(target, new.clone())?;
        Ok(new)
    }

    // ── Indexation ───────────────────────────────────────────

    fn index(&mut self, target: Value<'p>, index: Vec<Value<'p>>) -> R<'p> {
        if let Value::Tainted(inner) = &target {
            return Ok(self.index((**inner).clone(), index)?.taint());
        }
        let [key] = index.as_slice() else {
            return raise("ArgumentError", "un seul indice attendu");
        };
        match (&target, key.untainted()) {
            (Value::Array(items), Value::Int(i)) => {
                let items = items.borrow();
                Ok(resolve_index(*i, items.len()).and_then(|i| items.get(i)).cloned().unwrap_or(Value::Nil))
            }
            (Value::Array(items), Value::Range(lo, hi, inclusive)) => {
                let items = items.borrow();
                let len = items.len() as i64;
                let start = if *lo < 0 { lo + len } else { *lo }.clamp(0, len) as usize;
                let end = if *hi < 0 { hi + len } else { *hi } + i64::from(*inclusive);
                let end = end.clamp(start as i64, len) as usize;
                Ok(Value::array(items[start..end].to_vec()))
            }
            (Value::Str(s), Value::Int(i)) => {
                let chars: Vec<char> = s.chars().collect();
                Ok(resolve_index(*i, chars.len())
                    .and_then(|i| chars.get(i))
                    .map_or(Value::Nil, |c| Value::str(c.to_string())))
            }
            (Value::Hash(entries), key) => {
                Ok(entries.borrow().iter().find(|(k, _)| equal(k, key)).map_or(Value::Nil, |(_, v)| v.clone()))
            }
            (Value::Type(sup), Value::Type(agent)) => self.supervisor_child(sup, agent),
            (target, key) => {
                raise("TypeError", format!("{} ne peut pas être indexé par {}", target.type_name(), key.type_name()))
            }
        }
    }

    fn index_set(&mut self, target: Value<'p>, index: Vec<Value<'p>>, value: Value<'p>) -> Result<(), Ctrl<'p>> {
        let [key] = index.as_slice() else {
            return raise("ArgumentError", "un seul indice attendu");
        };
        match (&target, key.untainted()) {
            (Value::Array(items), Value::Int(i)) => {
                let mut items = items.borrow_mut();
                let len = items.len();
                let Some(i) = resolve_index(*i, len.max(*i as usize + 1)) else {
                    return raise("IndexError", format!("indice {i} hors du tableau"));
                };
                if i >= items.len() {
                    items.resize(i + 1, Value::Nil);
                }
                items[i] = value;
                Ok(())
            }
            (Value::Hash(entries), key) => {
                let mut entries = entries.borrow_mut();
                match entries.iter_mut().find(|(k, _)| equal(k, key)) {
                    Some((_, v)) => *v = value,
                    None => entries.push((key.clone(), value)),
                }
                Ok(())
            }
            (target, _) => raise("TypeError", format!("impossible d'affecter un indice sur {}", target.type_name())),
        }
    }

    // ── case ─────────────────────────────────────────────────

    fn case(&mut self, subject: Option<&'p Expr>, arms: &'p [grenat_ast::CaseArm], else_: Option<&'p [Expr]>) -> R<'p> {
        let subject = subject.map(|s| self.eval(s)).transpose()?;
        for arm in arms {
            let matched = match &arm.test {
                ArmTest::In(pattern) => match &subject {
                    Some(s) => self.match_pattern(pattern, s)?,
                    None => return raise("SyntaxError", "`case … in` exige une valeur à filtrer"),
                },
                ArmTest::When(values) => {
                    let mut any = false;
                    for v in values {
                        let candidate = self.eval(v)?;
                        let hit = match &subject {
                            Some(s) => self.case_eq(&candidate, s),
                            None => candidate.truthy(),
                        };
                        if hit {
                            any = true;
                            break;
                        }
                    }
                    any
                }
            };
            let guard = match (&arm.guard, matched) {
                (Some(g), true) => self.eval(g)?.truthy(),
                (None, m) => m,
                (_, false) => false,
            };
            if guard {
                return self.eval_stmts(&arm.body);
            }
        }
        match else_ {
            Some(stmts) => self.eval_stmts(stmts),
            None if arms.iter().any(|a| matches!(a.test, ArmTest::In(_))) => raise(
                "NoMatchingPattern",
                format!("aucun motif ne correspond à {}", subject.map_or("nil".into(), |s| s.inspect())),
            ),
            None => Ok(Value::Nil),
        }
    }

    fn case_eq(&self, candidate: &Value<'p>, subject: &Value<'p>) -> bool {
        match candidate {
            Value::Type(name) => self.is_a(subject, name),
            Value::Range(lo, hi, inclusive) => match subject.untainted() {
                Value::Int(n) => *n >= *lo && if *inclusive { n <= hi } else { n < hi },
                _ => false,
            },
            _ => equal(candidate, subject),
        }
    }

    pub(crate) fn is_a(&self, value: &Value<'p>, name: &str) -> bool {
        match value.untainted() {
            Value::Record(r) => &*r.ty == name,
            Value::Object(o) => &*o.ty == name,
            Value::Variant(v) => &*v.enum_name == name || &*v.name == name,
            Value::Error(e) => error_is_a(&e.ty, name),
            Value::Int(_) | Value::Float(_) if name == "Numeric" => true,
            other => other.type_name() == name,
        }
    }

    fn match_pattern(&mut self, pattern: &'p Pattern, value: &Value<'p>) -> Result<bool, Ctrl<'p>> {
        let tainted = value.is_tainted();
        let inner = value.untainted().clone();
        let wrap = |v: Value<'p>| if tainted { v.taint() } else { v };
        match &pattern.kind {
            PatternKind::Wildcard => Ok(true),
            PatternKind::Bind(name) => {
                scope_set(self.scope(), name, value.clone());
                Ok(true)
            }
            PatternKind::Lit(e) => {
                let lit = self.eval(e)?;
                Ok(equal(&lit, &inner))
            }
            PatternKind::Const { path, fields } => {
                let name = path.last().expect("chemin non vide").name.as_str();
                let (type_ok, value_fields): (bool, Option<Fields<'p>>) = match &inner {
                    Value::Record(r) => (&*r.ty == name, Some(r.fields.clone())),
                    Value::Variant(v) => {
                        (&*v.name == name || (&*v.enum_name == name && fields.is_none()), Some(v.fields.clone()))
                    }
                    Value::Error(e) => (error_is_a(&e.ty, name), Some(e.fields.clone())),
                    other => (self.is_a(other, name), None),
                };
                if !type_ok {
                    return Ok(false);
                }
                let Some(pattern_fields) = fields else { return Ok(true) };
                let value_fields = value_fields.unwrap_or_default();
                for (i, pf) in pattern_fields.iter().enumerate() {
                    let fv = match &pf.name {
                        Some(n) => field(&value_fields, &n.name).cloned(),
                        None => value_fields.get(i).map(|(_, v)| v.clone()),
                    };
                    let Some(fv) = fv else { return Ok(false) };
                    if !self.match_pattern(&pf.pattern, &wrap(fv))? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            PatternKind::Array(patterns) => {
                let Value::Array(items) = &inner else { return Ok(false) };
                let items = items.borrow().clone();
                if items.len() != patterns.len() {
                    return Ok(false);
                }
                for (p, item) in patterns.iter().zip(items) {
                    if !self.match_pattern(p, &wrap(item))? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            PatternKind::Or(alts) => {
                for alt in alts {
                    if self.match_pattern(alt, value)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }

    // ── Appels ───────────────────────────────────────────────

    fn eval_call(
        &mut self,
        recv: &'p Option<Box<Expr>>,
        name: &'p Ident,
        args: &'p [grenat_ast::Arg],
        block: &'p Option<Box<Block>>,
        safe: bool,
    ) -> R<'p> {
        if recv.is_none()
            && name.name == "race"
            && let Some(block) = block
        {
            return self.race(block);
        }
        let receiver = match recv {
            Some(r) => {
                let v = self.eval(r)?;
                if safe && matches!(v, Value::Nil) {
                    return Ok(Value::Nil);
                }
                Some(v)
            }
            None => None,
        };
        let mut call_args = self.eval_args(args)?;
        if let Some(block) = block {
            call_args.block = Some(self.make_closure(block));
        }
        let result = match receiver {
            Some(v) => self.call_method(v, &name.name, call_args),
            None => self.call_function(&name.name, call_args),
        };
        match result {
            // `break` dans un bloc termine l'appel qui l'a reçu
            Err(Ctrl::Break(v)) if block.is_some() => Ok(v),
            other => other,
        }
    }

    fn eval_args(&mut self, args: &'p [grenat_ast::Arg]) -> Result<Args<'p>, Ctrl<'p>> {
        let mut out = Args::default();
        for arg in args {
            match arg {
                grenat_ast::Arg::Pos(e) => out.pos.push(self.eval(e)?),
                grenat_ast::Arg::Named { name, value } => {
                    let v = match value {
                        Some(e) => self.eval(e)?,
                        None => self.lookup_var(&name.name)?,
                    };
                    out.named.push((name.name.clone(), v));
                }
                grenat_ast::Arg::BlockPass(e) => out.block = Some(self.eval(e)?),
            }
        }
        Ok(out)
    }

    fn make_closure(&self, block: &'p Block) -> Value<'p> {
        Value::Closure(Rc::new(Closure {
            params: block.params.iter().map(|p| p.name.name.clone()).collect(),
            body: &block.body,
            scope: self.scope().clone(),
            self_val: self.self_val(),
            taint_args: false,
        }))
    }

    pub(crate) fn call_block(&mut self, block: &Value<'p>, args: Vec<Value<'p>>) -> R<'p> {
        match block {
            Value::Closure(c) => {
                let scope = new_scope(Some(c.scope.clone()));
                // `|k, v|` sur une paire : déstructuration, comme en Ruby
                let args = match args.as_slice() {
                    [single] if c.params.len() > 1 => match single.untainted() {
                        Value::Array(items) => {
                            let tainted = single.is_tainted();
                            items.borrow().iter().map(|v| if tainted { v.clone().taint() } else { v.clone() }).collect()
                        }
                        _ => args,
                    },
                    _ => args,
                };
                for (i, param) in c.params.iter().enumerate() {
                    let v = args.get(i).cloned().unwrap_or(Value::Nil);
                    scope_define(&scope, param, if c.taint_args { v.taint() } else { v });
                }
                self.push_frame(c.self_val.clone(), scope)?;
                let result = self.eval_body(c.body);
                self.pop_frame();
                match result {
                    Err(Ctrl::Next(v)) => Ok(v),
                    other => other,
                }
            }
            // `&:upcase`
            Value::Symbol(name) => {
                let receiver = args.into_iter().next().unwrap_or(Value::Nil);
                self.call_method(receiver, name, Args::default())
            }
            other => raise("TypeError", format!("bloc attendu, reçu {}", other.type_name())),
        }
    }

    fn race(&mut self, block: &'p Block) -> R<'p> {
        // Phase 1 : exécution séquentielle, la première branche gagne.
        let scope = new_scope(Some(self.scope().clone()));
        self.push_frame(self.self_val(), scope)?;
        let result = block.body.stmts.first().map_or(Ok(Value::Nil), |first| self.eval(first));
        self.pop_frame();
        result
    }

    pub(crate) fn call_function(&mut self, name: &str, args: Args<'p>) -> R<'p> {
        if name.starts_with(|c: char| c.is_uppercase()) {
            return self.construct(name, args);
        }
        if name == "run" && self.in_current_agent() {
            return self.agent_run(args);
        }
        if let Some(receiver) = self.self_val()
            && self.method_of(&receiver, name).is_some()
        {
            return self.call_method(receiver, name, args);
        }
        if let Some(def) = self.fns.get(name).copied() {
            return self.call_fn(def, args, None);
        }
        if let Some(result) = builtins::call_global(self, name, args) {
            return result;
        }
        raise("NameError", format!("fonction inconnue `{name}`"))
    }

    fn in_current_agent(&self) -> bool {
        match (self.agents.last(), self.self_val()) {
            (Some(frame), Some(Value::Object(obj))) => Rc::ptr_eq(&frame.agent, &obj),
            _ => false,
        }
    }

    pub(crate) fn method_of(&self, receiver: &Value<'p>, name: &str) -> Option<&'p FnDef> {
        let ty: &str = match receiver {
            Value::Record(r) => &r.ty,
            Value::Object(o) => &o.ty,
            Value::Variant(v) => &v.enum_name,
            _ => return None,
        };
        self.types.get(ty).and_then(|info| info.methods.get(name).copied())
    }

    pub(crate) fn call_fn(&mut self, def: &'p FnDef, args: Args<'p>, self_val: Option<Value<'p>>) -> R<'p> {
        if def.is_abstract {
            return raise("NotImplementedError", format!("`{}` est abstraite", def.name.name));
        }
        if args.block.is_some() {
            return raise("ArgumentError", format!("`{}` ne prend pas de bloc", def.name.name));
        }
        if let Some(effect) = dangerous_effect(def) {
            let tainted = args.pos.iter().chain(args.named.iter().map(|(_, v)| v)).any(Value::contains_taint);
            if tainted {
                return raise(
                    "TaintError",
                    format!(
                        "une valeur produite par un LLM atteint `{}` (effet `{effect}`) sans validation ; \
                         validez-la avec `.check {{ … }}`, `.approve(by: :human)` ou `.trust!`",
                        def.name.name
                    ),
                );
            }
        }
        self.push_frame(self_val, new_scope(None))?;
        let result = self.bind_params(&def.params, args, &def.name.name).and_then(|()| match def.kind {
            FnKind::Prompt => self.run_prompt(def),
            _ => self.eval_body(&def.body),
        });
        self.pop_frame();
        match result {
            Ok(v) | Err(Ctrl::Return(v)) => Ok(v),
            Err(Ctrl::Raise(e)) => {
                e.trace.borrow_mut().push((def.name.name.clone(), def.span));
                Err(Ctrl::Raise(e))
            }
            Err(Ctrl::Break(_) | Ctrl::Next(_)) => {
                raise("LocalJumpError", format!("`break`/`next` hors d'une boucle dans `{}`", def.name.name))
            }
            Err(other) => Err(other),
        }
    }

    pub(crate) fn bind_params(
        &mut self,
        params: &'p [grenat_ast::Param],
        args: Args<'p>,
        owner: &str,
    ) -> Result<(), Ctrl<'p>> {
        let mut pos = args.pos.into_iter();
        let mut named = args.named;
        for param in params {
            let value = if let Some(i) = named.iter().position(|(n, _)| *n == param.name.name) {
                named.remove(i).1
            } else if let Some(v) = pos.next() {
                v
            } else if let Some(default) = &param.default {
                self.eval(default)?
            } else {
                return raise("ArgumentError", format!("argument `{}` manquant pour `{owner}`", param.name.name));
            };
            scope_define(self.scope(), &param.name.name, value);
        }
        if pos.next().is_some() {
            return raise("ArgumentError", format!("trop d'arguments pour `{owner}` ({} attendus)", params.len()));
        }
        if let Some((name, _)) = named.first() {
            return raise("ArgumentError", format!("argument nommé inconnu `{name}:` pour `{owner}`"));
        }
        Ok(())
    }

    pub(crate) fn call_method(&mut self, receiver: Value<'p>, name: &str, args: Args<'p>) -> R<'p> {
        match &receiver {
            Value::Tainted(inner) => {
                let inner = (**inner).clone();
                return self.tainted_method(receiver, inner, name, args);
            }
            Value::Type(ty) => {
                let ty = ty.clone();
                return self.static_method(&ty, name, args);
            }
            _ => {}
        }
        if let Some(def) = self.method_of(&receiver, name) {
            return self.call_fn(def, args, Some(receiver));
        }
        if let Value::Object(obj) = &receiver
            && obj.is_agent
        {
            match name {
                "ask" => return self.agent_ask(obj.clone(), args),
                "tell" => return self.agent_ask(obj.clone(), args).map(|_| Value::Nil),
                _ => {}
            }
        }
        if args.is_empty() {
            let found = match &receiver {
                Value::Record(r) => field(&r.fields, name).cloned(),
                Value::Variant(v) => field(&v.fields, name).cloned(),
                Value::Error(e) => field(&e.fields, name).cloned(),
                _ => None,
            };
            if let Some(v) = found {
                return Ok(v);
            }
        }
        builtins::call_method(self, receiver, name, args)
    }

    /// Méthodes d'une valeur `~T` : validation, sinon propagation de la teinte.
    fn tainted_method(&mut self, receiver: Value<'p>, inner: Value<'p>, name: &str, mut args: Args<'p>) -> R<'p> {
        match name {
            "trust!" => Ok(inner),
            "tainted?" => Ok(Value::Bool(true)),
            "inspect" => Ok(Value::str(receiver.inspect())),
            "check" => {
                let Some(block) = args.block else {
                    return raise("ArgumentError", "`check` attend un bloc de validation");
                };
                if self.call_block(&block, vec![inner.clone()])?.truthy() {
                    Ok(Value::ok(inner))
                } else {
                    let error = ErrorVal::new("CheckError", format!("validation refusée pour {}", inner.inspect()));
                    Ok(Value::err(Value::Error(Rc::new(error))))
                }
            }
            "approve" => {
                let message = format!("Valeur produite par un LLM :\n{}", inner.inspect());
                if self.ask_human(&message)? {
                    Ok(inner)
                } else {
                    raise("ApprovalDenied", "valeur refusée par l'humain")
                }
            }
            _ => {
                if let Some(Value::Closure(c)) = &args.block {
                    args.block = Some(Value::Closure(Rc::new(Closure {
                        params: c.params.clone(),
                        body: c.body,
                        scope: c.scope.clone(),
                        self_val: c.self_val.clone(),
                        taint_args: true,
                    })));
                }
                Ok(self.call_method(inner, name, args)?.taint())
            }
        }
    }

    fn static_method(&mut self, ty: &str, name: &str, args: Args<'p>) -> R<'p> {
        if let Some(info) = self.types.get(ty) {
            if name == "new" {
                return self.construct(ty, args);
            }
            if let Some(def) = info.statics.get(name).copied() {
                return self.call_fn(def, args, Some(Value::Type(ty.into())));
            }
            if self.variants.get(name) == Some(&info.def.name.name.as_str()) {
                return self.construct(name, args);
            }
        }
        builtins::call_static(self, ty, name, args)
    }

    // ── Construction ─────────────────────────────────────────

    pub(crate) fn construct(&mut self, name: &str, args: Args<'p>) -> R<'p> {
        match name {
            "Ok" => return Ok(Value::ok(args.pos.into_iter().next().unwrap_or(Value::Nil))),
            "Err" => return Ok(Value::err(args.pos.into_iter().next().unwrap_or(Value::Nil))),
            _ => {}
        }
        if let Some(info) = self.types.get(name) {
            let (kind, fields) = (info.def.kind, info.fields.clone());
            let name: &'p str = info.def.name.name.as_str();
            return match kind {
                TypeKind::Struct => {
                    let fields = self.build_fields(name, &fields, args)?;
                    Ok(Value::record(name, fields))
                }
                TypeKind::Class => self.new_object(name, args, false),
                TypeKind::Agent => raise("TypeError", format!("un agent se démarre avec `spawn {name}`")),
                TypeKind::Enum => {
                    raise("TypeError", format!("`{name}` est un enum : construisez une de ses variantes"))
                }
                TypeKind::Module | TypeKind::Supervisor => {
                    raise("TypeError", format!("`{name}` ne peut pas être instancié"))
                }
            };
        }
        if let Some(enum_name) = self.variants.get(name).copied() {
            let variant =
                self.types[enum_name].variants.iter().find(|v| v.name.name == name).copied().expect("variante");
            let defs: Vec<_> = variant.fields.iter().collect();
            let fields = self.build_fields(name, &defs, args)?;
            return Ok(Value::Variant(Rc::new(Variant { enum_name: enum_name.into(), name: name.into(), fields })));
        }
        if is_error_name(name) {
            let mut error = ErrorVal::new(name, name);
            let mut pos = args.pos.into_iter();
            if let Some(message) = pos.next() {
                error.message = message.to_display();
            }
            error.fields = args.named.into_iter().map(|(n, v)| (n.into(), v)).collect();
            return Ok(Value::Error(Rc::new(error)));
        }
        if self.messages.contains(name) {
            // message d'agent : champs positionnels `_0`, `_1`… puis nommés
            let mut fields: Fields<'p> =
                args.pos.into_iter().enumerate().map(|(i, v)| (format!("_{i}").into(), v)).collect();
            fields.extend(args.named.into_iter().map(|(n, v)| (n.into(), v)));
            return Ok(Value::record(name, fields));
        }
        raise("NameError", format!("type inconnu `{name}`"))
    }

    fn build_fields(
        &mut self,
        owner: &str,
        defs: &[&'p grenat_ast::Field],
        args: Args<'p>,
    ) -> Result<Fields<'p>, Ctrl<'p>> {
        let mut pos = args.pos.into_iter();
        let mut named = args.named;
        let mut fields = Vec::with_capacity(defs.len());
        for def in defs {
            let value = if let Some(i) = named.iter().position(|(n, _)| *n == def.name.name) {
                named.remove(i).1
            } else if let Some(v) = pos.next() {
                v
            } else if let Some(default) = &def.default {
                self.eval(default)?
            } else if matches!(def.ty, Some(grenat_ast::Type::Optional(..))) {
                Value::Nil
            } else {
                return raise("ArgumentError", format!("champ `{}` manquant pour `{owner}`", def.name.name));
            };
            fields.push((def.name.name.as_str().into(), value));
        }
        if pos.next().is_some() {
            return raise("ArgumentError", format!("trop de valeurs pour `{owner}` ({} champs)", defs.len()));
        }
        if let Some((name, _)) = named.first() {
            return raise("ArgumentError", format!("champ inconnu `{name}:` pour `{owner}`"));
        }
        Ok(fields)
    }

    /// Instance de classe ou d'agent : état `@…` initialisé, puis `initialize`.
    fn new_object(&mut self, ty: &'p str, args: Args<'p>, is_agent: bool) -> R<'p> {
        let obj = Rc::new(Object { ty: ty.into(), fields: RefCell::new(Vec::new()), is_agent });
        let value = Value::Object(obj.clone());
        let info_fields = self.types[ty].fields.clone();
        self.push_frame(Some(value.clone()), new_scope(None))?;
        let init = (|| {
            for f in info_fields.iter().filter(|f| f.is_ivar) {
                let v = match &f.default {
                    Some(d) => self.eval(d)?,
                    None => Value::Nil,
                };
                set_field(&mut obj.fields.borrow_mut(), &f.name.name, v);
            }
            Ok(())
        })();
        self.pop_frame();
        init?;

        if let Some(initialize) = self.types[ty].methods.get("initialize").copied() {
            self.call_fn(initialize, args, Some(value.clone()))?;
        } else {
            if let Some(v) = args.pos.first() {
                return raise(
                    "ArgumentError",
                    format!("`{ty}` sans `initialize` n'accepte que des arguments nommés (reçu {})", v.inspect()),
                );
            }
            for (name, v) in args.named {
                if !info_fields.iter().any(|f| f.name.name == name) {
                    return raise("ArgumentError", format!("état inconnu `@{name}` pour `{ty}`"));
                }
                set_field(&mut obj.fields.borrow_mut(), &name, v);
            }
        }
        Ok(value)
    }

    // ── Agents ───────────────────────────────────────────────

    pub(crate) fn spawn(&mut self, target: &Value<'p>) -> R<'p> {
        let Value::Type(name) = target else {
            return raise("TypeError", format!("`spawn` attend un type d'agent, reçu {}", target.inspect()));
        };
        let Some(info) = self.types.get(&**name) else {
            return raise("NameError", format!("agent inconnu `{name}`"));
        };
        if !info.is(TypeKind::Agent) {
            return raise("TypeError", format!("`{name}` n'est pas un agent"));
        }
        let ty: &'p str = info.def.name.name.as_str();
        let budget_directive = info.directives.iter().find(|d| d.name.name == "budget").copied();
        let agent = self.new_object(ty, Args::default(), true)?;
        if let (Some(directive), Value::Object(obj)) = (budget_directive, &agent) {
            let args = self.eval_args(&directive.args)?;
            let budget = builtins::budget_from_args(&args)?;
            self.agent_budgets.insert(Rc::as_ptr(obj) as usize, Rc::new(budget));
        }
        Ok(agent)
    }

    fn supervisor_child(&mut self, sup: &str, agent: &str) -> R<'p> {
        let Some(info) = self.types.get(sup).filter(|i| i.is(TypeKind::Supervisor)) else {
            return raise("TypeError", format!("`{sup}` n'est pas un superviseur"));
        };
        let declared = info.directives.iter().any(|d| {
            d.name.name == "child"
                && matches!(d.args.first(), Some(grenat_ast::Arg::Pos(Expr { kind: ExprKind::Const(p), .. }))
                    if p.last().is_some_and(|i| i.name == agent))
        });
        if !declared {
            return raise("NameError", format!("`{agent}` n'est pas un enfant de `{sup}`"));
        }
        let key = (sup.to_string(), agent.to_string());
        if let Some(existing) = self.children.get(&key) {
            return Ok(existing.clone());
        }
        let child = self.spawn(&Value::Type(agent.into()))?;
        self.children.insert(key, child.clone());
        Ok(child)
    }

    fn agent_ask(&mut self, agent: Rc<Object<'p>>, args: Args<'p>) -> R<'p> {
        let Some(message) = args.pos.into_iter().next() else {
            return raise("ArgumentError", "`ask` attend un message, par exemple `ask(Research(topic: t))`");
        };
        let Value::Record(record) = message.untainted().clone() else {
            return raise("TypeError", format!("message attendu, reçu {}", message.inspect()));
        };
        let info = &self.types[&*agent.ty];
        let Some(handler) = info.handlers.get(&*record.ty).copied() else {
            let known: Vec<_> = info.handlers.keys().copied().collect();
            return raise(
                "NoMethodError",
                format!("l'agent `{}` ne gère pas `{}` (messages : {})", agent.ty, record.ty, known.join(", ")),
            );
        };
        let mut call_args = Args::default();
        for (name, value) in &record.fields {
            if name.starts_with('_') && name[1..].chars().all(|c| c.is_ascii_digit()) {
                call_args.pos.push(value.clone());
            } else {
                call_args.named.push((name.to_string(), value.clone()));
            }
        }

        let budget = self.agent_budgets.get(&(Rc::as_ptr(&agent) as usize)).cloned();
        if let Some(b) = &budget {
            self.budgets.push(b.clone());
        }
        self.agents.push(AgentFrame { agent: agent.clone(), handler });
        let pushed = self.push_frame(Some(Value::Object(agent.clone())), new_scope(None));
        let result = pushed.and_then(|()| {
            let r = self
                .bind_params(&handler.params, call_args, &handler.message.name)
                .and_then(|()| self.eval_body(&handler.body));
            self.pop_frame();
            r
        });
        self.agents.pop();
        if budget.is_some() {
            self.budgets.pop();
        }
        match result {
            Ok(v) | Err(Ctrl::Return(v)) => Ok(v),
            Err(Ctrl::Raise(e)) => {
                e.trace.borrow_mut().push((format!("{}#{}", agent.ty, handler.message.name), handler.span));
                Err(Ctrl::Raise(e))
            }
            Err(other) => Err(other),
        }
    }

    // ── Budgets ──────────────────────────────────────────────

    pub(crate) fn within(&mut self, budget: Rc<Budget>, block: &Value<'p>) -> R<'p> {
        budget.started.set(std::time::Instant::now());
        self.budgets.push(budget);
        let result = self.call_block(block, Vec::new());
        self.budgets.pop();
        result
    }

    pub(crate) fn check_budgets(&self) -> Result<(), Ctrl<'p>> {
        for budget in self.budgets.iter().rev() {
            if let Some(reason) = budget.exceeded() {
                let mut error = ErrorVal::new("BudgetExceeded", format!("budget dépassé : {reason}"));
                error.fields.push(("spent".into(), Value::Money(budget.spent_usd.get())));
                error.fields.push(("tokens".into(), Value::Int(budget.tokens.get() as i64)));
                return Err(Ctrl::Raise(Rc::new(error)));
            }
        }
        Ok(())
    }

    // ── Humain dans la boucle ────────────────────────────────

    pub(crate) fn ask_human(&mut self, message: &str) -> Result<bool, Ctrl<'p>> {
        match self.approver.clone() {
            Some(Value::Symbol(policy)) => Ok(&*policy == "approve_all"),
            Some(handler) => {
                let request = Value::record("ApprovalRequest", vec![("message".into(), Value::str(message))]);
                Ok(self.call_block(&handler, vec![request])?.truthy())
            }
            None => {
                self.write_err(&format!("\n[approbation] {message}\nApprouver ? (o/N) "));
                let answer = self.read_line().unwrap_or_default();
                Ok(matches!(answer.trim().to_lowercase().as_str(), "o" | "oui" | "y" | "yes"))
            }
        }
    }
}

fn set_field<'p>(fields: &mut Fields<'p>, name: &str, value: Value<'p>) {
    match fields.iter_mut().find(|(n, _)| &**n == name) {
        Some((_, v)) => *v = value,
        None => fields.push((name.into(), value)),
    }
}

fn resolve_index(i: i64, len: usize) -> Option<usize> {
    let i = if i < 0 { i + len as i64 } else { i };
    (0..len as i64).contains(&i).then_some(i as usize)
}

fn int_op<'p>(op: BinOp, a: i64, b: i64) -> Option<R<'p>> {
    use BinOp::*;
    let overflow = || raise("OverflowError", "dépassement d'entier");
    let checked = |r: Option<i64>| Some(r.map_or_else(overflow, |n| Ok(Value::Int(n))));
    match op {
        Add => checked(a.checked_add(b)),
        Sub => checked(a.checked_sub(b)),
        Mul => checked(a.checked_mul(b)),
        Div | Rem if b == 0 => Some(raise("ZeroDivisionError", "division par zéro")),
        // division entière arrondie vers -∞, comme en Ruby
        Div => checked(a.checked_div(b).map(|q| if (a % b != 0) && ((a < 0) != (b < 0)) { q - 1 } else { q })),
        Rem => checked(Some(((a % b) + b) % b)),
        Pow if b >= 0 => checked(u32::try_from(b).ok().and_then(|b| a.checked_pow(b))),
        Pow => Some(Ok(Value::Float((a as f64).powf(b as f64)))),
        Lt => Some(Ok(Value::Bool(a < b))),
        Le => Some(Ok(Value::Bool(a <= b))),
        Gt => Some(Ok(Value::Bool(a > b))),
        Ge => Some(Ok(Value::Bool(a >= b))),
        Cmp => Some(Ok(Value::Int(a.cmp(&b) as i64))),
        BitAnd => Some(Ok(Value::Int(a & b))),
        BitOr => Some(Ok(Value::Int(a | b))),
        BitXor => Some(Ok(Value::Int(a ^ b))),
        Shl => checked(u32::try_from(b).ok().and_then(|b| a.checked_shl(b))),
        Shr => checked(u32::try_from(b).ok().and_then(|b| a.checked_shr(b))),
        _ => None,
    }
}

fn float_op<'p>(op: BinOp, a: f64, b: f64) -> Option<R<'p>> {
    use BinOp::*;
    Some(Ok(match op {
        Add => Value::Float(a + b),
        Sub => Value::Float(a - b),
        Mul => Value::Float(a * b),
        Div => Value::Float(a / b),
        Rem => Value::Float(a.rem_euclid(b)),
        Pow => Value::Float(a.powf(b)),
        Lt => Value::Bool(a < b),
        Le => Value::Bool(a <= b),
        Gt => Value::Bool(a > b),
        Ge => Value::Bool(a >= b),
        Cmp => Value::Int(a.partial_cmp(&b).map_or(0, |o| o as i64)),
        _ => return None,
    }))
}
