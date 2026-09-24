//! Liveness of the variables holding objects: where Perceus inserts its
//! `dup`s and `drop`s.
//!
//! Invariant: at every point of the function, the variables owning a
//! reference are exactly the live ones. A variable stops owning:
//! - at its last use, which *moves* the reference to its consumer;
//! - right after an assignment when the new value is never read;
//! - on entering a branch or leaving a loop where it is no longer live
//!   (the other path may still use it): the translator drops it there.
//!
//! The analysis walks the body backwards, in the exact evaluation order of
//! the translator; loops iterate to a fixed point.

use std::collections::{BTreeSet, HashMap};

use grenat_ast::{Arg, BinOp, Block, Expr, ExprKind, FnDef, StrSeg};

use crate::infer::{Method, Typed};

type Live = BTreeSet<String>;

/// A control-flow edge on which variables may die.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Edge {
    /// Into the `then` branch of an `if`.
    Then,
    /// Into the `else` branch (possibly absent) of an `if`.
    Else,
    /// Into the right operand of `&&`/`||`.
    Rhs,
    /// Past the right operand of `&&`/`||`.
    Skip,
    /// Into a loop body.
    Body,
    /// Out of a loop.
    Exit,
}

/// Where references move and die, keyed by address in the AST.
#[derive(Debug, Default)]
pub(crate) struct Plan {
    /// Reads of a variable: `true` for its last use (a move).
    moves: HashMap<*const Expr, bool>,
    /// Assignments whose variable is never read afterwards.
    dead_defs: HashMap<*const Expr, bool>,
    edges: HashMap<(*const Expr, Edge), Vec<String>>,
    /// Parameters of an inlined block never read by its body.
    dead_params: HashMap<*const Expr, Vec<String>>,
    /// Parameters of the function never read.
    pub entry: Vec<String>,
}

impl Plan {
    pub fn moves(&self, read: &Expr) -> bool {
        self.moves.get(&(read as *const Expr)).copied().unwrap_or(false)
    }

    pub fn dead_after(&self, assign: &Expr) -> bool {
        self.dead_defs.get(&(assign as *const Expr)).copied().unwrap_or(false)
    }

    pub fn drops(&self, e: &Expr, edge: Edge) -> &[String] {
        self.edges.get(&(e as *const Expr, edge)).map_or(&[], Vec::as_slice)
    }

    pub fn dead_params(&self, call: &Expr) -> &[String] {
        self.dead_params.get(&(call as *const Expr)).map_or(&[], Vec::as_slice)
    }
}

pub(crate) fn plan(def: &FnDef, typed: &Typed) -> Plan {
    let mut cx = Liveness { typed, plan: Plan::default() };
    let live = cx.stmts(&def.body.stmts, Live::new());
    cx.plan.entry =
        def.params.iter().map(|p| p.name.name.clone()).filter(|p| typed.is_heap_var(p) && !live.contains(p)).collect();
    cx.plan
}

struct Liveness<'a> {
    typed: &'a Typed,
    plan: Plan,
}

