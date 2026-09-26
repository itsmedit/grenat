//! Expressions, with the parentheses the parser needs and no more.

use grenat_ast::{Arg, BinOp, Block, Expr, ExprKind, StrSeg, UnOp};

use crate::printer::{Printer, WIDTH};

/// Binding powers of the operator at the top of `e`, as the parser's (Pratt):
/// (how tightly it takes a left operand, how far its right operand extends).
/// Atoms (literals, calls with parentheses, `if … end`…) take nothing.
fn powers(e: &Expr) -> (u8, u8) {
    use BinOp::*;
    const ATOM: (u8, u8) = (u8::MAX, u8::MAX);
    match &e.kind {
        ExprKind::Binary { op, .. } => match op {
            Or => (4, 5),
            And => (6, 7),
            Eq | NotEq | Match | Cmp => (8, 9),
            Lt | Le | Gt | Ge => (10, 11),
            BitOr | BitXor => (12, 13),
            BitAnd => (14, 15),
            Shl | Shr => (16, 17),
            Add | Sub => (18, 19),
            Mul | Div | Rem => (20, 21),
            Pow => (25, 24),
        },
        ExprKind::Range { .. } => (2, 3),
        ExprKind::Unary { op: UnOp::Neg, .. } => (u8::MAX, 24),
        ExprKind::Unary { op: UnOp::Not, .. } => (u8::MAX, 26),
        ExprKind::Assign { .. } | ExprKind::OpAssign { .. } | ExprKind::MultiAssign { .. } => (0, 0),
        ExprKind::Return(Some(_)) | ExprKind::Break(Some(_)) | ExprKind::Next(Some(_)) => (0, 0),
        _ if is_command(e) => (u8::MAX, 0),
        _ => ATOM,
    }
}

/// A call without parentheses but with arguments: they extend to the end.
pub(crate) fn is_command(e: &Expr) -> bool {
    matches!(&e.kind, ExprKind::Call { parens: false, args, .. } if !args.is_empty())
}

/// `e` is written starting with a number (`3`, `3.abs`, `2 * x`).
fn starts_with_number(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Int(_) | ExprKind::Float(_) => true,
        ExprKind::Call { recv: Some(recv), .. } | ExprKind::Index { recv, .. } => starts_with_number(recv),
        ExprKind::Try(inner) => starts_with_number(inner),
        ExprKind::Binary { lhs, .. } | ExprKind::Range { lo: lhs, .. } => starts_with_number(lhs),
        _ => false,
    }
}

fn is_control(e: &Expr) -> bool {
    matches!(e.kind, ExprKind::If { .. } | ExprKind::While { .. } | ExprKind::Case { .. } | ExprKind::Begin(_))
}

fn is_atom(e: &Expr) -> bool {
    powers(e) == (u8::MAX, u8::MAX)
}

fn op_text(op: BinOp) -> &'static str {
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

