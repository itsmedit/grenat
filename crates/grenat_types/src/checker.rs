//! The checker: global state and diagnostic reporting.

use std::collections::{HashMap, HashSet};

use grenat_ast::{Diagnostic, Directive, Field, FnDef, Handler, Member, Program, Span, TypeDef, Variant};

use crate::ty::{Ty, V};
use crate::*;

pub(crate) struct TypeDecl<'p> {
    pub(crate) def: &'p TypeDef,
    pub(crate) fields: Vec<&'p Field>,
    pub(crate) ivars: Vec<&'p Field>,
    pub(crate) methods: HashMap<&'p str, &'p FnDef>,
    pub(crate) statics: HashMap<&'p str, &'p FnDef>,
    pub(crate) variants: Vec<&'p Variant>,
    pub(crate) handlers: HashMap<&'p str, &'p Handler>,
    pub(crate) directives: Vec<&'p Directive>,
    pub(crate) includes: Vec<&'p str>,
}

impl<'p> TypeDecl<'p> {
    pub(crate) fn new(def: &'p TypeDef) -> Self {
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

pub(crate) struct Checker<'p> {
    pub(crate) program: &'p Program,
    pub(crate) fns: HashMap<&'p str, &'p FnDef>,
    pub(crate) types: HashMap<&'p str, TypeDecl<'p>>,
    pub(crate) variants: HashMap<&'p str, &'p str>,
    /// Message → agents that handle it.
    pub(crate) messages: HashMap<&'p str, Vec<(&'p str, &'p Handler)>>,
    pub(crate) models: Vec<&'p str>,
    pub(crate) diags: Vec<Diagnostic>,
    pub(crate) seen: HashSet<(u32, u32, String)>,
    pub(crate) memo: HashMap<Key, (V, Vec<Eff>)>,
    pub(crate) in_progress: HashSet<Key>,
    pub(crate) prev_effects: HashMap<usize, Vec<Eff>>,
    /// `@…` state that received a tainted value: (type, name) → origin.
    pub(crate) ivar_taint: HashMap<(String, String), Span>,
    /// Type of unannotated `@…` state, inferred from its initial value.
    pub(crate) ivar_types: HashMap<(String, String), Ty>,
}

impl<'p> Checker<'p> {
    pub(crate) fn new(program: &'p Program) -> Self {
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

    pub(crate) fn report(&mut self, diag: Diagnostic) {
        if self.seen.insert((diag.span.start, diag.span.end, diag.message.clone())) {
            self.diags.push(diag);
        }
    }

    pub(crate) fn error(&mut self, code: &'static str, span: Span, message: impl Into<String>) {
        self.report(Diagnostic::new(span, message).with_code(code));
    }

    pub(crate) fn error_help(
        &mut self,
        code: &'static str,
        span: Span,
        message: impl Into<String>,
        help: Option<String>,
    ) {
        let mut diag = Diagnostic::new(span, message).with_code(code);
        diag.help = help;
        self.report(diag);
    }
}
