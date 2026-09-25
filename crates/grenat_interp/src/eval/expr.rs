//! Evaluation of statements and expressions, name resolution.

use crate::prelude::*;

/// Built-in modules and types usable as values.
const BUILTIN_TYPES: &[&str] = &[
    "File", "Dir", "Math", "Env", "Json", "Runtime", "Cli", "Time", "Http", "Db", "Shell", "Mcp", "Pdf", "Image", "Mail", "Int", "Float", "String", "Bool", "Array", "Hash",
    "Symbol", "Nil", "Range", "Money", "Duration",
];

impl<'p> Interp<'p> {
    // ── Context ─────────────────────────────────────────────

    pub(crate) fn scope(&self) -> &crate::value::Scope<'p> {
        &self.frames.last().expect("at least one frame").scope
    }

    pub(crate) fn self_val(&self) -> Option<Value<'p>> {
        self.frames.last().and_then(|f| f.self_val.clone())
    }

    pub(crate) fn self_object(&self, ivar: &str) -> Result<Arc<Object<'p>>, Ctrl<'p>> {
        match self.self_val() {
            Some(Value::Object(o)) => Ok(o),
            _ => raise("NameError", format!("`@{ivar}` used outside a class or an agent")),
        }
    }

    pub(crate) fn push_frame(
        &mut self,
        self_val: Option<Value<'p>>,
        scope: crate::value::Scope<'p>,
    ) -> Result<(), Ctrl<'p>> {
        if self.depth >= self.max_depth || self.stack.exceeded() {
            return raise("StackOverflow", "recursion too deep");
        }
        self.depth += 1;
        self.frames.push(Frame { self_val, scope });
        Ok(())
    }

    pub(crate) fn pop_frame(&mut self) {
        self.depth -= 1;
        self.frames.pop();
    }

    // ── Statements ─────────────────────────────────────────

    pub(crate) fn eval_stmts(&mut self, stmts: &'p [Expr]) -> R<'p> {
        let mut last = Value::Nil;
        for stmt in stmts {
            self.check_cancel()?;
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
                && err.span().is_none()
            {
                err.set_span(e.span);
            }
        })
    }

    pub(crate) fn eval_kind(&mut self, e: &'p Expr) -> R<'p> {
        match &e.kind {
            ExprKind::Int(n) => Ok(Value::Int(*n)),
            ExprKind::Float(f) => Ok(Value::Float(*f)),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            ExprKind::Nil => Ok(Value::Nil),
            ExprKind::Symbol(s) => Ok(Value::Symbol(s.as_str().into())),
            ExprKind::Str(segs) => self.string(segs),
            ExprKind::SelfRef => match self.self_val() {
                Some(v) => Ok(v),
                None => raise("NameError", "`self` used outside a method"),
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
                Ok(Value::Hash(Arc::new(Mutex::new(pairs))))
            }
            ExprKind::Range { lo, hi, inclusive } => {
                let (lo, hi) = (self.eval(lo)?, self.eval(hi)?);
                match (lo.untainted(), hi.untainted()) {
                    (Value::Int(a), Value::Int(b)) => Ok(Value::Range(*a, *b, *inclusive)),
                    _ => raise("TypeError", "range bounds must be integers"),
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
                                None => return raise("OverflowError", "integer overflow"),
                            },
                            Value::Float(f) => Value::Float(-f),
                            Value::Money(m) => Value::Money(-m),
                            other => {
                                return raise("TypeError", format!("`-` is not defined for {}", other.type_name()));
                            }
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
                    self.check_cancel()?;
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

    pub(crate) fn eval_opt(&mut self, e: Option<&'p Expr>) -> R<'p> {
        e.map_or(Ok(Value::Nil), |e| self.eval(e))
    }

    pub(crate) fn string(&mut self, segs: &'p [StrSeg]) -> R<'p> {
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

    /// `to_s`, honouring a user-defined `to_s` method.
    pub(crate) fn display(&mut self, v: &Value<'p>) -> Result<String, Ctrl<'p>> {
        if let Some(def) = self.method_of(v.untainted(), "to_s") {
            return Ok(self.call_fn(def, Args::default(), Some(v.untainted().clone()))?.to_display());
        }
        Ok(v.to_display())
    }

    pub(crate) fn try_unwrap(&mut self, value: Value<'p>) -> R<'p> {
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
            Value::Nil => raise("NilError", "`?` applied to nil"),
            _ => Ok(value),
        }
    }

    pub(crate) fn lookup_var(&mut self, name: &str) -> R<'p> {
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
        if let Some((def, ty)) = self.sibling_static(name) {
            return self.call_fn(def, Args::default(), Some(Value::Type(ty)));
        }
        if let Some(def) = self.fns.get(name).copied() {
            return self.call_fn(def, Args::default(), None);
        }
        if let Some(result) = builtins::call_global(self, name, Args::default()) {
            return result;
        }
        // `r?`: the lexer reads a predicate name; on a variable, it is the `?` operator
        if let Some(base) = name.strip_suffix('?')
            && let Some(value) = scope_get(self.scope(), base)
        {
            return self.try_unwrap(value);
        }
        raise("NameError", format!("unknown variable or function `{name}`"))
    }

    /// Field of `self`; tainted if `self` is (method called on a `~T` value).
    pub(crate) fn self_field(&self, receiver: &Value<'p>, name: &str) -> Option<Value<'p>> {
        let found = match receiver.untainted() {
            Value::Record(r) => field(&r.fields, name).cloned(),
            Value::Variant(v) => field(&v.fields, name).cloned(),
            _ => None,
        }?;
        Some(if receiver.is_tainted() { found.taint() } else { found })
    }

    pub(crate) fn resolve_const(&mut self, path: &'p [Ident]) -> R<'p> {
        let name = path.last().expect("non-empty path").name.as_str();
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
        raise("NameError", format!("unknown constant `{name}`"))
    }

    /// Field-less variant (`Positive`); a variant with fields is built with `Name(…)`.
    pub(crate) fn variant_value(&mut self, name: &str) -> R<'p> {
        let enum_name = self.variants[name];
        let has_fields = self.types[enum_name].variants.iter().any(|v| v.name.name == name && !v.fields.is_empty());
        if has_fields {
            return Ok(Value::Type(name.into()));
        }
        Ok(Value::Variant(Arc::new(Variant { enum_name: enum_name.into(), name: name.into(), fields: Vec::new() })))
    }
}
