//! Checking context of a function: scopes, `self`, collected effects and returns.

use std::collections::HashMap;

use grenat_ast::{Field, FnDef, Handler, Param, Span, Type};

use crate::ty::{Ty, V};
use crate::*;

/// Parameter, field or variant: what a call must bind.
#[derive(Clone, Copy)]
pub(crate) struct Slot<'p> {
    pub(crate) name: &'p str,
    pub(crate) ty: Option<&'p Type>,
    pub(crate) optional: bool,
}

impl<'p> Slot<'p> {
    pub(crate) fn params(params: &'p [Param]) -> Vec<Slot<'p>> {
        params.iter().map(|p| Slot { name: &p.name.name, ty: p.ty.as_ref(), optional: p.default.is_some() }).collect()
    }

    pub(crate) fn fields(fields: &[&'p Field]) -> Vec<Slot<'p>> {
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
pub(crate) enum Kind<'p> {
    Top,
    Fn(&'p FnDef),
    Handler(&'p str, &'p Handler),
}

/// Context of the function being checked.
pub(crate) struct Ctx<'p> {
    pub(crate) scopes: Vec<HashMap<String, V>>,
    pub(crate) self_ty: Option<Ty>,
    pub(crate) self_taint: Option<Span>,
    pub(crate) kind: Kind<'p>,
    pub(crate) effects: Vec<Eff>,
    pub(crate) returns: Vec<V>,
    pub(crate) run_span: Option<Span>,
    /// Depth inside `step { … }` blocks.
    pub(crate) steps: usize,
    /// Non-deterministic effects used outside any `step` (an error in a workflow).
    pub(crate) unstepped: Vec<Eff>,
}

impl<'p> Ctx<'p> {
    pub(crate) fn new(kind: Kind<'p>, self_ty: Option<Ty>, self_taint: Option<Span>) -> Self {
        Ctx {
            scopes: vec![HashMap::new()],
            self_ty,
            self_taint,
            kind,
            effects: Vec::new(),
            returns: Vec::new(),
            run_span: None,
            steps: 0,
            unstepped: Vec::new(),
        }
    }

    pub(crate) fn lookup(&self, name: &str) -> Option<V> {
        self.scopes.iter().rev().find_map(|s| s.get(name).cloned())
    }

    pub(crate) fn assign(&mut self, name: &str, value: V) {
        match self.scopes.iter_mut().rev().find(|s| s.contains_key(name)) {
            Some(scope) => {
                let merged = scope[name].taint.or(value.taint);
                scope.insert(name.to_string(), V { ty: value.ty, taint: merged });
            }
            None => self.define(name, value),
        }
    }

    pub(crate) fn define(&mut self, name: &str, value: V) {
        self.scopes.last_mut().expect("scope").insert(name.to_string(), value);
    }

    pub(crate) fn add_effect(&mut self, effect: Eff) {
        if self.steps == 0 && NONDETERMINISTIC.contains(&effect.path.as_str()) {
            self.unstepped.push(effect.clone());
        }
        if !self.effects.iter().any(|e| e.path == effect.path && e.arg == effect.arg) {
            self.effects.push(effect);
        }
    }

    pub(crate) fn names(&self) -> Vec<String> {
        self.scopes.iter().flat_map(|s| s.keys().cloned()).collect()
    }
}

pub(crate) struct ArgV {
    pub(crate) name: Option<String>,
    pub(crate) v: V,
    pub(crate) span: Span,
    /// String literal without interpolation (effect restriction).
    pub(crate) lit: Option<String>,
}

pub(crate) type Key = (usize, Vec<bool>, bool);
