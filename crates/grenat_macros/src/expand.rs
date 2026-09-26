//! Expanding the macro invocations of a program.

use std::collections::HashMap;

use grenat_ast::{Arg, Diagnostic, Expr, ExprKind, Item, MacroDef, Member, Program, Span, TypeKind};

use crate::MAX_DEPTH;
use crate::template::{Binding, render};

/// Where an invocation is, hence what its expansion declares.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Context {
    TopLevel,
    /// In the body of a type of this kind.
    Members(TypeKind),
}

/// A macro invocation: `name args…`, as a statement or a type member.
struct Invocation {
    name: String,
    args: Vec<Arg>,
    span: Span,
    context: Context,
}

/// Replaces the macro invocations of `program` (parsed from `text`) by
/// their expansions; returns the problems found.
pub fn expand(program: &mut Program, text: &str) -> Vec<Diagnostic> {
    let mut macros: HashMap<String, MacroDef> = HashMap::new();
    let mut diagnostics = Vec::new();
    for item in &program.items {
        if let Item::Macro(def) = item
            && macros.insert(def.name.name.clone(), def.clone()).is_some()
        {
            diagnostics.push(Diagnostic::new(def.name.span, format!("macro `{}` is defined twice", def.name.name)));
        }
    }
    let expander = Expander { macros };
    let items = std::mem::take(&mut program.items);
    for mut item in items {
        if let Item::Stmt(expr) = &item
            && let Some(invocation) = expander.invocation(expr)
        {
            match expander.expand(&invocation, text, 1).and_then(|code| parse_items(&code, &invocation)) {
                Ok(items) => program.items.extend(items),
                Err(diagnostic) => diagnostics.push(diagnostic),
            }
            continue;
        }
        if let Item::Type(def) = &mut item {
            let members = std::mem::take(&mut def.members);
            for member in members {
                match member {
                    Member::Directive(d) if expander.macros.contains_key(&d.name.name) => {
                        let invocation = Invocation {
                            name: d.name.name,
                            args: d.args,
                            span: d.span,
                            context: Context::Members(def.kind),
                        };
                        match expander
                            .expand(&invocation, text, 1)
                            .and_then(|code| parse_members(&code, &invocation, def.kind))
                        {
                            Ok(members) => def.members.extend(members),
                            Err(diagnostic) => diagnostics.push(diagnostic),
                        }
                    }
                    // `table :tickets` makes a struct a record
                    Member::Directive(d) if d.name.name == "table" && def.kind == TypeKind::Struct => {
                        def.members.push(Member::Directive(d));
                    }
                    Member::Directive(d) if !matches!(def.kind, TypeKind::Agent | TypeKind::Supervisor) => {
                        diagnostics.push(Diagnostic::new(d.name.span, format!("unknown macro `{}`", d.name.name)));
                    }
                    other => def.members.push(other),
                }
            }
        }
        program.items.push(item);
    }
    diagnostics
}

struct Expander {
    macros: HashMap<String, MacroDef>,
}

impl Expander {
    /// `expr` as a top-level macro invocation.
    fn invocation(&self, expr: &Expr) -> Option<Invocation> {
        match &expr.kind {
            ExprKind::Call { recv: None, name, args, block: None, .. } if self.macros.contains_key(&name.name) => {
                Some(Invocation {
                    name: name.name.clone(),
                    args: args.clone(),
                    span: expr.span,
                    context: Context::TopLevel,
                })
            }
            _ => None,
        }
    }

    /// The code of `invocation` (whose arguments are in `text`), with the
    /// macros it invokes expanded too.
    fn expand(&self, invocation: &Invocation, text: &str, depth: usize) -> Result<String, Diagnostic> {
        let fail = |message: String| Diagnostic::new(invocation.span, message);
        if depth > MAX_DEPTH {
            return Err(fail(format!("macro `{}`: expansions nested too deep (is it recursive?)", invocation.name)));
        }
        let def = &self.macros[&invocation.name];
        let bindings = bind(def, &invocation.args, text).map_err(fail)?;
        let code = render(&def.body, &bindings).map_err(|e| fail(format!("in macro `{}`: {e}", def.name.name)))?;
        self.expand_within(&code, invocation.context, depth)
            .map_err(|e| fail(format!("in the expansion of `{}`: {}", invocation.name, e.message)))
    }

