//! Method calls: declared types, built-in values, modules.

use grenat_ast::{Block, FnDef, Ident, Span, TypeKind};

use crate::ty::{Ty, V};
use crate::*;

impl<'p> Checker<'p> {
    pub(crate) fn method_def(&self, ty: &Ty, name: &str) -> Option<&'p FnDef> {
        match ty.base() {
            Ty::User(t) => self.types.get(t.as_str()).and_then(|d| d.methods.get(name).copied()),
            Ty::Type(t) => self.types.get(t.as_str()).and_then(|d| d.statics.get(name).copied()),
            _ => None,
        }
    }

    /// Struct field (or a field shared by enum variants).
    pub(crate) fn field_of(&self, ty: &Ty, name: &str) -> Option<Ty> {
        let Ty::User(t) = ty.base() else { return None };
        let decl = self.types.get(t.as_str())?;
        let field = match decl.def.kind {
            TypeKind::Struct => decl.fields.iter().find(|f| f.name.name == name).copied(),
            TypeKind::Enum => decl.variants.iter().flat_map(|v| v.fields.iter()).find(|f| f.name.name == name),
            _ => None,
        }?;
        Some(field.ty.as_ref().map_or(Ty::Unknown, |t| self.peek_ty(t)))
    }

    pub(crate) fn method(
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
        if secrets::is_secret(&recv.ty) {
            return self.secret_method(name.span, n);
        }
        // taint control
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
                        self.error(E_TYPE, name.span, format!("unknown method `{n}` for `{base}`"));
                        V { ty: Ty::Unknown, taint }
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn user_type_method(
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
        if let Some(v) = self.record_instance(cx, span, &recv, t, n) {
            return v;
        }
        if let Some(def) = self.method_def(&recv.ty, n) {
            return self.user_method(cx, span, recv, def, argv, block);
        }
        let kind = self.types.get(t).map(|d| d.def.kind);
        if kind == Some(TypeKind::Agent) && matches!(n, "ask" | "tell") {
            let v = self.ask(cx, span, t, argv);
            return if n == "tell" { V::new(Ty::Nil) } else { v };
        }
        if kind.is_none()
            && let Some((ty, effect)) = builtins::record_method(t, n)
        {
            return self.record_method(cx, span, (ty, effect), (t, n), &argv, block);
        }
        if argv.is_empty() && block.is_none() {
            if let Some(ty) = self.field_of(&recv.ty, n) {
                return V { ty, taint };
            }
            if kind == Some(TypeKind::Enum) && n == "name" {
                return V { ty: Ty::Str, taint };
            }
            if kind.is_none()
                && let Some((ty, untrusted)) = builtins::record_field(t, n)
            {
                return V { ty, taint: if untrusted { taint.or(Some(span)) } else { taint } };
            }
            if kind.is_none() && is_error_name(t) {
                // built-in or undeclared errors: free-form fields
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
                                format!("unknown field `{field}:` for `{t}`"),
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
            "method"
        } else {
            "field or method"
        };
        self.error_help(E_TYPE, name.span, format!("unknown {what} `{n}` for `{t}`"), suggest(n, candidates));
        V { ty: Ty::Unknown, taint }
    }

    pub(crate) fn static_call(
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
        if let Some(v) = self.record_static(cx, span, t, n, &argv) {
            return v;
        }
        if let Some((ty, effect)) = builtins::static_method(t, n) {
            let block_v = self.walk_block(cx, block, &[]);
            if let Some(path) = effect {
                let mut arg = argv.first().and_then(|a| a.lit.clone());
                if path == "net" {
                    // `net` is restricted by host: that of a literal URL
                    arg = arg.and_then(|url| crate::effects::url_host(&url).map(str::to_string));
                }
                cx.add_effect(Eff { path: path.into(), arg, origin: span });
            }
            if let Some(sink @ ("fs.write" | "net" | "shell" | "mcp")) = effect {
                for arg in &argv {
                    if let Some(origin) = arg.v.taint {
                        self.taint_violation(arg.span, origin, &format!("{t}.{n}"), sink);
                    }
                }
            }
            let _ = block_v;
            // what a pure function makes of untrusted data is untrusted
            let taint = if builtins::sanitizes(t, n) {
                None
            } else if builtins::untrusted_result(t, n) {
                Some(span)
            } else if effect.is_none() {
                argv.iter().find_map(|a| a.v.taint)
            } else {
                None
            };
            return V { ty, taint };
        }
        self.walk_block(cx, block, &[]);
        let mut candidates: Vec<&str> = Vec::new();
        if let Some(decl) = self.types.get(t) {
            candidates.extend(decl.statics.keys().copied());
            candidates.push("new");
        }
        self.error_help(E_TYPE, name.span, format!("unknown method `{t}.{n}`"), suggest(n, candidates));
        V::unknown()
    }

    /// A method of a built-in record (a database): SQL text is never
    /// untrusted, written values neither; `as: T` types the rows.
    fn record_method(
        &mut self,
        cx: &mut Ctx<'p>,
        span: Span,
        (ty, effect): (Ty, &'static str),
        (record, name): (&str, &str),
        argv: &[ArgV],
        block: Option<&'p Block>,
    ) -> V {
        let block_v = self.walk_block(cx, block, &[]);
        cx.add_effect(Eff { path: effect.into(), arg: None, origin: span });
        // an email: nothing untrusted reaches people
        if record == builtins::MAILER {
            for arg in argv {
                if let Some(origin) = arg.v.taint {
                    self.taint_violation(arg.span, origin, name, effect);
                }
            }
            return V::new(ty);
        }
        let positional: Vec<&ArgV> = argv.iter().filter(|a| a.name.is_none()).collect();
        if record == builtins::DATABASE
            && let Some(sql) = positional.first()
            && let Some(origin) = sql.v.taint
        {
            self.taint_violation(sql.span, origin, name, effect);
        }
        if record == builtins::DATABASE
            && effect == "db.write"
            && let Some(values) = positional.get(1)
            && let Some(origin) = values.v.taint
        {
            self.taint_violation(values.span, origin, name, effect);
        }
        let record_type = argv.iter().find(|a| a.name.as_deref() == Some("as")).and_then(|a| match &a.v.ty {
            Ty::Type(t) => Some(Ty::User(t.clone())),
            _ => None,
        });
        let taint = builtins::untrusted_method(record, name).then_some(span);
        let ty = match (name, record_type) {
            ("query", Some(t)) => Ty::array(t),
            ("first", Some(t)) => Ty::opt(t),
            ("transaction", _) => block_v.map_or(ty, |v| v.ty),
            _ => ty,
        };
        V { ty, taint }
    }

    /// Whether the struct `t` is stored in a table (`table :tickets`).
    pub(crate) fn is_record(&self, t: &str) -> bool {
        self.types.get(t).is_some_and(|d| d.directives.iter().any(|d| d.name.name == "table"))
    }

    /// `Ticket.all/where/find/count/create`: typed, with their effects; a
    /// record created from untrusted values is refused.
    fn record_static(&mut self, cx: &mut Ctx<'p>, span: Span, t: &str, n: &str, argv: &[ArgV]) -> Option<V> {
        if !self.is_record(t) {
            return None;
        }
        let record = Ty::User(t.into());
        let (ty, effect) = match n {
            "all" | "where" => (Ty::array(record), "db.read"),
            "find" => (Ty::opt(record), "db.read"),
            "count" => (Ty::Int, "db.read"),
            "create" => (record, "db.write"),
            _ => return None,
        };
        cx.add_effect(Eff { path: effect.into(), arg: None, origin: span });
        if effect == "db.write" {
            for arg in argv {
                if let Some(origin) = arg.v.taint {
                    self.taint_violation(arg.span, origin, &format!("{t}.create"), effect);
                }
            }
        }
        Some(V::new(ty))
    }

    /// `record.save` and `record.delete`: writes.
    pub(crate) fn record_instance(&mut self, cx: &mut Ctx<'p>, span: Span, recv: &V, t: &str, n: &str) -> Option<V> {
        if !matches!(n, "save" | "delete") || !self.is_record(t) || self.method_def(&recv.ty, n).is_some() {
            return None;
        }
        cx.add_effect(Eff { path: "db.write".into(), arg: None, origin: span });
        if n == "save"
            && let Some(origin) = recv.taint
        {
            self.taint_violation(span, origin, "save", "db.write");
        }
        Some(if n == "save" { V::new(recv.ty.clone()) } else { V::new(Ty::Nil) })
    }
}
