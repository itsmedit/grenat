//! Parser de Grenat : descente récursive pour les instructions et les
//! déclarations, Pratt pour les opérateurs binaires.
//!
//! Règles « à la Ruby » implémentées ici :
//! - un appel sans parenthèses (`puts x`, `spawn Researcher`) est reconnu quand
//!   un identifiant est suivi, après un blanc, d'un token qui peut commencer
//!   un argument ;
//! - dans les arguments d'un tel appel, `do … end` appartient à l'appel
//!   englobant (`within budget(usd: 1) do … end`), `{ … }` à l'appel le plus proche ;
//! - `if`/`unless`/`while`/`until`/`rescue` après une instruction sont des modificateurs.
//!
//! Le parser récupère après une erreur (en sautant à la ligne suivante) pour
//! signaler plusieurs problèmes en une passe.

use std::collections::HashMap;

use grenat_ast::*;
use grenat_lexer::{Comment, Keyword as K, StrPart, Token, TokenKind as T, lex};

const MAX_DIAGNOSTICS: usize = 100;

pub use grenat_ast::Diagnostic;

#[derive(Debug)]
pub struct Parsed {
    pub program: Program,
    /// Erreurs du lexer puis du parser.
    pub diagnostics: Vec<Diagnostic>,
}

pub fn parse(src: &str) -> Parsed {
    let lexed = lex(src);
    let mut diagnostics: Vec<Diagnostic> =
        lexed.errors.into_iter().map(|e| Diagnostic::new(e.span, e.message)).collect();
    let docs = DocTable::new(src, &lexed.comments);
    let mut parser = Parser::new(lexed.tokens, &docs);
    let program = parser.program();
    diagnostics.extend(parser.diags);
    Parsed { program, diagnostics }
}

type PResult<T> = Result<T, ()>;

/// Commentaires `##` indexés par ligne, pour les rattacher aux déclarations.
struct DocTable {
    line_starts: Vec<usize>,
    leading: HashMap<usize, String>,
    trailing: HashMap<usize, String>,
}

impl DocTable {
    fn new(src: &str, comments: &[Comment]) -> Self {
        let line_starts = std::iter::once(0).chain(src.match_indices('\n').map(|(i, _)| i + 1)).collect();
        let mut table = DocTable { line_starts, leading: HashMap::new(), trailing: HashMap::new() };
        for comment in comments.iter().filter(|c| c.doc) {
            let line = table.line_of(comment.span);
            let map = if comment.trailing { &mut table.trailing } else { &mut table.leading };
            map.insert(line, comment.text.clone());
        }
        table
    }

    fn line_of(&self, span: Span) -> usize {
        self.line_starts.partition_point(|&start| start <= span.start as usize) - 1
    }

    /// Lignes `##` contiguës juste au-dessus, puis `##` en fin de la même ligne.
    fn doc_for(&self, span: Span) -> Option<String> {
        let line = self.line_of(span);
        let mut lines = Vec::new();
        let mut above = line;
        while above > 0
            && let Some(text) = self.leading.get(&(above - 1))
        {
            lines.push(text.clone());
            above -= 1;
        }
        lines.reverse();
        lines.extend(self.trailing.get(&line).cloned());
        (!lines.is_empty()).then(|| lines.join("\n"))
    }
}

enum Infix {
    Bin(BinOp),
    Range(bool),
}

enum ArgOrBlock {
    Arg(Arg),
    Block(Block),
}

struct Parser<'d> {
    toks: Vec<Token>,
    pos: usize,
    diags: Vec<Diagnostic>,
    docs: &'d DocTable,
    /// Vrai dans les arguments d'un appel sans parenthèses : `do` y appartient à l'appel englobant.
    no_do: bool,
}

fn not(e: Expr) -> Expr {
    let span = e.span;
    Expr::new(ExprKind::Unary { op: UnOp::Not, expr: Box::new(e) }, span)
}

fn takes_block(e: &Expr) -> bool {
    matches!(e.kind, ExprKind::Var(_) | ExprKind::Call { block: None, .. })
}

fn attach_block(e: Expr, block: Block) -> Expr {
    let span = e.span.to(block.span);
    let block = Some(Box::new(block));
    let kind = match e.kind {
        ExprKind::Var(name) => ExprKind::Call {
            recv: None,
            name: Ident { name, span: e.span },
            args: Vec::new(),
            block,
            safe: false,
            parens: false,
        },
        ExprKind::Call { recv, name, args, safe, parens, .. } => {
            ExprKind::Call { recv, name, args, block, safe, parens }
        }
        _ => unreachable!("attach_block sur une expression qui ne prend pas de bloc"),
    };
    Expr::new(kind, span)
}

fn is_assignable(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Var(_) | ExprKind::IVar(_) | ExprKind::Const(_) | ExprKind::Index { .. } => true,
        ExprKind::Call { recv: Some(_), args, block: None, parens: false, .. } => args.is_empty(),
        _ => false,
    }
}

impl<'d> Parser<'d> {
    fn new(toks: Vec<Token>, docs: &'d DocTable) -> Self {
        debug_assert!(matches!(toks.last(), Some(Token { kind: T::Eof, .. })));
        Parser { toks, pos: 0, diags: Vec::new(), docs, no_do: false }
    }

    // ── Navigation ───────────────────────────────────────────

    fn peek(&self) -> &Token {
        &self.toks[self.pos]
    }

    fn kind(&self) -> &T {
        &self.peek().kind
    }