impl Printer<'_> {
    pub(crate) fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Symbol(_) => {
                // as written, without the parentheses around it (`(3)`)
                let mut text = self.source(e.span).trim();
                while let Some(inner) = text.strip_prefix('(').and_then(|t| t.strip_suffix(')')) {
                    text = inner.trim();
                }
                self.write(text);
            }
            ExprKind::Str(segs) => self.string(e, segs),
            ExprKind::Bool(b) => self.write(if *b { "true" } else { "false" }),
            ExprKind::Nil => self.write("nil"),
            ExprKind::SelfRef => self.write("self"),
            ExprKind::It => self.write("&"),
            ExprKind::Var(name) => self.write(name),
            ExprKind::IVar(name) => {
                self.write("@");
                self.write(name);
            }
            ExprKind::Const(path) => {
                let text = path.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join("::");
                self.write(&text);
            }
            ExprKind::Array(items) => {
                let items: Vec<&Expr> = items.iter().collect();
                self.list("[", "]", &items, |p, e| p.expr(e));
            }
            ExprKind::Hash(entries) => self.hash(entries),
            ExprKind::Range { lo, hi, inclusive } => {
                self.left(lo, 2);
                self.write(if *inclusive { ".." } else { "..." });
                self.right(hi, 3);
            }
            ExprKind::Call { .. } => self.call(e),
            ExprKind::Index { recv, args } => {
                self.receiver(recv);
                let args: Vec<&Expr> = args.iter().collect();
                self.list("[", "]", &args, |p, e| p.expr(e));
            }
            ExprKind::Unary { op, expr } => self.unary(*op, expr),
            ExprKind::Binary { op, lhs, rhs } => {
                let (l, r) = powers(e);
                self.left(lhs, l);
                self.write(" ");
                self.write(op_text(*op));
                self.write(" ");
                self.right(rhs, r);
            }
            ExprKind::Try(inner) => {
                self.receiver(inner);
                self.write("?");
            }
            ExprKind::Assign { target, value } => {
                self.expr(target);
                self.write(" = ");
                self.expr(value);
            }
            ExprKind::OpAssign { op, target, value } => {
                self.expr(target);
                self.write(" ");
                self.write(op_text(*op));
                self.write("= ");
                self.expr(value);
            }
            ExprKind::MultiAssign { targets, value } => {
                for (i, target) in targets.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.expr(target);
                }
                self.write(" = ");
                self.expr(value);
            }
            ExprKind::Return(value) => self.jump("return", value.as_deref()),
            ExprKind::Break(value) => self.jump("break", value.as_deref()),
            ExprKind::Next(value) => self.jump("next", value.as_deref()),
            ExprKind::If { .. } | ExprKind::While { .. } | ExprKind::Case { .. } | ExprKind::Begin(_) => {
                self.control(e)
            }
        }
    }

    /// Left operand of an operator taking it with power `l`.
    fn left(&mut self, e: &Expr, l: u8) {
        let (_, r) = powers(e);
        self.maybe_parens(e, r <= l || is_control(e));
    }

    /// Right operand of an operator extending with power `r` (an `if … end`
    /// operand is clearer in parentheses, though the grammar needs none).
    fn right(&mut self, e: &Expr, r: u8) {
        let (l, _) = powers(e);
        self.maybe_parens(e, l < r || is_control(e));
    }

    pub(crate) fn maybe_parens(&mut self, e: &Expr, parens: bool) {
        if parens {
            self.write("(");
            self.expr(e);
            self.write(")");
        } else {
            self.expr(e);
        }
    }

    fn unary(&mut self, op: UnOp, e: &Expr) {
        match op {
            UnOp::Neg => {
                self.write("-");
                // `-3` would be read as the literal -3 (`-3.abs` too), `- -x` as `--`
                let literal = starts_with_number(e) || matches!(e.kind, ExprKind::Unary { .. });
                let (l, _) = powers(e);
                self.maybe_parens(e, literal || l < 24);
            }
            UnOp::Not => {
                // `unless c` and `not c` are `!c` in the tree
                self.write("!");
                self.maybe_parens(e, !is_atom(e) && !matches!(e.kind, ExprKind::Unary { .. }));
            }
        }
    }

    /// A receiver of `.`, `[…]` or `?`: an atom, or parenthesized.
    fn receiver(&mut self, e: &Expr) {
        self.maybe_parens(e, !is_atom(e));
    }

    fn jump(&mut self, keyword: &str, value: Option<&Expr>) {
        self.write(keyword);
        if let Some(value) = value {
            self.write(" ");
            self.expr(value);
        }
    }

    // ── Calls and blocks ─────────────────────────────────────

    fn call(&mut self, e: &Expr) {
        let ExprKind::Call { recv, .. } = &e.kind else { unreachable!("a call") };
        // a long chain `a.b(…).c(…)`: one call per line, the lexer joins them
        let chain = chain(e);
        // a module or a type stays with its first call: `Dir.list(…)`, `Http.get(…)`
        let anchored = chain.first().is_some_and(|c| matches!(receiver_of(c).kind, ExprKind::Const(_)));
        let links = if anchored { &chain[1..] } else { &chain[..] };
        if !self.measuring && links.len() >= 2 && !self.first_line_fits(|p| p.expr(e)) {
            self.receiver(receiver_of(chain[0]));
            if anchored {
                self.link(chain[0]);
            }
            self.indented(|p| {
                for link in links {
                    p.newline();
                    p.link(link);
                }
            });
            return;
        }
        if let Some(recv) = recv {
            self.receiver(recv);
        }
        self.link(e);
    }

    /// A call without its receiver: `.name(args) block`.
    fn link(&mut self, e: &Expr) {
        let ExprKind::Call { recv, name, args, block, safe, parens } = &e.kind else { unreachable!("a call") };
        if let Some(recv) = recv {
            let constant = matches!(recv.kind, ExprKind::Const(_)) && name.name.starts_with(char::is_uppercase);
            self.write(if constant {
                "::"
            } else if *safe {
                "&."
            } else {
                "."
            });
        }
        self.write(&name.name);
        // `&.sent?` was parsed as a block over `it`, among the arguments
        let (short, block) = match block.as_deref() {
            Some(b) if self.source(b.span).starts_with("&.") => (Some(b), None),
            other => (None, other),
        };
        let mut items: Vec<ArgItem> = args.iter().map(ArgItem::Arg).collect();
        items.extend(short.map(ArgItem::Short));
        if *parens {
            self.list("(", ")", &items, |p, a| p.arg(a, false, false));
        } else if !items.is_empty() {
            self.write(" ");
            let alone = items.len() == 1;
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    self.write(", ");
                }
                self.arg(item, !alone, true);
            }
        }
        if let Some(block) = block {
            // with arguments but no parentheses, `{` would go to the last argument
            let force_do = !*parens && !items.is_empty();
            self.block(block, force_do);
        }
    }

    /// An argument; `command`: of a call without parentheses.
    fn arg(&mut self, item: &ArgItem, among_others: bool, command: bool) {
        match item {
            ArgItem::Arg(Arg::Pos(e)) => {
                // `f g a, b` would give `b` to `g`; `f {…}` would be a block
                let parens = (among_others && is_command(e)) || (command && matches!(e.kind, ExprKind::Hash(_)));
                self.maybe_parens(e, parens);
            }
            ArgItem::Arg(Arg::Named { name, value }) => {
                self.write(&name.name);
                self.write(":");
                if let Some(value) = value {
                    self.write(" ");
                    self.maybe_parens(value, among_others && is_command(value));
                }
            }
            ArgItem::Arg(Arg::BlockPass(e)) => {
                self.write("&");
                self.receiver(e);
            }
            ArgItem::Short(block) => self.expr(&block.body.stmts[0]),
        }
    }

    /// `{ |x| … }` or `do |x| … end`, as the source had it (braces on one
    /// line when they fit).
    pub(crate) fn block(&mut self, block: &Block, force_do: bool) {
        let braces = !force_do && self.source(block.span).starts_with('{');
        let params = if block.params.is_empty() {
            String::new()
        } else {
            let mut m = self.measurer();
            for (i, param) in block.params.iter().enumerate() {
                if i > 0 {
                    m.write(", ");
                }
                m.param(param);
            }
            format!(" |{}|", m.out)
        };
        if braces {
            let simple = block.body.rescues.is_empty() && block.body.ensure.is_none();
            if let (true, [stmt]) = (simple, block.body.stmts.as_slice())
                && let Some(flat) = self.flat(|p| p.stmt_inline(stmt))
                && self.column() + flat.len() + params.len() + 4 <= WIDTH
            {
                self.write(" {");
                self.write(&params);
                self.write(" ");
                self.write(&flat);
                self.write(" }");
                return;
            }
            if simple && block.body.stmts.is_empty() {
                self.write(" {");
                self.write(&params);
                self.write(" }");
                return;
            }
            self.write(" {");
            self.write(&params);
            self.line_end(block.span.start);
            self.newline();
            self.block_stmts(&block.body.stmts, block.span.end);
            self.write("}");
            return;
        }
        self.write(" do");
        self.write(&params);
        self.line_end(block.span.start);
        self.newline();
        self.body(&block.body, block.span.end);
        self.write("end");
    }

    /// Whether the first line of what `f` prints fits: a block's body goes on
    /// lines of its own and does not count.
    pub(crate) fn first_line_fits(&self, f: impl FnOnce(&mut Printer)) -> bool {
        let mut m = self.measurer();
        f(&mut m);
        let first = m.out.split('\n').next().unwrap_or_default();
        self.column() + first.chars().count() <= WIDTH
    }

    /// `e` printed on one line, if it fits on one.
    pub(crate) fn flat(&self, f: impl FnOnce(&mut Printer)) -> Option<String> {
        let mut m = self.measurer();
        f(&mut m);
        (!m.out.contains('\n')).then_some(m.out)
    }

    // ── Lists ────────────────────────────────────────────────

    /// `[a, b]`, `(a, b)`: on one line if it fits, otherwise one item per line.
    fn list<T>(&mut self, open: &str, close: &str, items: &[T], each: impl Fn(&mut Printer, &T)) {
        let flat = self.flat(|p| {
            p.write(open);
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    p.write(", ");
                }
                each(p, item);
            }
            p.write(close);
        });
        if let Some(flat) = flat.filter(|f| self.column() + f.len() <= WIDTH || items.len() < 2) {
            self.write(&flat);
            return;
        }
        self.write(open);
        self.newline();
        self.indented(|p| {
            for item in items {
                each(p, item);
                p.write(",");
                p.newline();
            }
        });
        self.write(close);
    }

    fn hash(&mut self, entries: &[(Expr, Expr)]) {
        let entries: Vec<&(Expr, Expr)> = entries.iter().collect();
        if entries.is_empty() {
            self.write("{}");
            return;
        }
        let entry = |p: &mut Printer, (key, value): &&(Expr, Expr)| {
            // `name: value` when written so (a label), `key => value` otherwise
            let label = matches!(key.kind, ExprKind::Symbol(_)) && p.source(key.span).ends_with(':');
            if label {
                let text = p.source(key.span);
                p.write(text);
                // `{query:}`: the value is the key itself
                if value.span == key.span {
                    return;
                }
            } else {
                p.expr(key);
                p.write(" =>");
            }
            p.write(" ");
            p.expr(value);
        };
        let flat = self.flat(|p| {
            p.write("{");
            for (i, e) in entries.iter().enumerate() {
                p.write(if i > 0 { ", " } else { "" });
                entry(p, e);
            }
            p.write("}");
        });
        if let Some(flat) = flat.filter(|f| self.column() + f.len() <= WIDTH) {
            self.write(&flat);
            return;
        }
        self.write("{");
        self.newline();
        self.indented(|p| {
            for e in &entries {
                entry(p, e);
                p.write(",");
                p.newline();
            }
        });
        self.write("}");
    }

    // ── Strings ──────────────────────────────────────────────

    /// A string as written; a heredoc's body is written after the line,
    /// re-indented.
    fn string(&mut self, e: &Expr, _segs: &[StrSeg]) {
        let opener = self.source(e.span);
        if !opener.starts_with("<<") {
            self.write(opener);
            return;
        }
        self.write(opener);
        if self.measuring {
            return;
        }
        let (lines, terminator, squiggly) = heredoc_body(self.src, e.span.start as usize);
        let body_indent = self.indent + 1;
        if squiggly {
            let strip = lines
                .iter()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.len() - l.trim_start_matches([' ', '\t']).len())
                .min()
                .unwrap_or(0);
            let lines = lines.iter().map(|l| if l.trim().is_empty() { String::new() } else { l[strip..].to_string() });
            self.defer_heredoc(lines.collect(), terminator, body_indent);
        } else {
            // `<<-`: the body is kept as it is (its indentation is content)
            self.defer_heredoc_raw(lines.iter().map(|l| l.to_string()).collect(), terminator);
        }
    }
}