    /// `code` with its own invocations expanded (in the place of each).
    fn expand_within(&self, code: &str, context: Context, depth: usize) -> Result<String, Diagnostic> {
        let (wrapped, offset) = wrap(code, context);
        let parsed = grenat_parser::parse(&wrapped);
        // a code that does not parse is reported when the expansion is parsed
        if !parsed.diagnostics.is_empty() {
            return Ok(code.to_string());
        }
        let mut invocations = Vec::new();
        for item in &parsed.program.items {
            match item {
                Item::Stmt(expr) => invocations.extend(self.invocation(expr)),
                Item::Type(def) => {
                    for member in &def.members {
                        if let Member::Directive(d) = member
                            && self.macros.contains_key(&d.name.name)
                        {
                            invocations.push(Invocation {
                                name: d.name.name.clone(),
                                args: d.args.clone(),
                                span: d.span,
                                context: Context::Members(def.kind),
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        let mut out = wrapped.clone();
        invocations.sort_by_key(|i| std::cmp::Reverse(i.span.start));
        for invocation in &invocations {
            let expansion = self.expand(invocation, &wrapped, depth + 1)?;
            out.replace_range(invocation.span.start as usize..invocation.span.end as usize, &expansion);
        }
        Ok(unwrap(&out, offset, context))
    }
}

/// The values of the macro's parameters.
fn bind(def: &MacroDef, args: &[Arg], text: &str) -> Result<HashMap<String, Binding>, String> {
    let name = &def.name.name;
    let mut bindings = HashMap::new();
    let (fixed, variadic): (Vec<_>, Vec<_>) = def.params.iter().partition(|p| !p.variadic);
    let mut positional = args.iter().filter_map(|a| match a {
        Arg::Pos(e) => Some(e),
        _ => None,
    });
    for param in &fixed {
        if let Some(expr) = positional.next() {
            bindings.insert(param.name.name.clone(), Binding::One(argument(expr, text)));
        }
    }
    let rest: Vec<String> = positional.map(|e| argument(e, text)).collect();
    match variadic.first() {
        Some(param) => {
            bindings.insert(param.name.name.clone(), Binding::Many(rest));
        }
        None if !rest.is_empty() => {
            let got = args.iter().filter(|a| matches!(a, Arg::Pos(_))).count();
            return Err(format!("macro `{name}` takes {} argument(s), got {got}", fixed.len()));
        }
        None => {}
    }
    for arg in args {
        if let Arg::Named { name: param, value } = arg {
            if !fixed.iter().any(|p| p.name.name == param.name) {
                return Err(format!("macro `{name}` has no parameter `{}`", param.name));
            }
            let value = value.as_ref().map_or_else(|| param.name.clone(), |e| argument(e, text));
            bindings.insert(param.name.clone(), Binding::One(value));
        }
    }
    if let Some(missing) = fixed.iter().find(|p| !bindings.contains_key(&p.name.name)) {
        return Err(format!("macro `{name}`: missing argument `{}`", missing.name.name));
    }
    Ok(bindings)
}

/// An argument as the template sees it: a symbol gives its name, anything
/// else the text written.
fn argument(expr: &Expr, text: &str) -> String {
    match &expr.kind {
        ExprKind::Symbol(name) => name.clone(),
        _ => text.get(expr.span.start as usize..expr.span.end as usize).unwrap_or_default().to_string(),
    }
}

/// Members are parsed inside a type of the same kind.
fn wrap(code: &str, context: Context) -> (String, usize) {
    match context {
        Context::TopLevel => (code.to_string(), 0),
        Context::Members(kind) => {
            let header = format!("{} MacroExpansion\n", keyword(kind));
            (format!("{header}{code}\nend\n"), header.len())
        }
    }
}

fn unwrap(text: &str, offset: usize, context: Context) -> String {
    match context {
        Context::TopLevel => text.to_string(),
        Context::Members(_) => text[offset..text.len() - "\nend\n".len()].to_string(),
    }
}

fn keyword(kind: TypeKind) -> &'static str {
    match kind {
        TypeKind::Struct => "struct",
        TypeKind::Class => "class",
        TypeKind::Module => "module",
        TypeKind::Enum => "enum",
        TypeKind::Agent => "agent",
        TypeKind::Supervisor => "supervisor",
    }
}

/// The items of a top-level expansion.
fn parse_items(code: &str, invocation: &Invocation) -> Result<Vec<Item>, Diagnostic> {
    let parsed = grenat_parser::parse_expansion(code, invocation.span);
    if let Some(first) = parsed.diagnostics.into_iter().next() {
        return Err(Diagnostic {
            message: format!("in the expansion of `{}`: {}", invocation.name, first.message),
            ..first
        });
    }
    if parsed.program.items.iter().any(|i| matches!(i, Item::Macro(_))) {
        return Err(Diagnostic::new(
            invocation.span,
            format!("macro `{}` defines a macro: not supported", invocation.name),
        ));
    }
    Ok(parsed.program.items)
}

/// The members of an expansion in the body of a type.
fn parse_members(code: &str, invocation: &Invocation, kind: TypeKind) -> Result<Vec<Member>, Diagnostic> {
    let (wrapped, _) = wrap(code, Context::Members(kind));
    let mut items = parse_items(&wrapped, invocation)?;
    match items.pop() {
        Some(Item::Type(def)) if items.is_empty() => Ok(def.members),
        _ => Err(Diagnostic::new(invocation.span, format!("macro `{}` must expand to members here", invocation.name))),
    }
}