    fn nth(&self, n: usize) -> &T {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].kind
    }

    fn span(&self) -> Span {
        self.peek().span
    }

    fn prev_span(&self) -> Span {
        self.toks[self.pos.saturating_sub(1)].span
    }

    fn bump(&mut self) -> Token {
        let tok = self.toks[self.pos].clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        tok
    }

    fn at(&self, kind: &T) -> bool {
        self.kind() == kind
    }

    /// Token collé au précédent : `f(x)` et non `f (x)`.
    fn at_tight(&self, kind: &T) -> bool {
        self.at(kind) && !self.peek().space_before
    }

    fn at_kw(&self, kw: K) -> bool {
        *self.kind() == T::Kw(kw)
    }

    fn at_ident(&self, name: &str) -> bool {
        matches!(self.kind(), T::Ident(s) if s == name)
    }

    fn at_line_end(&self) -> bool {
        matches!(self.kind(), T::Newline | T::Eof)
    }

    fn eat(&mut self, kind: &T) -> bool {
        let found = self.at(kind);
        if found {
            self.bump();
        }
        found
    }

    fn eat_kw(&mut self, kw: K) -> bool {
        self.eat(&T::Kw(kw))
    }

    fn skip_newlines(&mut self) {
        while self.at(&T::Newline) {
            self.bump();
        }
    }

    fn recover_line(&mut self) {
        while !self.at_line_end() {
            self.bump();
        }
    }

    fn with_do<R>(&mut self, allowed: bool, f: impl FnOnce(&mut Self) -> R) -> R {
        let saved = self.no_do;
        self.no_do = !allowed;
        let result = f(self);
        self.no_do = saved;
        result
    }

    // ── Erreurs ──────────────────────────────────────────────

    fn report(&mut self, diag: Diagnostic) {
        if self.diags.len() < MAX_DIAGNOSTICS {
            self.diags.push(diag);
        }
    }

    fn fail<X>(&mut self, span: Span, message: impl Into<String>) -> PResult<X> {
        self.report(Diagnostic::new(span, message));
        Err(())
    }

    fn unexpected<X>(&mut self, expected: &str) -> PResult<X> {
        let message = format!("attendu : {expected} ; trouvé : {}", self.kind().describe());
        self.fail(self.span(), message)
    }

    fn expect(&mut self, kind: T, expected: &str) -> PResult<Span> {
        if self.at(&kind) { Ok(self.bump().span) } else { self.unexpected(expected) }
    }

    fn expect_end(&mut self, opener: Span, what: &str) -> PResult<Span> {
        if self.at_kw(K::End) {
            return Ok(self.bump().span);
        }
        let message = format!("`end` attendu pour fermer `{what}` ; trouvé : {}", self.kind().describe());
        self.report(Diagnostic::new(self.span(), message).with_note(opener, format!("`{what}` ouvert ici")));
        Err(())
    }

    // ── Noms ─────────────────────────────────────────────────

    fn ident(&mut self, expected: &str) -> PResult<Ident> {
        match self.kind().clone() {
            T::Ident(name) => Ok(Ident { name, span: self.bump().span }),
            _ => self.unexpected(expected),
        }
    }

    fn const_name(&mut self, expected: &str) -> PResult<Ident> {
        match self.kind().clone() {
            T::Const(name) => Ok(Ident { name, span: self.bump().span }),
            _ => self.unexpected(expected),
        }
    }

    /// Après `.` : les mots-clés sont des noms de méthode valides (`x.class`).
    fn method_name(&mut self) -> PResult<Ident> {
        let name = match self.kind() {
            T::Ident(s) | T::Const(s) => s.clone(),
            T::Kw(k) => k.as_str().to_string(),
            _ => return self.unexpected("un nom de méthode"),
        };
        Ok(Ident { name, span: self.bump().span })
    }

    // ── Programme et déclarations ────────────────────────────

    fn program(&mut self) -> Program {
        let mut items = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&T::Eof) || self.diags.len() >= MAX_DIAGNOSTICS {
                break;
            }
            let before = self.pos;
            match self.item() {
                Ok(item) => {
                    items.push(item);
                    if !self.at_line_end() {
                        let _ = self.unexpected::<()>("une fin de ligne");
                        self.recover_line();
                    }
                }
                Err(()) => self.recover_line(),
            }
            if self.pos == before {
                self.bump();
            }
        }
        Program { items }
    }

    fn item(&mut self) -> PResult<Item> {
        let doc = self.docs.doc_for(self.span());
        match self.kind() {
            T::Kw(K::Def | K::Abstract | K::Prompt | K::Tool | K::Workflow) => {
                self.fn_def(doc).map(|f| Item::Fn(Box::new(f)))
            }
            T::Kw(K::Struct) => self.type_def(TypeKind::Struct, doc).map(Item::Type),
            T::Kw(K::Class) => self.type_def(TypeKind::Class, doc).map(Item::Type),
            T::Kw(K::Module) => self.type_def(TypeKind::Module, doc).map(Item::Type),
            T::Kw(K::Enum) => self.type_def(TypeKind::Enum, doc).map(Item::Type),
            T::Kw(K::Agent) => self.type_def(TypeKind::Agent, doc).map(Item::Type),
            T::Kw(K::Supervisor) => self.type_def(TypeKind::Supervisor, doc).map(Item::Type),
            T::Ident(name) if name == "model" && matches!(self.nth(1), T::Symbol(_)) => {
                self.model_decl().map(Item::Model)
            }
            _ => self.stmt().map(Item::Stmt),
        }
    }

    fn fn_def(&mut self, doc: Option<String>) -> PResult<FnDef> {
        let start = self.span();
        let is_abstract = self.eat_kw(K::Abstract);
        let (kind, keyword) = match self.kind() {
            T::Kw(K::Def) => (FnKind::Def, "def"),
            T::Kw(K::Prompt) => (FnKind::Prompt, "prompt"),
            T::Kw(K::Tool) => (FnKind::Tool, "tool"),
            T::Kw(K::Workflow) => (FnKind::Workflow, "workflow"),
            _ => return self.unexpected("`def`"),
        };
        let keyword_span = self.bump().span;

        let on_self = self.at_kw(K::SelfKw) && *self.nth(1) == T::Dot;
        if on_self {
            self.bump();
            self.bump();
        }
        let name = self.ident("un nom de fonction")?;
        let params = if self.at_tight(&T::LParen) { self.params()? } else { Vec::new() };
        let ret = if self.eat(&T::Arrow) { Some(self.ty()?) } else { None };

        let mut effects = Vec::new();
        let mut model = None;
        loop {
            if self.at_ident("uses") {
                self.bump();
                effects.extend(self.effects()?);
            } else if self.at_ident("using") {
                self.bump();
                model = Some(self.unary()?);
            } else {
                break;
            }
        }

        let (body, short) = if self.eat(&T::Eq) {
            self.skip_newlines();
            let e = self.stmt()?;
            let span = e.span;
            (Body::new(vec![e], span), true)
        } else if is_abstract {
            (Body::new(Vec::new(), self.prev_span()), false)
        } else {
            let body = self.body(&[])?;
            self.expect_end(keyword_span, keyword)?;
            (body, false)
        };

        Ok(FnDef {
            kind,
            doc,
            is_abstract,
            on_self,
            name,
            params,
            ret,
            effects,
            model,
            body,
            short,
            span: start.to(self.prev_span()),
        })
    }

    fn params(&mut self) -> PResult<Vec<Param>> {
        self.expect(T::LParen, "`(`")?;
        let mut params = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&T::RParen) {
                break;
            }
            params.push(self.param()?);
            self.skip_newlines();
            if !self.eat(&T::Comma) {
                break;
            }
        }
        self.skip_newlines();
        self.expect(T::RParen, "`)`")?;
        Ok(params)
    }

    /// `nom: Type = défaut` ou `nom = défaut`.
    fn param(&mut self) -> PResult<Param> {
        let start = self.span();
        let (name, ty) = match self.kind().clone() {
            T::Label(name) => {
                self.bump();
                (name, Some(self.ty()?))
            }
            T::Ident(name) => {
                self.bump();
                (name, None)
            }
            _ => return self.unexpected("un paramètre `nom: Type`"),
        };
        let default = if self.eat(&T::Eq) { Some(self.expr()?) } else { None };
        Ok(Param { name: Ident { name, span: start }, ty, default, span: start.to(self.prev_span()) })
    }

    /// `llm, net("host"), fs.read("./docs")`
    fn effects(&mut self) -> PResult<Vec<Effect>> {
        let mut effects = Vec::new();
        loop {
            let first = self.ident("un effet (`llm`, `net`, `fs.read`…)")?;
            let start = first.span;
            let mut path = vec![first];
            while self.at(&T::Dot) && matches!(self.nth(1), T::Ident(_)) {
                self.bump();
                path.push(self.ident("un effet")?);
            }
            let mut args = Vec::new();
            if self.at_tight(&T::LParen) {
                for arg in self.paren_args()?.0 {
                    match arg {
                        Arg::Pos(e) => args.push(e),
                        Arg::Named { name, .. } => {
                            return self.fail(name.span, "les restrictions d'effet sont positionnelles");
                        }
                        Arg::BlockPass(e) => return self.fail(e.span, "bloc inattendu dans un effet"),
                    }
                }
            }
            effects.push(Effect { path, args, span: start.to(self.prev_span()) });
            if !self.eat(&T::Comma) {
                break;
            }
            self.skip_newlines();
        }
        Ok(effects)
    }

    fn model_decl(&mut self) -> PResult<ModelDecl> {
        let start = self.bump().span;
        let name = match self.kind().clone() {
            T::Symbol(name) => Ident { name, span: self.bump().span },
            _ => return self.unexpected("un symbole (`:fast`)"),
        };
        let options = if self.eat(&T::Comma) {
            self.skip_newlines();
            self.arg_list()?.0
        } else {
            Vec::new()
        };
        Ok(ModelDecl { name, options, span: start.to(self.prev_span()) })
    }

    fn type_def(&mut self, kind: TypeKind, doc: Option<String>) -> PResult<TypeDef> {
        let keyword = self.bump();
        let name = self.const_name("un nom en majuscule")?;
        let options = if self.eat(&T::Comma) {
            self.skip_newlines();
            self.arg_list()?.0
        } else {
            Vec::new()
        };
        let members = self.members(kind);
        let what = match keyword.kind {
            T::Kw(k) => k.as_str(),
            _ => "type",
        };
        let end = self.expect_end(keyword.span, what)?;
        Ok(TypeDef { kind, doc, name, options, members, span: keyword.span.to(end) })
    }

    fn members(&mut self, kind: TypeKind) -> Vec<Member> {
        let mut members = Vec::new();
        loop {
            self.skip_newlines();
            if self.at_kw(K::End) || self.at(&T::Eof) || self.diags.len() >= MAX_DIAGNOSTICS {
                break;
            }
            let before = self.pos;
            match self.member(kind) {
                Ok(member) => {
                    members.push(member);
                    if !self.at_line_end() && !self.at_kw(K::End) {
                        let _ = self.unexpected::<()>("une fin de ligne");
                        self.recover_line();
                    }
                }
                Err(()) => self.recover_line(),
            }
            if self.pos == before {
                self.bump();
            }
        }
        members
    }

    fn member(&mut self, kind: TypeKind) -> PResult<Member> {
        let start = self.span();
        let doc = self.docs.doc_for(start);
        match self.kind().clone() {
            T::Label(_) => self.field(doc).map(Member::Field),
            T::IVar(name) => {
                self.bump();
                let ty = if self.eat(&T::Colon) { Some(self.ty()?) } else { None };
                let default = if self.eat(&T::Eq) { Some(self.expr()?) } else { None };
                if ty.is_none() && default.is_none() {
                    return self.fail(start, format!("`@{name}` : type (`@{name}: Type`) ou valeur initiale attendus"));
                }
                let span = start.to(self.prev_span());
                Ok(Member::Field(Field { doc, name: Ident { name, span: start }, is_ivar: true, ty, default, span }))
            }
            T::Kw(K::Def | K::Abstract | K::Prompt | K::Tool | K::Workflow) => self.fn_def(doc).map(Member::Method),
            T::Ident(name) if name == "include" => {
                self.bump();
                self.ty().map(Member::Include)
            }
            T::Ident(name) if name == "on" && kind == TypeKind::Agent => self.handler(doc).map(Member::Handler),
            T::Const(_) if kind == TypeKind::Enum => self.variant(doc).map(Member::Variant),
            T::Ident(_) if matches!(kind, TypeKind::Agent | TypeKind::Supervisor) => {
                let name = self.ident("une directive")?;
                let args = if self.at_line_end() { Vec::new() } else { self.command_args()?.0 };
                Ok(Member::Directive(Directive { name, args, span: start.to(self.prev_span()) }))
            }
            _ => self.unexpected(match kind {
                TypeKind::Enum => "une variante, une méthode ou `end`",
                TypeKind::Agent => "une directive, `@état`, `on Message`, une méthode ou `end`",
                TypeKind::Supervisor => "une directive (`child …`) ou `end`",
                _ => "un champ `nom: Type`, une méthode ou `end`",
            }),
        }
    }

    /// `nom: Type = défaut`
    fn field(&mut self, doc: Option<String>) -> PResult<Field> {
        let start = self.span();
        let T::Label(name) = self.kind().clone() else {
            return self.unexpected("un champ `nom: Type`");
        };
        self.bump();
        let ty = self.ty()?;
        let default = if self.eat(&T::Eq) { Some(self.expr()?) } else { None };
        let span = start.to(self.prev_span());
        Ok(Field { doc, name: Ident { name, span: start }, is_ivar: false, ty: Some(ty), default, span })
    }

    fn variant(&mut self, doc: Option<String>) -> PResult<Variant> {
        let name = self.const_name("une variante")?;
        let mut fields = Vec::new();
        if self.at_tight(&T::LParen) {
            self.bump();
            loop {
                self.skip_newlines();
                if self.at(&T::RParen) {
                    break;
                }
                let doc = self.docs.doc_for(self.span());
                fields.push(self.field(doc)?);
                self.skip_newlines();
                if !self.eat(&T::Comma) {
                    break;
                }
            }
            self.skip_newlines();
            self.expect(T::RParen, "`)`")?;
        }
        let span = name.span.to(self.prev_span());
        Ok(Variant { doc, name, fields, span })
    }

    fn handler(&mut self, doc: Option<String>) -> PResult<Handler> {
        let start = self.bump().span;
        let message = self.const_name("un nom de message en majuscule")?;
        let params = if self.at_tight(&T::LParen) { self.params()? } else { Vec::new() };
        let ret = if self.eat(&T::Arrow) { Some(self.ty()?) } else { None };
        let body = self.body(&[])?;
        let end = self.expect_end(start, "on")?;
        Ok(Handler { doc, message, params, ret, body, span: start.to(end) })
    }

    // ── Types ────────────────────────────────────────────────

    fn ty(&mut self) -> PResult<Type> {
        if self.at(&T::Tilde) {
            let start = self.bump().span;
            let inner = self.ty()?;
            let span = start.to(inner.span());
            return Ok(Type::Tainted(Box::new(inner), span));
        }
        let start = self.span();
        let mut path = vec![self.const_name("un type")?];
        while self.eat(&T::ColonColon) {
            path.push(self.const_name("un type")?);
        }
        let mut args = Vec::new();
        if self.at_tight(&T::LParen) {
            self.bump();
            loop {
                self.skip_newlines();
                if self.at(&T::RParen) {
                    break;
                }
                args.push(self.ty()?);
                self.skip_newlines();
                if !self.eat(&T::Comma) {
                    break;
                }
            }
            self.skip_newlines();
            self.expect(T::RParen, "`)`")?;
        }
        let mut ty = Type::Named { path, args, span: start.to(self.prev_span()) };
        while self.at_tight(&T::Question) {
            let span = ty.span().to(self.bump().span);
            ty = Type::Optional(Box::new(ty), span);
        }
        Ok(ty)
    }

    // ── Corps et instructions ────────────────────────────────

    /// Instructions puis clauses `rescue`/`ensure` ; ne consomme pas le `end`.
    fn body(&mut self, stops: &[K]) -> PResult<Body> {
        let start = self.span();
        let mut all_stops = vec![K::Rescue, K::Ensure, K::End];
        all_stops.extend_from_slice(stops);
        let stmts = self.stmts(&all_stops);
        let mut rescues = Vec::new();
        while self.at_kw(K::Rescue) {
            rescues.push(self.rescue_clause()?);
        }
        let ensure = if self.eat_kw(K::Ensure) { Some(self.stmts(&[K::End])) } else { None };
        Ok(Body { stmts, rescues, ensure, span: start.to(self.prev_span()) })
    }

    fn stmts(&mut self, stops: &[K]) -> Vec<Expr> {
        self.with_do(true, |p| {
            let mut out = Vec::new();
            loop {
                p.skip_newlines();
                match p.kind() {
                    T::Kw(k) if stops.contains(k) => break,
                    T::RBrace | T::RParen | T::Eof => break,
                    _ if p.diags.len() >= MAX_DIAGNOSTICS => break,
                    _ => {}
                }
                let before = p.pos;
                match p.stmt() {
                    Ok(e) => {
                        out.push(e);
                        let at_end = matches!(p.kind(), T::Newline | T::Eof | T::RBrace)
                            || matches!(p.kind(), T::Kw(k) if stops.contains(k));
                        if !at_end {
                            let _ = p.unexpected::<()>("une fin de ligne");
                            p.recover_line();
                        }
                    }
                    Err(()) => p.recover_line(),
                }
                if p.pos == before {
                    p.bump();
                }
            }
            out
        })
    }

    fn rescue_clause(&mut self) -> PResult<Rescue> {
        let start = self.bump().span;
        let mut types = Vec::new();
        if matches!(self.kind(), T::Const(_)) {
            types.push(self.ty()?);
            while self.eat(&T::Comma) {
                types.push(self.ty()?);
            }
        }
        let binding = if self.eat(&T::FatArrow) { Some(self.ident("un nom de variable")?) } else { None };
        self.eat_kw(K::Then);
        let body = self.stmts(&[K::Rescue, K::Ensure, K::End]);
        Ok(Rescue { types, binding, body, span: start.to(self.prev_span()) })
    }

    /// Instruction : expression suivie d'éventuels modificateurs (`x if y`).
    fn stmt(&mut self) -> PResult<Expr> {
        let mut e = match self.multi_assign()? {
            Some(e) => e,
            None => self.expr_stmt()?,
        };
        while let T::Kw(kw @ (K::If | K::Unless | K::While | K::Until | K::Rescue)) = *self.kind() {
            let kw_span = self.bump().span;
            let rhs = self.expr_stmt()?;
            let span = e.span.to(rhs.span);
            let kind = match kw {
                K::If => ExprKind::If { cond: Box::new(rhs), then: vec![e], else_: None },
                K::Unless => ExprKind::If { cond: Box::new(not(rhs)), then: vec![e], else_: None },
                K::While => ExprKind::While { cond: Box::new(rhs), body: vec![e] },
                K::Until => ExprKind::While { cond: Box::new(not(rhs)), body: vec![e] },
                _ => {
                    let rescue =
                        Rescue { types: Vec::new(), binding: None, span: kw_span.to(rhs.span), body: vec![rhs] };
                    ExprKind::Begin(Body { stmts: vec![e], rescues: vec![rescue], ensure: None, span })
                }
            };
            e = Expr::new(kind, span);
        }
        Ok(e)
    }

    /// `a, b = valeur`
    fn multi_assign(&mut self) -> PResult<Option<Expr>> {
        let mut i = 0;
        loop {
            if !matches!(self.nth(i), T::Ident(_) | T::IVar(_)) {
                return Ok(None);
            }
            match self.nth(i + 1) {
                T::Comma => i += 2,
                T::Eq if i > 0 => break,
                _ => return Ok(None),
            }
        }
        let start = self.span();
        let mut targets = Vec::new();
        loop {
            let tok = self.bump();
            let kind = match tok.kind {
                T::Ident(name) => ExprKind::Var(name),
                T::IVar(name) => ExprKind::IVar(name),
                _ => unreachable!(),
            };
            targets.push(Expr::new(kind, tok.span));
            if self.bump().kind == T::Eq {
                break;
            }
        }
        self.skip_newlines();
        let value = self.expr()?;
        let span = start.to(value.span);
        Ok(Some(Expr::new(ExprKind::MultiAssign { targets, value: Box::new(value) }, span)))
    }

    /// Niveau `and` / `or` / `not` (plus faible que l'affectation, comme en Ruby).
    fn expr_stmt(&mut self) -> PResult<Expr> {
        let mut lhs = self.not_expr()?;
        loop {
            let op = if self.at_kw(K::And) {
                BinOp::And
            } else if self.at_kw(K::Or) {
                BinOp::Or
            } else {
                break;
            };
            self.bump();
            self.skip_newlines();
            let rhs = self.not_expr()?;
            let span = lhs.span.to(rhs.span);
            lhs = Expr::new(ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span);
        }
        Ok(lhs)
    }

    fn not_expr(&mut self) -> PResult<Expr> {
        if self.at_kw(K::Not) {
            let start = self.bump().span;
            let e = self.not_expr()?;
            let span = start.to(e.span);
            return Ok(Expr { span, ..not(e) });
        }
        self.expr()
    }

    // ── Expressions ──────────────────────────────────────────

    /// Affectation (associative à droite) puis opérateurs binaires.
    fn expr(&mut self) -> PResult<Expr> {
        let lhs = self.binary(0)?;
        let op = match self.kind() {
            T::Eq => None,
            T::PlusEq => Some(BinOp::Add),
            T::MinusEq => Some(BinOp::Sub),
            T::StarEq => Some(BinOp::Mul),
            T::SlashEq => Some(BinOp::Div),
            T::OrOrEq => Some(BinOp::Or),
            T::AndAndEq => Some(BinOp::And),
            _ => return Ok(lhs),
        };
        if !is_assignable(&lhs) {
            return self.fail(lhs.span, "cette expression ne peut pas être affectée");
        }
        self.bump();
        self.skip_newlines();
        let value = self.expr()?;
        let span = lhs.span.to(value.span);
        let (target, value) = (Box::new(lhs), Box::new(value));
        let kind = match op {
            None => ExprKind::Assign { target, value },
            Some(op) => ExprKind::OpAssign { op, target, value },
        };
        Ok(Expr::new(kind, span))
    }

    /// (opérateur, force à gauche, force à droite) — Pratt.
    fn infix(&self) -> Option<(Infix, u8, u8)> {
        use BinOp::*;
        let bin = |op, l, r| Some((Infix::Bin(op), l, r));
        match self.kind() {
            T::DotDot => Some((Infix::Range(true), 2, 3)),
            T::DotDotDot => Some((Infix::Range(false), 2, 3)),
            T::OrOr => bin(Or, 4, 5),
            T::AndAnd => bin(And, 6, 7),
            T::EqEq => bin(Eq, 8, 9),
            T::NotEq => bin(NotEq, 8, 9),
            T::Match => bin(Match, 8, 9),
            T::Cmp => bin(Cmp, 8, 9),
            T::Lt => bin(Lt, 10, 11),
            T::Le => bin(Le, 10, 11),
            T::Gt => bin(Gt, 10, 11),
            T::Ge => bin(Ge, 10, 11),
            T::Pipe => bin(BitOr, 12, 13),
            T::Caret => bin(BitXor, 12, 13),
            T::Amp => bin(BitAnd, 14, 15),
            T::Shl => bin(Shl, 16, 17),
            T::Shr => bin(Shr, 16, 17),
            T::Plus => bin(Add, 18, 19),
            T::Minus => bin(Sub, 18, 19),
            T::Star => bin(Mul, 20, 21),
            T::Slash => bin(Div, 20, 21),
            T::Percent => bin(Rem, 20, 21),
            T::StarStar => bin(Pow, 25, 24),
            _ => None,
        }
    }

    fn binary(&mut self, min_bp: u8) -> PResult<Expr> {
        let mut lhs = self.unary()?;
        while let Some((op, left_bp, right_bp)) = self.infix() {
            if left_bp < min_bp {
                break;
            }
            self.bump();
            self.skip_newlines();
            let rhs = self.binary(right_bp)?;
            let span = lhs.span.to(rhs.span);
            let (lhs_box, rhs_box) = (Box::new(lhs), Box::new(rhs));
            let kind = match op {
                Infix::Bin(op) => ExprKind::Binary { op, lhs: lhs_box, rhs: rhs_box },
                Infix::Range(inclusive) => ExprKind::Range { lo: lhs_box, hi: rhs_box, inclusive },
            };
            lhs = Expr::new(kind, span);
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> PResult<Expr> {
        let op = match self.kind() {
            T::Minus => UnOp::Neg,
            T::Bang => UnOp::Not,
            _ => return self.postfix(),
        };
        let start = self.bump().span;
        // `-2 ** 2` vaut -(2 ** 2), comme en Ruby
        let e = if op == UnOp::Neg { self.binary(24)? } else { self.unary()? };
        let span = start.to(e.span);
        Ok(Expr::new(ExprKind::Unary { op, expr: Box::new(e) }, span))
    }

    fn postfix(&mut self) -> PResult<Expr> {
        let e = self.primary()?;
        self.postfix_from(e)
    }

    fn postfix_from(&mut self, mut e: Expr) -> PResult<Expr> {
        loop {
            match self.kind() {
                T::Dot | T::SafeDot => {
                    let safe = self.bump().kind == T::SafeDot;
                    e = self.method_call(e, safe)?;
                }
                T::LBracket if !self.peek().space_before => {
                    self.bump();
                    let args = self.expr_list(T::RBracket, "`]`")?;
                    let span = e.span.to(self.prev_span());
                    e = Expr::new(ExprKind::Index { recv: Box::new(e), args }, span);
                }
                T::Question if !self.peek().space_before => {
                    let span = e.span.to(self.bump().span);
                    e = Expr::new(ExprKind::Try(Box::new(e)), span);
                }
                T::LBrace if takes_block(&e) => {
                    let block = self.brace_block()?;
                    e = attach_block(e, block);
                }
                T::Kw(K::Do) if !self.no_do && takes_block(&e) => {
                    let block = self.do_block()?;
                    e = attach_block(e, block);
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn method_call(&mut self, recv: Expr, safe: bool) -> PResult<Expr> {
        let name = self.method_name()?;
        let (args, block, parens) = if self.at_tight(&T::LParen) {
            let (args, block) = self.paren_args()?;
            (args, block, true)
        } else if self.can_start_command_arg() {
            let (args, block) = self.command_args()?;
            (args, block, false)
        } else {
            (Vec::new(), None, false)
        };
        let span = recv.span.to(self.prev_span());
        let kind = ExprKind::Call { recv: Some(Box::new(recv)), name, args, block: block.map(Box::new), safe, parens };
        Ok(Expr::new(kind, span))
    }

    /// Un identifiant suivi, après un blanc, d'un de ces tokens est un appel sans parenthèses.
    fn can_start_command_arg(&self) -> bool {
        let tok = self.peek();
        tok.space_before
            && matches!(
                tok.kind,
                T::Int(_)
                    | T::Float(_)
                    | T::Str(_)
                    | T::Symbol(_)
                    | T::Label(_)
                    | T::Ident(_)
                    | T::Const(_)
                    | T::IVar(_)
                    | T::LBracket
                    | T::LParen
                    | T::Kw(K::True | K::False | K::Nil | K::SelfKw)
            )
    }

    /// Arguments sans parenthèses : `do` n'y est pas consommé et revient à cet appel.
    fn command_args(&mut self) -> PResult<(Vec<Arg>, Option<Block>)> {
        let (args, mut block) = self.with_do(false, |p| p.arg_list())?;
        if block.is_none() {
            if self.at(&T::LBrace) {
                block = Some(self.brace_block()?);
            } else if !self.no_do && self.at_kw(K::Do) {
                block = Some(self.do_block()?);
            }
        }
        Ok((args, block))
    }

    fn arg_list(&mut self) -> PResult<(Vec<Arg>, Option<Block>)> {
        let mut args = Vec::new();
        let mut block = None;
        loop {
            match self.arg()? {
                ArgOrBlock::Arg(arg) => args.push(arg),
                ArgOrBlock::Block(b) => block = Some(b),
            }
            if !self.eat(&T::Comma) {
                break;
            }
            self.skip_newlines();
        }
        Ok((args, block))
    }

    fn paren_args(&mut self) -> PResult<(Vec<Arg>, Option<Block>)> {
        self.expect(T::LParen, "`(`")?;
        self.with_do(true, |p| {
            let mut args = Vec::new();
            let mut block = None;
            loop {
                p.skip_newlines();
                if p.at(&T::RParen) {
                    break;
                }
                match p.arg()? {
                    ArgOrBlock::Arg(arg) => args.push(arg),
                    ArgOrBlock::Block(b) => block = Some(b),
                }
                p.skip_newlines();
                if !p.eat(&T::Comma) {
                    break;
                }
            }
            p.skip_newlines();
            p.expect(T::RParen, "`)`")?;
            Ok((args, block))
        })
    }

    fn arg(&mut self) -> PResult<ArgOrBlock> {
        match self.kind().clone() {
            T::Label(name) => {
                let name = Ident { name, span: self.bump().span };
                let punned =
                    matches!(self.kind(), T::Comma | T::RParen | T::RBracket | T::RBrace | T::Newline | T::Eof)
                        || self.at_kw(K::Do);
                let value = if punned { None } else { Some(self.expr()?) };
                Ok(ArgOrBlock::Arg(Arg::Named { name, value }))
            }
            T::Amp => {
                self.bump();
                Ok(ArgOrBlock::Arg(Arg::BlockPass(self.unary()?)))
            }
            // `&.sent?` : bloc court sur le paramètre implicite `it`
            T::SafeDot => {
                let start = self.bump().span;
                let call = self.method_call(Expr::new(ExprKind::It, start), false)?;
                let body = self.postfix_from(call)?;
                let span = start.to(body.span);
                let it = Param { name: Ident { name: "it".into(), span: start }, ty: None, default: None, span: start };
                Ok(ArgOrBlock::Block(Block { params: vec![it], body: Body::new(vec![body], span), span }))
            }
            _ => Ok(ArgOrBlock::Arg(Arg::Pos(self.expr()?))),
        }
    }

    fn expr_list(&mut self, close: T, expected: &str) -> PResult<Vec<Expr>> {
        self.with_do(true, |p| {
            let mut items = Vec::new();
            loop {
                p.skip_newlines();
                if p.at(&close) {
                    break;
                }
                items.push(p.expr()?);
                p.skip_newlines();
                if !p.eat(&T::Comma) {
                    break;
                }
            }
            p.skip_newlines();
            p.expect(close, expected)?;
            Ok(items)
        })
    }

    fn primary(&mut self) -> PResult<Expr> {
        let tok = self.peek().clone();
        let simple = |p: &mut Self, kind| {
            p.bump();
            Ok(Expr::new(kind, tok.span))
        };
        match tok.kind {
            T::Int(n) => simple(self, ExprKind::Int(n)),
            T::Float(n) => simple(self, ExprKind::Float(n)),
            T::Symbol(ref s) => simple(self, ExprKind::Symbol(s.clone())),
            T::IVar(ref s) => simple(self, ExprKind::IVar(s.clone())),
            T::Kw(K::True) => simple(self, ExprKind::Bool(true)),
            T::Kw(K::False) => simple(self, ExprKind::Bool(false)),
            T::Kw(K::Nil) => simple(self, ExprKind::Nil),
            T::Kw(K::SelfKw) => simple(self, ExprKind::SelfRef),
            T::Str(parts) => {
                self.bump();
                Ok(self.string(parts, tok.span))
            }
            T::Ident(_) => self.ident_expr(),
            T::Const(_) => self.const_expr(),
            T::LParen => {
                self.bump();
                let inner = self.with_do(true, |p| {
                    p.skip_newlines();
                    let e = p.stmt()?;
                    p.skip_newlines();
                    p.expect(T::RParen, "`)`")?;
                    Ok(e)
                })?;
                Ok(Expr { span: tok.span.to(self.prev_span()), ..inner })
            }
            T::LBracket => {
                self.bump();
                let items = self.expr_list(T::RBracket, "`]`")?;
                Ok(Expr::new(ExprKind::Array(items), tok.span.to(self.prev_span())))
            }
            T::LBrace => self.hash(),
            T::Kw(K::If) => self.if_expr(false),
            T::Kw(K::Unless) => self.if_expr(true),
            T::Kw(K::While) => self.while_expr(false),
            T::Kw(K::Until) => self.while_expr(true),
            T::Kw(K::Case) => self.case_expr(),
            T::Kw(K::Begin) => {
                self.bump();
                let body = self.body(&[])?;
                let end = self.expect_end(tok.span, "begin")?;
                Ok(Expr::new(ExprKind::Begin(body), tok.span.to(end)))
            }
            T::Kw(K::Return | K::Break | K::Next) => self.jump(),
            T::Kw(K::End) => self.fail(tok.span, "`end` sans bloc ouvrant"),
            T::Kw(
                kw @ (K::Def
                | K::Abstract
                | K::Prompt
                | K::Tool
                | K::Workflow
                | K::Struct
                | K::Class
                | K::Module
                | K::Enum
                | K::Agent
                | K::Supervisor),
            ) => self.fail(tok.span, format!("`{}` n'est autorisé qu'au niveau supérieur", kw.as_str())),
            T::Label(name) => self.fail(tok.span, format!("argument nommé `{name}:` inattendu ici")),
            _ => self.unexpected("une expression"),
        }
    }

    fn string(&mut self, parts: Vec<StrPart>, span: Span) -> Expr {
        let segs = parts
            .into_iter()
            .map(|part| match part {
                StrPart::Lit(text) => StrSeg::Lit(text),
                StrPart::Interp(tokens, span) => StrSeg::Interp(self.interpolation(tokens, span)),
            })
            .collect();
        Expr::new(ExprKind::Str(segs), span)
    }

    fn interpolation(&mut self, tokens: Vec<Token>, span: Span) -> Expr {
        let mut sub = Parser::new(tokens, self.docs);
        let mut stmts = sub.stmts(&[]);
        if !sub.at(&T::Eof) {
            let _ = sub.unexpected::<()>("`}`");
        }
        self.diags.append(&mut sub.diags);
        if stmts.len() > 1 {
            self.report(Diagnostic::new(span, "une seule expression attendue dans `#{…}`"));
        }
        stmts.pop().unwrap_or_else(|| Expr::new(ExprKind::Str(Vec::new()), span))
    }

    fn ident_expr(&mut self) -> PResult<Expr> {
        let tok = self.bump();
        let T::Ident(name) = tok.kind else { unreachable!() };
        let name = Ident { name, span: tok.span };
        let (args, block, parens) = if self.at_tight(&T::LParen) {
            let (args, block) = self.paren_args()?;
            (args, block, true)
        } else if self.can_start_command_arg() {
            let (args, block) = self.command_args()?;
            (args, block, false)
        } else {
            return Ok(Expr::new(ExprKind::Var(name.name), tok.span));
        };
        let span = tok.span.to(self.prev_span());
        Ok(Expr::new(ExprKind::Call { recv: None, name, args, block: block.map(Box::new), safe: false, parens }, span))
    }

    /// `Foo`, `A::B`, ou `Research(topic: t)` (construction / appel de type).
    fn const_expr(&mut self) -> PResult<Expr> {
        let start = self.span();
        let mut path = vec![self.const_name("une constante")?];
        while self.at(&T::ColonColon) && matches!(self.nth(1), T::Const(_)) {
            self.bump();
            path.push(self.const_name("une constante")?);
        }
        if !self.at_tight(&T::LParen) {
            return Ok(Expr::new(ExprKind::Const(path), start.to(self.prev_span())));
        }
        let name = path.pop().expect("chemin non vide");
        let recv = path.last().map(|last| Box::new(Expr::new(ExprKind::Const(path.clone()), start.to(last.span))));
        let (args, block) = self.paren_args()?;
        let kind = ExprKind::Call { recv, name, args, block: block.map(Box::new), safe: false, parens: true };
        Ok(Expr::new(kind, start.to(self.prev_span())))
    }

    fn hash(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let entries = self.with_do(true, |p| {
            let mut entries = Vec::new();
            loop {
                p.skip_newlines();
                if p.at(&T::RBrace) {
                    break;
                }
                if let T::Label(name) = p.kind().clone() {
                    let key = Expr::new(ExprKind::Symbol(name), p.bump().span);
                    p.skip_newlines();
                    entries.push((key, p.expr()?));
                } else {
                    let key = p.expr()?;
                    p.expect(T::FatArrow, "`=>`")?;
                    p.skip_newlines();
                    entries.push((key, p.expr()?));
                }
                p.skip_newlines();
                if !p.eat(&T::Comma) {
                    break;
                }
            }
            p.skip_newlines();
            p.expect(T::RBrace, "`}`")?;
            Ok(entries)
        })?;
        Ok(Expr::new(ExprKind::Hash(entries), start.to(self.prev_span())))
    }

    fn if_expr(&mut self, negate: bool) -> PResult<Expr> {
        let start = self.bump().span;
        let e = self.if_rest(start, negate)?;
        let end = self.expect_end(start, if negate { "unless" } else { "if" })?;
        Ok(Expr { span: start.to(end), ..e })
    }

    /// Condition, branche `then`, puis `elsif`/`else` ; ne consomme pas le `end`.
    fn if_rest(&mut self, start: Span, negate: bool) -> PResult<Expr> {
        let cond = self.expr_stmt()?;
        let cond = if negate { not(cond) } else { cond };
        self.eat_kw(K::Then);
        let then = self.stmts(&[K::Elsif, K::Else, K::End]);
        let else_ = if self.at_kw(K::Elsif) {
            let elsif = self.bump().span;
            Some(vec![self.if_rest(elsif, false)?])
        } else if self.eat_kw(K::Else) {
            Some(self.stmts(&[K::End]))
        } else {
            None
        };
        let kind = ExprKind::If { cond: Box::new(cond), then, else_ };
        Ok(Expr::new(kind, start.to(self.prev_span())))
    }

    fn while_expr(&mut self, until: bool) -> PResult<Expr> {
        let start = self.bump().span;
        let cond = self.with_do(false, |p| p.expr_stmt())?;
        let cond = if until { not(cond) } else { cond };
        self.eat_kw(K::Do);
        let body = self.stmts(&[K::End]);
        let end = self.expect_end(start, if until { "until" } else { "while" })?;
        Ok(Expr::new(ExprKind::While { cond: Box::new(cond), body }, start.to(end)))
    }

    fn case_expr(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let subject = if self.at_line_end() { None } else { Some(Box::new(self.expr_stmt()?)) };
        self.skip_newlines();

        let mut arms = Vec::new();
        loop {
            let arm_start = self.span();
            let test = if self.eat_kw(K::In) {
                ArmTest::In(self.pattern()?)
            } else if self.eat_kw(K::When) {
                let mut values = vec![self.expr()?];
                while self.eat(&T::Comma) {
                    self.skip_newlines();
                    values.push(self.expr()?);
                }
                ArmTest::When(values)
            } else {
                break;
            };
            let guard = if self.eat_kw(K::If) {
                Some(self.expr_stmt()?)
            } else if self.eat_kw(K::Unless) {
                Some(not(self.expr_stmt()?))
            } else {
                None
            };
            self.eat_kw(K::Then);
            let body = self.stmts(&[K::In, K::When, K::Else, K::End]);
            arms.push(CaseArm { test, guard, body, span: arm_start.to(self.prev_span()) });
        }
        if arms.is_empty() {
            self.report(Diagnostic::new(self.span(), "au moins une branche `in` ou `when` attendue"));
        }
        let else_ = if self.eat_kw(K::Else) { Some(self.stmts(&[K::End])) } else { None };
        let end = self.expect_end(start, "case")?;
        Ok(Expr::new(ExprKind::Case { subject, arms, else_ }, start.to(end)))
    }

    fn jump(&mut self) -> PResult<Expr> {
        let tok = self.bump();
        let has_value = !matches!(
            self.kind(),
            T::Newline
                | T::Eof
                | T::RBrace
                | T::RParen
                | T::Kw(
                    K::If
                        | K::Unless
                        | K::While
                        | K::Until
                        | K::Rescue
                        | K::End
                        | K::Else
                        | K::Elsif
                        | K::In
                        | K::When
                        | K::Then
                        | K::Do
                        | K::Ensure
                )
        );
        let value = if has_value { Some(Box::new(self.expr()?)) } else { None };
        let span = tok.span.to(self.prev_span());
        let kind = match tok.kind {
            T::Kw(K::Return) => ExprKind::Return(value),
            T::Kw(K::Break) => ExprKind::Break(value),
            _ => ExprKind::Next(value),
        };
        Ok(Expr::new(kind, span))
    }

    // ── Blocs ────────────────────────────────────────────────

    fn brace_block(&mut self) -> PResult<Block> {
        let start = self.bump().span;
        let params = self.block_params()?;
        let body_start = self.span();
        let stmts = self.stmts(&[]);
        let end = self.expect(T::RBrace, "`}`")?;
        Ok(Block { params, body: Body::new(stmts, body_start.to(self.prev_span())), span: start.to(end) })
    }

    fn do_block(&mut self) -> PResult<Block> {
        let start = self.bump().span;
        let params = self.block_params()?;
        let body = self.body(&[])?;
        let end = self.expect_end(start, "do")?;
        Ok(Block { params, body, span: start.to(end) })
    }

    /// `|a, b|`, `|x: Int|` ou rien.
    fn block_params(&mut self) -> PResult<Vec<Param>> {
        if self.eat(&T::OrOr) || !self.eat(&T::Pipe) {
            return Ok(Vec::new());
        }
        let mut params = Vec::new();
        loop {
            let start = self.span();
            let (name, ty) = match self.kind().clone() {
                T::Ident(name) => {
                    self.bump();
                    (name, None)
                }
                T::Label(name) => {
                    self.bump();
                    (name, Some(self.ty()?))
                }
                _ => return self.unexpected("un paramètre de bloc"),
            };
            params.push(Param {
                name: Ident { name, span: start },
                ty,
                default: None,
                span: start.to(self.prev_span()),
            });
            if !self.eat(&T::Comma) {
                break;
            }
        }
        self.expect(T::Pipe, "`|`")?;
        Ok(params)
    }

    // ── Motifs ───────────────────────────────────────────────

    fn pattern(&mut self) -> PResult<Pattern> {
        let first = self.pattern_atom()?;
        if !self.at(&T::Pipe) {
            return Ok(first);
        }
        let mut alts = vec![first];
        while self.eat(&T::Pipe) {
            alts.push(self.pattern_atom()?);
        }
        let span = alts[0].span.to(alts[alts.len() - 1].span);
        Ok(Pattern { kind: PatternKind::Or(alts), span })
    }

    fn pattern_atom(&mut self) -> PResult<Pattern> {
        let start = self.span();
        let kind = match self.kind().clone() {
            T::Ident(name) => {
                self.bump();
                if name == "_" { PatternKind::Wildcard } else { PatternKind::Bind(name) }
            }
            T::Int(_) | T::Float(_) | T::Str(_) | T::Symbol(_) | T::Kw(K::True | K::False | K::Nil) => {
                PatternKind::Lit(self.primary()?)
            }
            T::Minus => PatternKind::Lit(self.unary()?),
            T::Const(_) => {
                let mut path = vec![self.const_name("une constante")?];
                while self.eat(&T::ColonColon) {
                    path.push(self.const_name("une constante")?);
                }
                let fields = if self.at_tight(&T::LParen) { Some(self.pattern_fields()?) } else { None };
                PatternKind::Const { path, fields }
            }
            T::LBracket => {
                self.bump();
                let mut items = Vec::new();
                loop {
                    self.skip_newlines();
                    if self.at(&T::RBracket) {
                        break;
                    }
                    items.push(self.pattern()?);
                    if !self.eat(&T::Comma) {
                        break;
                    }
                }
                self.skip_newlines();
                self.expect(T::RBracket, "`]`")?;
                PatternKind::Array(items)
            }
            _ => return self.unexpected("un motif"),
        };
        Ok(Pattern { kind, span: start.to(self.prev_span()) })
    }

    /// `(r)`, `(w, h)`, `(category: Spam)`, `(category:)`
    fn pattern_fields(&mut self) -> PResult<Vec<PatField>> {
        self.bump();
        let mut fields = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&T::RParen) {
                break;
            }
            if let T::Label(name) = self.kind().clone() {
                let span = self.bump().span;
                let pattern = if matches!(self.kind(), T::Comma | T::RParen) {
                    Pattern { kind: PatternKind::Bind(name.clone()), span }
                } else {
                    self.pattern()?
                };
                fields.push(PatField { name: Some(Ident { name, span }), pattern });
            } else {
                fields.push(PatField { name: None, pattern: self.pattern()? });
            }
            self.skip_newlines();
            if !self.eat(&T::Comma) {
                break;
            }
        }
        self.skip_newlines();
        self.expect(T::RParen, "`)`")?;
        Ok(fields)
    }
}
