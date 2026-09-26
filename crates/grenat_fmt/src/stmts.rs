//! Statements, bodies and control flow, patterns, types and parameters.

use grenat_ast::{ArmTest, Body, CaseArm, Expr, ExprKind, Param, Pattern, PatternKind, Rescue, Type, UnOp};

use crate::printer::Printer;

impl Printer<'_> {
    /// Statements, one per line, each with its comments.
    pub(crate) fn stmts(&mut self, stmts: &[Expr]) {
        for stmt in stmts {
            self.line_element(stmt.span.start);
            self.stmt(stmt);
            self.line_end(stmt.span.end);
            self.newline();
        }
    }

    /// Statements of a block, indented, with the comments left before `end`
    /// (the next clause, or the closing `end`).
    pub(crate) fn block_stmts(&mut self, stmts: &[Expr], end: u32) {
        self.indented(|p| {
            p.stmts(stmts);
            p.comments_before(end);
        });
    }

    /// A body ending at `end`: its statements indented, then `rescue` and
    /// `ensure` clauses.
    pub(crate) fn body(&mut self, body: &Body, end: u32) {
        let ensure_at = body.ensure.as_ref().map(|ensure| {
            let before = ensure.first().map_or(end, |s| s.span.start) as usize;
            self.src[..before].rfind("ensure").unwrap_or(before) as u32
        });
        let after_stmts = body.rescues.first().map(|r| r.span.start).or(ensure_at).unwrap_or(end);
        self.block_stmts(&body.stmts, after_stmts);
        for (i, rescue) in body.rescues.iter().enumerate() {
            let next = body.rescues.get(i + 1).map(|r| r.span.start).or(ensure_at).unwrap_or(end);
            self.rescue(rescue, next);
        }
        if let (Some(ensure), Some(at)) = (&body.ensure, ensure_at) {
            self.line_element(at);
            self.write("ensure");
            self.line_end(at);
            self.newline();
            self.block_stmts(ensure, end);
        }
    }

    fn rescue(&mut self, rescue: &Rescue, next: u32) {
        self.line_element(rescue.span.start);
        self.write("rescue");
        for (i, ty) in rescue.types.iter().enumerate() {
            self.write(if i == 0 { " " } else { ", " });
            self.ty(ty);
        }
        if let Some(binding) = &rescue.binding {
            self.write(" => ");
            self.write(&binding.name);
        }
        self.line_end(rescue.span.start);
        self.newline();
        self.block_stmts(&rescue.body, next);
    }

    /// A statement: modifiers (`x if c`, `x rescue y`) are kept when the
    /// source used them.
    pub(crate) fn stmt(&mut self, e: &Expr) {
        let text = self.source(e.span);
        match &e.kind {
            ExprKind::If { cond, then, else_: None }
                if then.len() == 1 && !starts_with_word(text, &["if", "unless"]) =>
            {
                self.stmt(&then[0]);
                let keyword = self.keyword_between(then[0].span.end, cond.span.start, &["unless", "if"]);
                self.condition(keyword, cond);
            }
            ExprKind::While { cond, body } if body.len() == 1 && !starts_with_word(text, &["while", "until"]) => {
                self.stmt(&body[0]);
                let keyword = self.keyword_between(body[0].span.end, cond.span.start, &["until", "while"]);
                self.condition(keyword, cond);
            }
            ExprKind::Begin(body) if !starts_with_word(text, &["begin"]) && body.stmts.len() == 1 => {
                self.stmt(&body.stmts[0]);
                self.write(" rescue ");
                self.expr(&body.rescues[0].body[0]);
            }
            _ => self.expr(e),
        }
    }

    /// A statement that must fit on one line (a `{ … }` block's body).
    pub(crate) fn stmt_inline(&mut self, e: &Expr) {
        self.stmt(e);
    }

    /// ` if c`, ` unless c` (the tree has `!c`): the keyword the source used.
    fn condition(&mut self, keyword: &str, cond: &Expr) {
        self.write(" ");
        self.write(keyword);
        self.write(" ");
        match (&cond.kind, keyword) {
            (ExprKind::Unary { op: UnOp::Not, expr }, "unless" | "until") => self.expr(expr),
            _ => self.expr(cond),
        }
    }

    /// Which of `keywords` the source has between `from` and `to`.
    fn keyword_between<'k>(&self, from: u32, to: u32, keywords: &[&'k str]) -> &'k str {
        let text = &self.src[from as usize..to as usize];
        keywords
            .iter()
            .copied()
            .find(|k| text.split_whitespace().any(|w| w == *k))
            .unwrap_or(keywords[keywords.len() - 1])
    }

    // ── Control flow ─────────────────────────────────────────

    pub(crate) fn control(&mut self, e: &Expr) {
        match &e.kind {
            // `cond ? a : b`, as written
            ExprKind::If { cond, then, else_: Some(otherwise) }
                if then.len() == 1
                    && otherwise.len() == 1
                    && !starts_with_word(
                        self.source(e.span).trim_start_matches(['(', ' ', '\n']),
                        &["if", "unless"],
                    ) =>
            {
                self.expr(cond);
                self.write(" ? ");
                self.expr(&then[0]);
                self.write(" : ");
                self.expr(&otherwise[0]);
            }
            ExprKind::If { cond, then, else_ } => self.if_expr(e, cond, then, else_.as_deref()),
            ExprKind::While { cond, body } => {
                let text = self.source(e.span);
                let until = starts_with_word(text, &["until"]);
                self.write(if until { "until " } else { "while " });
                match (&cond.kind, until) {
                    (ExprKind::Unary { op: UnOp::Not, expr }, true) => self.expr(expr),
                    _ => self.expr(cond),
                }
                self.line_end(e.span.start);
                self.newline();
                self.block_stmts(body, e.span.end);
                self.write("end");
            }
            ExprKind::Case { subject, arms, else_ } => {
                self.write("case");
                if let Some(subject) = subject {
                    self.write(" ");
                    self.expr(subject);
                }
                self.line_end(e.span.start);
                self.newline();
                let else_at = else_.as_ref().map(|stmts| self.else_position(stmts, e.span.end));
                for (i, arm) in arms.iter().enumerate() {
                    let next = arms.get(i + 1).map(|a| a.span.start).or(else_at).unwrap_or(e.span.end);
                    self.arm(arm, next);
                }
                if let Some(else_) = else_ {
                    self.else_clause(else_, e.span.end);
                }
                self.comments_before(e.span.end);
                self.write("end");
            }
            ExprKind::Begin(body) => {
                self.write("begin");
                self.line_end(e.span.start);
                self.newline();
                self.body(body, e.span.end);
                self.comments_before(e.span.end);
                self.write("end");
            }
            _ => unreachable!("not a control structure"),
        }
    }

    fn if_expr(&mut self, e: &Expr, cond: &Expr, then: &[Expr], else_: Option<&[Expr]>) {
        let text = self.source(e.span);
        let unless = starts_with_word(text, &["unless"]) && matches!(cond.kind, ExprKind::Unary { op: UnOp::Not, .. });
        // `if c then a else b end`, kept on one line when written so
        if !text.contains('\n')
            && let Some(flat) = self.flat(|p| p.if_inline(cond, then, else_, unless))
        {
            self.write(&flat);
            return;
        }
        self.if_head(if unless { "unless" } else { "if" }, cond, unless);
        self.line_end(e.span.start);
        self.newline();
        let next = self.branch_end(else_, e.span.end);
        self.block_stmts(then, next);
        let mut rest = else_;
        while let Some(stmts) = rest {
            // `elsif`: an `else` holding a single `if` (written `elsif`)
            if let [ExprKind::If { cond, then, else_ }] = stmts.iter().map(|s| &s.kind).collect::<Vec<_>>().as_slice()
                && self.source(stmts[0].span).starts_with("elsif")
            {
                self.line_element(stmts[0].span.start);
                self.if_head("elsif", cond, false);
                self.line_end(stmts[0].span.start);
                self.newline();
                let next = self.branch_end(else_.as_deref(), e.span.end);
                self.block_stmts(then, next);
                rest = else_.as_deref();
                continue;
            }
            self.else_clause(stmts, e.span.end);
            break;
        }
        self.comments_before(e.span.end);
        self.write("end");
    }

    /// Where a branch followed by `else_` ends: at `elsif`, `else`, or `end`.
    fn branch_end(&self, else_: Option<&[Expr]>, end: u32) -> u32 {
        match else_ {
            Some([first, ..]) if self.source(first.span).starts_with("elsif") => first.span.start,
            Some(stmts) => self.else_position(stmts, end),
            None => end,
        }
    }

    /// The `else` keyword before `stmts` (or `end` for an empty `else`).
    fn else_position(&self, stmts: &[Expr], end: u32) -> u32 {
        let before = stmts.first().map_or(end, |s| s.span.start) as usize;
        self.src[..before].rfind("else").map_or(end, |i| i as u32)
    }

    fn if_head(&mut self, keyword: &str, cond: &Expr, negated: bool) {
        self.write(keyword);
        self.write(" ");
        match (&cond.kind, negated) {
            (ExprKind::Unary { op: UnOp::Not, expr }, true) => self.expr(expr),
            _ => self.expr(cond),
        }
    }

    fn if_inline(&mut self, cond: &Expr, then: &[Expr], else_: Option<&[Expr]>, unless: bool) {
        self.if_head(if unless { "unless" } else { "if" }, cond, unless);
        self.write(" then");
        self.inline_stmts(then);
        let mut rest = else_;
        while let Some(stmts) = rest {
            if let [stmt] = stmts
                && let ExprKind::If { cond, then, else_ } = &stmt.kind
                && self.source(stmt.span).starts_with("elsif")
            {
                self.write(" ");
                self.if_head("elsif", cond, false);
                self.write(" then");
                self.inline_stmts(then);
                rest = else_.as_deref();
                continue;
            }
            self.write(" else");
            self.inline_stmts(stmts);
            break;
        }
        self.write(" end");
    }

    /// Statements on one line: a single one (several need lines).
    fn inline_stmts(&mut self, stmts: &[Expr]) {
        match stmts {
            [] => {}
            [stmt] => {
                self.write(" ");
                self.stmt(stmt);
            }
            _ => self.newline(),
        }
    }

    fn else_clause(&mut self, stmts: &[Expr], end: u32) {
        // the comments above `else` (those after it belong to its statements)
        let at = stmts.first().and_then(|first| self.src[..first.span.start as usize].rfind("else"));
        if let Some(at) = at {
            self.line_element(at as u32);
        }
        // `else value`, kept on one line when written so
        if let (Some(at), [stmt]) = (at, stmts)
            && !self.src[at..stmt.span.end as usize].contains('\n')
            && let Some(flat) = self.flat(|p| p.stmt(stmt))
        {
            self.write("else ");
            self.write(&flat);
            self.line_end(stmt.span.end);
            self.newline();
            return;
        }
        self.write("else");
        if let Some(at) = at {
            self.line_end(at as u32);
        }
        self.newline();
        self.block_stmts(stmts, end);
    }

    fn arm(&mut self, arm: &CaseArm, next: u32) {
        self.line_element(arm.span.start);
        match &arm.test {
            ArmTest::In(pattern) => {
                self.write("in ");
                self.pattern(pattern);
            }
            ArmTest::When(values) => {
                self.write("when ");
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.expr(v);
                }
            }
        }
        if let Some(guard) = &arm.guard {
            let unless = self.source(arm.span).contains(" unless ")
                && matches!(guard.kind, ExprKind::Unary { op: UnOp::Not, .. });
            self.write(if unless { " unless " } else { " if " });
            match (&guard.kind, unless) {
                (ExprKind::Unary { op: UnOp::Not, expr }, true) => self.expr(expr),
                _ => self.expr(guard),
            }
        }
        // `in X then value`, kept on one line when written so
        let body_end = arm.body.last().map_or(arm.span.end, |s| s.span.end);
        let one_line = !self.src[arm.span.start as usize..body_end as usize].contains('\n');
        if let ([stmt], true) = (arm.body.as_slice(), one_line)
            && let Some(flat) = self.flat(|p| p.stmt(stmt))
        {
            self.write(" then ");
            self.write(&flat);
            self.line_end(body_end);
            self.newline();
            return;
        }
        self.line_end(arm.span.start);
        self.newline();
        self.block_stmts(&arm.body, next);
    }

    // ── Patterns, types, parameters ──────────────────────────

    fn pattern(&mut self, pattern: &Pattern) {
        match &pattern.kind {
            PatternKind::Wildcard => self.write("_"),
            PatternKind::Bind(name) => self.write(name),
            PatternKind::Lit(e) => self.expr(e),
            PatternKind::Const { path, fields } => {
                let name = path.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join("::");
                self.write(&name);
                if let Some(fields) = fields {
                    self.write("(");
                    for (i, field) in fields.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        if let Some(name) = &field.name {
                            self.write(&name.name);
                            self.write(":");
                            // `(category:)`: the field bound to a variable of its name
                            let punned = matches!(&field.pattern.kind, PatternKind::Bind(b) if *b == name.name)
                                && self.source(field.pattern.span).ends_with(':');
                            if punned {
                                continue;
                            }
                            self.write(" ");
                        }
                        self.pattern(&field.pattern);
                    }
                    self.write(")");
                }
            }
            PatternKind::Array(items) => {
                self.write("[");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.pattern(item);
                }
                self.write("]");
            }
            PatternKind::Or(alternatives) => {
                for (i, alt) in alternatives.iter().enumerate() {
                    if i > 0 {
                        self.write(" | ");
                    }
                    self.pattern(alt);
                }
            }
        }
    }

    pub(crate) fn ty(&mut self, ty: &Type) {
        match ty {
            Type::Named { path, args, .. } => {
                let name = path.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join("::");
                self.write(&name);
                if !args.is_empty() {
                    self.write("(");
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.ty(arg);
                    }
                    self.write(")");
                }
            }
            Type::Optional(inner, _) => {
                self.ty(inner);
                self.write("?");
            }
            Type::Tainted(inner, _) => {
                self.write("~");
                self.ty(inner);
            }
        }
    }

    pub(crate) fn param(&mut self, param: &Param) {
        self.write(&param.name.name);
        if let Some(ty) = &param.ty {
            self.write(": ");
            self.ty(ty);
        }
        if let Some(default) = &param.default {
            self.write(" = ");
            self.expr(default);
        }
    }

    pub(crate) fn params(&mut self, params: &[Param]) {
        self.write("(");
        for (i, param) in params.iter().enumerate() {
            if i > 0 {
                self.write(", ");
            }
            self.param(param);
        }
        self.write(")");
    }
}

/// `text` begins with one of `words`, as a whole word.
pub(crate) fn starts_with_word(text: &str, words: &[&str]) -> bool {
    words
        .iter()
        .any(|w| text.strip_prefix(w).is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_')))
}