/// The receiver of a method call.
fn receiver_of(call: &Expr) -> &Expr {
    match &call.kind {
        ExprKind::Call { recv: Some(recv), .. } => recv,
        _ => unreachable!("a method call"),
    }
}

/// The method calls of a chain, innermost first (`a.b.c` → `a.b`, `(a.b).c`).
fn chain(e: &Expr) -> Vec<&Expr> {
    let mut calls = Vec::new();
    let mut at = e;
    while let ExprKind::Call { recv: Some(recv), name, .. } = &at.kind {
        if matches!(recv.kind, ExprKind::Const(_)) && name.name.starts_with(char::is_uppercase) {
            break;
        }
        calls.push(at);
        at = recv;
    }
    calls.reverse();
    calls
}

/// An argument, or a `&.method` short block written among the arguments.
enum ArgItem<'a> {
    Arg(&'a Arg),
    Short(&'a Block),
}

/// The body lines and terminator of the heredoc opened at `opener` (bodies
/// of heredocs opened earlier on the same line come first), and whether it
/// strips indentation (`<<~`).
fn heredoc_body(src: &str, opener: usize) -> (Vec<&str>, String, bool) {
    let line_start = src[..opener].rfind('\n').map_or(0, |i| i + 1);
    let line_end = src[opener..].find('\n').map_or(src.len(), |i| opener + i);
    let mut body = (line_end + 1).min(src.len());
    let mut at = line_start;
    while let Some(found) = src[at..line_end].find("<<").map(|i| at + i) {
        let rest = &src[found + 2..];
        let squiggly = rest.starts_with('~');
        let id: String = rest[1..].trim_start_matches('\'').chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if !(rest.starts_with('~') || rest.starts_with('-')) || id.is_empty() {
            at = found + 2;
            continue;
        }
        let mut lines = Vec::new();
        let mut line = body;
        let terminator = loop {
            let end = src[line..].find('\n').map_or(src.len(), |i| line + i);
            let text = &src[line..end];
            if text.trim() == id || end >= src.len() {
                break text.trim().to_string();
            }
            lines.push(text);
            line = end + 1;
        };
        let after = src[line..].find('\n').map_or(src.len(), |i| line + i + 1);
        if found == opener {
            return (lines, terminator, squiggly);
        }
        body = after;
        at = found + 2;
    }
    (Vec::new(), String::new(), true)
}
