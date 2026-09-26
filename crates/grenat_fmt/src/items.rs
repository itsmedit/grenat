//! Declarations: functions, types and their members, models.

use grenat_ast::{
    Directive, Effect, Field, FnDef, FnKind, Handler, Item, MacroDef, Member, ModelDecl, Program, Span, TypeDef,
    TypeKind, Variant,
};

use crate::printer::Printer;

impl Printer<'_> {
    pub(crate) fn program(&mut self, program: &Program) {
        for item in &program.items {
            let span = item_span(item);
            self.line_element(span.start);
            match item {
                Item::Fn(def) => self.fn_def(def),
                Item::Type(def) => self.type_def(def),
                Item::Model(decl) => self.model(decl),
                Item::Macro(def) => self.macro_def(def),
                Item::Stmt(e) => self.stmt(e),
            }
            self.line_end(span.end);
            self.newline();
        }
    }

    fn fn_def(&mut self, def: &FnDef) {
        // `API = "…"`: a constant, written so
        let constant = def.name.name.starts_with(|c: char| c.is_ascii_uppercase());
        if constant && def.short && def.params.is_empty() && !self.source(def.span).starts_with("def") {
            self.write(&def.name.name);
            self.write(" = ");
            self.expr(&def.body.stmts[0]);
            return;
        }
        if def.is_abstract {
            self.write("abstract ");
        }
        self.write(match def.kind {
            FnKind::Def => "def ",
            FnKind::Prompt => "prompt ",
            FnKind::Tool => "tool ",
            FnKind::Workflow => "workflow ",
        });
        if def.on_self {
            self.write("self.");
        }
        self.write(&def.name.name);
        if !def.params.is_empty() {
            self.params(&def.params);
        }
        if let Some(ret) = &def.ret {
            self.write(" -> ");
            self.ty(ret);
        }
        if !def.effects.is_empty() {
            self.write(" uses ");
            for (i, effect) in def.effects.iter().enumerate() {
                if i > 0 {
                    self.write(", ");
                }
                self.effect(effect);
            }
        }
        if let Some(model) = &def.model {
            self.write(" using ");
            self.expr(model);
        }
        if def.is_abstract {
            return;
        }
        if def.short {
            self.write(" = ");
            self.stmt(&def.body.stmts[0]);
            return;
        }
        self.line_end(def.span.start);
        self.newline();
        self.body(&def.body, def.span.end);
        self.write("end");
    }

    fn effect(&mut self, effect: &Effect) {
        let path = effect.path.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join(".");
        self.write(&path);
        if !effect.args.is_empty() {
            self.write("(");
            for (i, arg) in effect.args.iter().enumerate() {
                if i > 0 {
                    self.write(", ");
                }
                self.expr(arg);
            }
            self.write(")");
        }
    }

    fn type_def(&mut self, def: &TypeDef) {
        self.write(match def.kind {
            TypeKind::Struct => "struct ",
            TypeKind::Class => "class ",
            TypeKind::Module => "module ",
            TypeKind::Enum => "enum ",
            TypeKind::Agent => "agent ",
            TypeKind::Supervisor => "supervisor ",
        });
        self.write(&def.name.name);
        self.options(&def.options);
        self.line_end(def.span.start);
        self.newline();
        self.indented(|p| {
            for member in &def.members {
                let span = member_span(member);
                p.line_element(span.start);
                p.member(member);
                p.line_end(span.end);
                p.newline();
            }
            p.comments_before(def.span.end);
        });
        self.write("end");
    }

    /// `, key: value, …` after a declaration's name.
    fn options(&mut self, options: &[grenat_ast::Arg]) {
        for arg in options {
            self.write(", ");
            self.bare_arg(arg);
        }
    }

    fn bare_arg(&mut self, arg: &grenat_ast::Arg) {
        match arg {
            grenat_ast::Arg::Pos(e) => self.expr(e),
            grenat_ast::Arg::Named { name, value } => {
                self.write(&name.name);
                self.write(":");
                if let Some(value) = value {
                    self.write(" ");
                    self.expr(value);
                }
            }
            grenat_ast::Arg::BlockPass(e) => {
                self.write("&");
                self.expr(e);
            }
        }
    }

    fn member(&mut self, member: &Member) {
        match member {
            Member::Field(field) => self.field(field),
            Member::Method(def) => self.fn_def(def),
            Member::Include(ty) => {
                self.write("include ");
                self.ty(ty);
            }
            Member::Variant(variant) => self.variant(variant),
            Member::Handler(handler) => self.handler(handler),
            Member::Directive(directive) => self.directive(directive),
        }
    }

    fn field(&mut self, field: &Field) {
        if field.is_ivar {
            self.write("@");
            self.write(&field.name.name);
            if let Some(ty) = &field.ty {
                self.write(": ");
                self.ty(ty);
            }
        } else {
            self.write(&field.name.name);
            self.write(": ");
            if let Some(ty) = &field.ty {
                self.ty(ty);
            }
        }
        if let Some(default) = &field.default {
            self.write(" = ");
            self.expr(default);
        }
    }

    fn variant(&mut self, variant: &Variant) {
        self.write(&variant.name.name);
        if !variant.fields.is_empty() {
            self.write("(");
            for (i, field) in variant.fields.iter().enumerate() {
                if i > 0 {
                    self.write(", ");
                }
                self.field(field);
            }
            self.write(")");
        }
    }

    fn handler(&mut self, handler: &Handler) {
        self.write("on ");
        self.write(&handler.message.name);
        if !handler.params.is_empty() {
            self.params(&handler.params);
        }
        if let Some(ret) = &handler.ret {
            self.write(" -> ");
            self.ty(ret);
        }
        self.line_end(handler.span.start);
        self.newline();
        self.body(&handler.body, handler.span.end);
        self.write("end");
    }

    fn directive(&mut self, directive: &Directive) {
        self.write(&directive.name.name);
        for (i, arg) in directive.args.iter().enumerate() {
            self.write(if i == 0 { " " } else { ", " });
            self.bare_arg(arg);
        }
    }

    /// A macro's template is printed as it was written.
    fn macro_def(&mut self, def: &MacroDef) {
        self.write("macro ");
        self.write(&def.name.name);
        if !def.params.is_empty() {
            let params: Vec<String> =
                def.params.iter().map(|p| format!("{}{}", if p.variadic { "*" } else { "" }, p.name.name)).collect();
            self.write(&format!("({})", params.join(", ")));
        }
        self.newline();
        self.out.push_str(&def.body);
        self.write("end");
    }

    fn model(&mut self, decl: &ModelDecl) {
        self.write("model :");
        self.write(&decl.name.name);
        self.options(&decl.options);
    }
}

fn item_span(item: &Item) -> Span {
    match item {
        Item::Fn(def) => def.span,
        Item::Type(def) => def.span,
        Item::Model(decl) => decl.span,
        Item::Macro(def) => def.span,
        Item::Stmt(e) => e.span,
    }
}

fn member_span(member: &Member) -> Span {
    match member {
        Member::Field(f) => f.span,
        Member::Method(def) => def.span,
        Member::Include(ty) => ty.span(),
        Member::Variant(v) => v.span,
        Member::Handler(h) => h.span,
        Member::Directive(d) => d.span,
    }
}
