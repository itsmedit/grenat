//! Function, method and block calls; parameter binding; methods of tainted values.

use crate::prelude::*;

/// Effect that forbids receiving a tainted value.
fn dangerous_effect(def: &FnDef) -> Option<String> {
    def.effects.iter().find_map(|effect| {
        let path: Vec<&str> = effect.path.iter().map(|i| i.name.as_str()).collect();
        let name = path.join(".");
        matches!(name.as_str(), "shell" | "net" | "fs.write" | "human").then_some(name)
    })
}

impl<'p> Interp<'p> {
    // ── Calls ───────────────────────────────────────────────

    pub(crate) fn eval_call(
        &mut self,
        recv: &'p Option<Box<Expr>>,
        name: &'p Ident,
        args: &'p [grenat_ast::Arg],
        block: &'p Option<Box<Block>>,
        safe: bool,
    ) -> R<'p> {
        self.check_cancel()?;
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
            // `break` in a block ends the call that received it
            Err(Ctrl::Break(v)) if block.is_some() => Ok(v),
            other => other,
        }
    }

    pub(crate) fn eval_args(&mut self, args: &'p [grenat_ast::Arg]) -> Result<Args<'p>, Ctrl<'p>> {
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

    pub(crate) fn make_closure(&self, block: &'p Block) -> Value<'p> {
        Value::Closure(Arc::new(Closure {
            params: block.params.iter().map(|p| p.name.name.clone()).collect(),
            body: &block.body,
            scope: self.scope().clone(),
            self_val: self.self_val(),
            taint_args: false,
        }))
    }

    pub(crate) fn call_block(&mut self, block: &Value<'p>, args: Vec<Value<'p>>) -> R<'p> {
        self.check_cancel()?;
        match block {
            Value::Closure(c) => {
                let scope = new_scope(Some(c.scope.clone()));
                // `|k, v|` on a pair: destructuring, as in Ruby
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
            other => raise("TypeError", format!("expected a block, got {}", other.type_name())),
        }
    }

    /// A call without a receiver: a constructor, `run` in an agent, a method
    /// of `self`, a function, a built-in.
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
        if let Some((def, ty)) = self.sibling_static(name) {
            return self.call_fn(def, args, Some(Value::Type(ty)));
        }
        if let Some(def) = self.fns.get(name).copied() {
            return self.call_fn(def, args, None);
        }
        if let Some(result) = builtins::call_global(self, name, args) {
            return result;
        }
        raise("NameError", format!("unknown function `{name}`"))
    }

    /// In a `def self.x`, another `def self.` of the same type, called
    /// without a receiver as in Ruby (`self` is the type); and that type.
    pub(crate) fn sibling_static(&self, name: &str) -> Option<(&'p FnDef, Arc<str>)> {
        let Some(Value::Type(ty)) = self.self_val() else { return None };
        let def = self.types.get(&*ty).and_then(|info| info.statics.get(name).copied())?;
        Some((def, ty))
    }

    pub(crate) fn in_current_agent(&self) -> bool {
        match (self.agents.last(), self.self_val()) {
            (Some(frame), Some(Value::Object(obj))) => Arc::ptr_eq(&frame.agent, &obj),
            _ => false,
        }
    }

    pub(crate) fn method_of(&self, receiver: &Value<'p>, name: &str) -> Option<&'p FnDef> {
        let ty: &str = match receiver.untainted() {
            Value::Record(r) => &r.ty,
            Value::Object(o) => &o.ty,
            Value::Variant(v) => &v.enum_name,
            _ => return None,
        };
        self.types.get(ty).and_then(|info| info.methods.get(name).copied())
    }

    pub(crate) fn call_fn(&mut self, def: &'p FnDef, args: Args<'p>, self_val: Option<Value<'p>>) -> R<'p> {
        if def.is_abstract {
            return raise("NotImplementedError", format!("`{}` is abstract", def.name.name));
        }
        if args.block.is_some() {
            return raise("ArgumentError", format!("`{}` does not take a block", def.name.name));
        }
        if self_val.is_none()
            && let Some(result) = self.call_native(def, &args)
        {
            return result;
        }
        if let Some(effect) = dangerous_effect(def) {
            let tainted = args
                .pos
                .iter()
                .chain(args.named.iter().map(|(_, v)| v))
                .chain(self_val.iter())
                .any(Value::contains_taint);
            if tainted {
                return raise(
                    "TaintError",
                    format!(
                        "an untrusted value reaches `{}` (effect `{effect}`) without validation; \
                         validate it with `.check {{ … }}`, `.approve(by: :human)` or `.trust!`",
                        def.name.name
                    ),
                );
            }
        }
        let caps = self.declared_capabilities(def)?;
        let workflow = if def.kind == FnKind::Workflow { Some(self.open_workflow(def, &args)?) } else { None };
        self.push_frame(self_val, new_scope(None))?;
        if let Some(caps) = caps {
            self.capabilities.push((def.name.name.clone(), caps));
        }
        let result = self.bind_params(&def.params, args, &def.name.name).and_then(|()| match (def.kind, workflow) {
            (FnKind::Prompt, _) => self.run_prompt(def),
            (FnKind::Workflow, Some(run)) => self.run_workflow(def, run),
            _ => self.eval_body(&def.body),
        });
        if !def.effects.is_empty() {
            self.capabilities.pop();
        }
        self.pop_frame();
        match result {
            Ok(v) | Err(Ctrl::Return(v)) => Ok(v),
            Err(Ctrl::Raise(e)) => {
                e.trace.borrow_mut().push((def.name.name.clone(), def.span));
                Err(Ctrl::Raise(e))
            }
            Err(Ctrl::Break(_) | Ctrl::Next(_)) => {
                raise("LocalJumpError", format!("`break`/`next` outside a loop in `{}`", def.name.name))
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
                return raise("ArgumentError", format!("missing argument `{}` for `{owner}`", param.name.name));
            };
            scope_define(self.scope(), &param.name.name, value);
        }
        if pos.next().is_some() {
            return raise("ArgumentError", format!("too many arguments for `{owner}` (expected {})", params.len()));
        }
        if let Some((name, _)) = named.first() {
            return raise("ArgumentError", format!("unknown named argument `{name}:` for `{owner}`"));
        }
        Ok(())
    }

    pub(crate) fn call_method(&mut self, receiver: Value<'p>, name: &str, args: Args<'p>) -> R<'p> {
        // a stored record's `save` and `delete` (unless the struct has its own)
        if matches!(name, "save" | "delete")
            && self.method_of(receiver.untainted(), name).is_none()
            && let Some(result) = self.record_instance(&receiver, name)
        {
            return result;
        }
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
        if matches!(receiver, Value::Agent(_) | Value::Pool(_)) && matches!(name, "ask" | "tell") {
            return self.send(&receiver, name, args).expect("agent or pool");
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

    /// Methods of a `~T` value: validation, otherwise taint propagation.
    pub(crate) fn tainted_method(
        &mut self,
        receiver: Value<'p>,
        inner: Value<'p>,
        name: &str,
        mut args: Args<'p>,
    ) -> R<'p> {
        match name {
            "trust!" => Ok(inner),
            "tainted?" => Ok(Value::Bool(true)),
            "inspect" => Ok(Value::str(receiver.inspect())),
            "check" => {
                let Some(block) = args.block else {
                    return raise("ArgumentError", "`check` expects a validation block");
                };
                if self.call_block(&block, vec![inner.clone()])?.truthy() {
                    Ok(Value::ok(inner))
                } else {
                    let error = ErrorVal::new("CheckError", format!("validation failed for {}", inner.inspect()));
                    Ok(Value::err(Value::Error(Arc::new(error))))
                }
            }
            "approve" => {
                let message = format!("LLM-produced value:\n{}", inner.inspect());
                if self.ask_human(&message)? {
                    Ok(inner)
                } else {
                    raise("ApprovalDenied", "value rejected by the human")
                }
            }
            // user method: `self` stays tainted, and so do its fields
            _ if self.method_of(&inner, name).is_some() => {
                let def = self.method_of(&inner, name).expect("checked above");
                self.call_fn(def, args, Some(receiver))
            }
            _ => {
                if let Some(Value::Closure(c)) = &args.block {
                    args.block = Some(Value::Closure(Arc::new(Closure {
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

    pub(crate) fn static_method(&mut self, ty: &str, name: &str, args: Args<'p>) -> R<'p> {
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
        if let Some(result) = self.record_static(ty, name, &args) {
            return result;
        }
        builtins::call_static(self, ty, name, args)
    }
}
