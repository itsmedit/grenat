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
            if let Some(sink @ ("fs.write" | "net")) = effect {
                for arg in &argv {
                    if let Some(origin) = arg.v.taint {
                        self.taint_violation(arg.span, origin, &format!("{t}.{n}"), sink);
                    }
                }
            }
            let _ = block_v;
            // what a pure function makes of untrusted data is untrusted
            let taint = if effect.is_none() { argv.iter().find_map(|a| a.v.taint) } else { None };
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
}
