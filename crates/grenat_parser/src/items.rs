//! Declarations: functions, `prompt`, `tool`, types, agents, supervisors, models.

use grenat_ast::*;
use grenat_lexer::{Keyword as K, TokenKind as T};

use crate::*;

impl<'d> Parser<'d> {
    pub(crate) fn program(&mut self) -> Program {
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
                        let _ = self.unexpected::<()>("end of line");
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

    pub(crate) fn item(&mut self) -> PResult<Item> {
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
            T::Ident(name) if name == "macro" && matches!(self.nth(1), T::Ident(_)) => {
                self.macro_def(doc).map(Item::Macro)
            }
            _ => self.stmt().map(Item::Stmt),
        }
    }

    pub(crate) fn fn_def(&mut self, doc: Option<String>) -> PResult<FnDef> {
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
        let name = self.ident("a function name")?;
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

    pub(crate) fn params(&mut self) -> PResult<Vec<Param>> {
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

    /// `NAME = expr` in a type: the class method `NAME`, without arguments.
    pub(crate) fn constant(&mut self, doc: Option<String>) -> PResult<FnDef> {
        let start = self.span();
        let name = self.const_name("a constant")?;
        self.expect(T::Eq, "`=`")?;
        self.skip_newlines();
        let value = self.expr()?;
        let span = value.span;
        Ok(FnDef {
            kind: FnKind::Def,
            doc,
            is_abstract: false,
            on_self: true,
            name,
            params: Vec::new(),
            ret: None,
            effects: Vec::new(),
            model: None,
            body: Body::new(vec![value], span),
            short: true,
            span: start.to(self.prev_span()),
        })
    }

    /// `name: Type = default` or `name = default`.
    pub(crate) fn param(&mut self) -> PResult<Param> {
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
            _ => return self.unexpected("a parameter `name: Type`"),
        };
        let default = if self.eat(&T::Eq) { Some(self.expr()?) } else { None };
        Ok(Param { name: Ident { name, span: start }, ty, default, span: start.to(self.prev_span()) })
    }

    /// `llm, net("host"), fs.read("./docs")`
    pub(crate) fn effects(&mut self) -> PResult<Vec<Effect>> {
        let mut effects = Vec::new();
        loop {
            let first = self.ident("an effect (`llm`, `net`, `fs.read`…)")?;
            let start = first.span;
            let mut path = vec![first];
            while self.at(&T::Dot) && matches!(self.nth(1), T::Ident(_)) {
                self.bump();
                path.push(self.ident("an effect")?);
            }
            let mut args = Vec::new();
            if self.at_tight(&T::LParen) {
                for arg in self.paren_args()?.0 {
                    match arg {
                        Arg::Pos(e) => args.push(e),
                        Arg::Named { name, .. } => {
                            return self.fail(name.span, "effect restrictions are positional");
                        }
                        Arg::BlockPass(e) => return self.fail(e.span, "unexpected block in an effect"),
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

    /// `macro name(a, *rest)`, its body (read raw by the lexer), `end`.
    pub(crate) fn macro_def(&mut self, doc: Option<String>) -> PResult<MacroDef> {
        let start = self.bump().span;
        let name = self.ident("a macro name")?;
        let mut params = Vec::new();
        if self.at_tight(&T::LParen) {
            self.bump();
            while !self.at(&T::RParen) {
                let variadic = self.eat(&T::Star);
                params.push(MacroParam { name: self.ident("a parameter name")?, variadic });
                if !self.eat(&T::Comma) {
                    break;
                }
            }
            self.expect(T::RParen, "`)`")?;
        }
        if params.iter().filter(|p| p.variadic).count() > 1 || params.iter().rev().skip(1).any(|p| p.variadic) {
            return self.fail(start.to(self.prev_span()), "only the last parameter of a macro can be `*variadic`");
        }
        self.expect(T::Newline, "the end of the line")?;
        let (body, body_span) = match self.kind().clone() {
            T::MacroBody(body) => (body, self.bump().span),
            _ => return self.unexpected("the macro's body"),
        };
        let end = self.expect_end(start, "macro")?;
        Ok(MacroDef { doc, name, params, body, body_span, span: start.to(end) })
    }

    pub(crate) fn model_decl(&mut self) -> PResult<ModelDecl> {
        let start = self.bump().span;
        let name = match self.kind().clone() {
            T::Symbol(name) => Ident { name, span: self.bump().span },
            _ => return self.unexpected("a symbol (`:fast`)"),
        };
        let options = if self.eat(&T::Comma) {
            self.skip_newlines();
            self.arg_list()?.0
        } else {
            Vec::new()
        };
        Ok(ModelDecl { name, options, span: start.to(self.prev_span()) })
    }

    pub(crate) fn type_def(&mut self, kind: TypeKind, doc: Option<String>) -> PResult<TypeDef> {
        let keyword = self.bump();
        let name = self.const_name("a capitalized name")?;
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

    pub(crate) fn members(&mut self, kind: TypeKind) -> Vec<Member> {
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
                        let _ = self.unexpected::<()>("end of line");
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

    pub(crate) fn member(&mut self, kind: TypeKind) -> PResult<Member> {
        let start = self.span();
        let doc = self.docs.doc_for(start);
        match self.kind().clone() {
            T::Label(_) => self.field(doc).map(Member::Field),
            T::IVar(name) => {
                self.bump();
                let ty = if self.eat(&T::Colon) { Some(self.ty()?) } else { None };
                let default = if self.eat(&T::Eq) { Some(self.expr()?) } else { None };
                if ty.is_none() && default.is_none() {
                    return self
                        .fail(start, format!("`@{name}`: expected a type (`@{name}: Type`) or an initial value"));
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
            // `API = "…"`: a constant, i.e. a `def self.API` without arguments
            T::Const(_) if *self.nth(1) == T::Eq => self.constant(doc).map(Member::Method),
            T::Const(_) if kind == TypeKind::Enum => self.variant(doc).map(Member::Variant),
            // a directive, or a macro invocation (see `grenat_macros`)
            T::Ident(_) => {
                let name = self.ident("a directive")?;
                let args = if self.at_line_end() { Vec::new() } else { self.command_args()?.0 };
                Ok(Member::Directive(Directive { name, args, span: start.to(self.prev_span()) }))
            }
            _ => self.unexpected(match kind {
                TypeKind::Enum => "a variant, a method or `end`",
                TypeKind::Agent => "a directive, `@state`, `on Message`, a method or `end`",
                TypeKind::Supervisor => "a directive (`child …`) or `end`",
                _ => "a field `name: Type`, a method or `end`",
            }),
        }
    }

    /// `name: Type = default`
    pub(crate) fn field(&mut self, doc: Option<String>) -> PResult<Field> {
        let start = self.span();
        let T::Label(name) = self.kind().clone() else {
            return self.unexpected("a field `name: Type`");
        };
        self.bump();
        let ty = self.ty()?;
        let default = if self.eat(&T::Eq) { Some(self.expr()?) } else { None };
        let span = start.to(self.prev_span());
        Ok(Field { doc, name: Ident { name, span: start }, is_ivar: false, ty: Some(ty), default, span })
    }

    pub(crate) fn variant(&mut self, doc: Option<String>) -> PResult<Variant> {
        let name = self.const_name("a variant")?;
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

    pub(crate) fn handler(&mut self, doc: Option<String>) -> PResult<Handler> {
        let start = self.bump().span;
        let message = self.const_name("a capitalized message name")?;
        let params = if self.at_tight(&T::LParen) { self.params()? } else { Vec::new() };
        let ret = if self.eat(&T::Arrow) { Some(self.ty()?) } else { None };
        let body = self.body(&[])?;
        let end = self.expect_end(start, "on")?;
        Ok(Handler { doc, message, params, ret, body, span: start.to(end) })
    }
}