impl Liveness<'_> {
    fn stmts(&mut self, stmts: &[Expr], mut live: Live) -> Live {
        for stmt in stmts.iter().rev() {
            live = self.expr(stmt, live);
        }
        live
    }

    /// Evaluates `exprs` left to right after nothing, then `after`.
    fn sequence<'e>(&mut self, exprs: impl DoubleEndedIterator<Item = &'e Expr>, mut live: Live) -> Live {
        for e in exprs.rev() {
            live = self.expr(e, live);
        }
        live
    }

    fn read(&mut self, e: &Expr, name: &str, mut live: Live) -> Live {
        if self.typed.is_heap_var(name) {
            self.plan.moves.insert(e as *const Expr, !live.contains(name));
            live.insert(name.to_string());
        }
        live
    }

    fn define(&mut self, e: &Expr, name: &str, mut live: Live) -> Live {
        if self.typed.is_heap_var(name) {
            self.plan.dead_defs.insert(e as *const Expr, !live.contains(name));
            live.remove(name);
        }
        live
    }

    fn edge(&mut self, e: &Expr, edge: Edge, from: &Live, to: &Live) {
        self.plan.edges.insert((e as *const Expr, edge), from.difference(to).cloned().collect());
    }

    /// Variables live before `e`, given those live after it.
    fn expr(&mut self, e: &Expr, after: Live) -> Live {
        match &e.kind {
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::Const(_) => after,
            ExprKind::Str(segs) => self.sequence(
                segs.iter().filter_map(|s| match s {
                    StrSeg::Interp(e) => Some(e),
                    StrSeg::Lit(_) => None,
                }),
                after,
            ),
            ExprKind::Array(items) => self.sequence(items.iter(), after),
            ExprKind::Var(name) => self.read(e, name, after),
            ExprKind::Assign { target, value } => match &target.kind {
                ExprKind::Var(name) => {
                    let live = self.define(e, name, after);
                    self.expr(value, live)
                }
                // value, then array, then index
                ExprKind::Index { recv, args } => {
                    let live = self.sequence(args.iter(), after);
                    let live = self.expr(recv, live);
                    self.expr(value, live)
                }
                _ => unreachable!("rejected by infer"),
            },
            ExprKind::OpAssign { target, value, .. } => match &target.kind {
                // read, value, assignment
                ExprKind::Var(name) => {
                    let live = self.define(e, name, after);
                    let live = self.expr(value, live);
                    self.read(target, name, live)
                }
                // array, index, value, store
                ExprKind::Index { recv, args } => {
                    let live = self.expr(value, after);
                    let live = self.sequence(args.iter(), live);
                    self.expr(recv, live)
                }
                _ => unreachable!("rejected by infer"),
            },
            ExprKind::Binary { op: BinOp::And | BinOp::Or, lhs, rhs } => {
                let rhs_live = self.expr(rhs, after.clone());
                let decided: Live = rhs_live.union(&after).cloned().collect();
                self.edge(e, Edge::Rhs, &decided, &rhs_live);
                self.edge(e, Edge::Skip, &decided, &after);
                self.expr(lhs, decided)
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                let live = self.expr(rhs, after);
                self.expr(lhs, live)
            }
            ExprKind::Unary { expr, .. } => self.expr(expr, after),
            ExprKind::If { cond, then, else_ } => {
                let then_live = self.stmts(then, after.clone());
                let else_live = match else_ {
                    Some(stmts) => self.stmts(stmts, after),
                    None => after,
                };
                let decided: Live = then_live.union(&else_live).cloned().collect();
                self.edge(e, Edge::Then, &decided, &then_live);
                self.edge(e, Edge::Else, &decided, &else_live);
                self.expr(cond, decided)
            }
            ExprKind::While { cond, body } => self.fixed_point(&after, |cx, header| {
                let body_live = cx.stmts(body, header.clone());
                let decided: Live = body_live.union(&after).cloned().collect();
                cx.edge(e, Edge::Body, &decided, &body_live);
                cx.edge(e, Edge::Exit, &decided, &after);
                cx.expr(cond, decided)
            }),
            ExprKind::Return(value) => self.expr(value.as_deref().expect("checked by infer"), Live::new()),
            ExprKind::Index { recv, args } => {
                let live = self.sequence(args.iter(), after);
                self.expr(recv, live)
            }
            ExprKind::Call { recv, args, block, .. } => {
                let live = match block {
                    Some(block) => self.inlined(e, block, after),
                    None => after,
                };
                let live = self.sequence(
                    args.iter().map(|a| match a {
                        Arg::Pos(e) | Arg::Named { value: Some(e), .. } => e,
                        _ => unreachable!("rejected by infer"),
                    }),
                    live,
                );
                match recv {
                    Some(recv) => self.expr(recv, live),
                    None => live,
                }
            }
            other => unreachable!("rejected by infer: {other:?}"),
        }
    }

    /// An inlined block: a loop whose body starts by binding the parameters.
    fn inlined(&mut self, call: &Expr, block: &Block, after: Live) -> Live {
        let params = &self.typed.blocks[&(call as *const Expr)].params;
        debug_assert!(matches!(
            self.typed.method(call),
            Method::Times | Method::Upto | Method::Each | Method::EachWithIndex
        ));
        self.fixed_point(&after, |cx, header| {
            let body_live = cx.stmts(&block.body.stmts, header.clone());
            let dead = params.iter().filter(|p| cx.typed.is_heap_var(p) && !body_live.contains(*p)).cloned().collect();
            cx.plan.dead_params.insert(call as *const Expr, dead);
            let entry: Live = body_live.iter().filter(|v| !params.contains(v)).cloned().collect();
            let decided: Live = entry.union(&after).cloned().collect();
            cx.edge(call, Edge::Body, &decided, &entry);
            cx.edge(call, Edge::Exit, &decided, &after);
            decided
        })
    }

    /// Live set at a loop header: `step` computes it from a guess of itself
    /// (the back edge); iterate until it no longer changes. Annotations are
    /// overwritten at each iteration, so the last one leaves the right ones.
    fn fixed_point(&mut self, after: &Live, mut step: impl FnMut(&mut Self, &Live) -> Live) -> Live {
        let mut header = after.clone();
        loop {
            let next = step(self, &header);
            if next == header {
                return header;
            }
            header = next;
        }
    }
}
