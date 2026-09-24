//! Imports communs aux modules de l'interpréteur.

pub(crate) use std::cmp::Ordering;
pub(crate) use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
pub(crate) use std::sync::{Arc, Mutex};
pub(crate) use std::time::Instant;

pub(crate) use grenat_ast::{
    ArmTest, BinOp, Block, Body, Expr, ExprKind, FnDef, FnKind, Ident, Pattern, PatternKind, StrSeg, TypeKind, UnOp,
};

pub(crate) use crate::eval::*;
pub(crate) use crate::value::{
    AgentRef, Budget, Closure, ErrorVal, Fields, Locked, Object, Strategy, Supervision, Value, Variant, equal, field,
    new_scope, scope_define, scope_get, scope_set,
};
pub(crate) use crate::{AgentFrame, Args, Ctrl, Frame, Interp, R, TypeInfo, builtins, llm, raise};
