//! What is under the cursor: its definition, its documentation; the
//! symbols of a document.

use grenat_ast::{FnDef, Item, Member, Program, Span, TypeKind};
use grenat_lexer::{StrPart, Token, TokenKind};
use grenat_report::Sources;

/// A name defined by the program.
pub(crate) struct Definition<'p> {
    pub name: &'p str,
    /// Where the name is (in the program's sources).
    pub name_span: Span,
    /// The whole definition.
    pub span: Span,
    pub doc: Option<&'p str>,
    /// LSP `SymbolKind`.
    pub kind: u32,
    /// Top-level (listed as a document symbol).
    pub top_level: bool,
}

const FUNCTION: u32 = 12;
const METHOD: u32 = 6;
const CLASS: u32 = 5;
const MODULE: u32 = 2;
const STRUCT: u32 = 23;
const ENUM: u32 = 10;
const ENUM_MEMBER: u32 = 22;
const FIELD: u32 = 8;
const CONSTANT: u32 = 14;

/// Every definition of the program: functions, types, their members, models.
pub(crate) fn definitions(program: &Program) -> Vec<Definition<'_>> {
    let mut out = Vec::new();
    for item in &program.items {
        match item {
            Item::Fn(def) => out.push(function(def, FUNCTION, true)),
            Item::Type(ty) => {
                let kind = match ty.kind {
                    TypeKind::Struct => STRUCT,
                    TypeKind::Enum => ENUM,
                    TypeKind::Module => MODULE,
                    TypeKind::Class | TypeKind::Agent | TypeKind::Supervisor => CLASS,
                };
                out.push(Definition {
                    name: &ty.name.name,
                    name_span: ty.name.span,
                    span: ty.span,
                    doc: ty.doc.as_deref(),
                    kind,
                    top_level: true,
                });
                for member in &ty.members {
                    match member {
                        Member::Method(def) => out.push(function(def, METHOD, false)),
                        Member::Variant(v) => out.push(Definition {
                            name: &v.name.name,
                            name_span: v.name.span,
                            span: v.span,
                            doc: v.doc.as_deref(),
                            kind: ENUM_MEMBER,
                            top_level: false,
                        }),
                        Member::Field(f) => out.push(Definition {
                            name: &f.name.name,
                            name_span: f.name.span,
                            span: f.span,
                            doc: f.doc.as_deref(),
                            kind: FIELD,
                            top_level: false,
                        }),
                        _ => {}
                    }
                }
            }
            Item::Model(model) => out.push(Definition {
                name: &model.name.name,
                name_span: model.name.span,
                span: model.span,
                doc: None,
                kind: CONSTANT,
                top_level: true,
            }),
            Item::Stmt(_) => {}
        }
    }
    out
}

fn function(def: &FnDef, kind: u32, top_level: bool) -> Definition<'_> {
    Definition { name: &def.name.name, name_span: def.name.span, span: def.span, doc: def.doc.as_deref(), kind, top_level }
}

/// The name under `offset` in `text` (a single file): an identifier, a
/// constant, a symbol (`:fast`, a model) or an instance variable.
pub(crate) fn name_at(text: &str, offset: usize) -> Option<String> {
    let lexed = grenat_lexer::lex(text);
    find(&lexed.tokens, offset as u32)
}

fn find(tokens: &[Token], offset: u32) -> Option<String> {
    for token in tokens {
        if offset < token.span.start || offset > token.span.end {
            continue;
        }
        match &token.kind {
            TokenKind::Ident(name) | TokenKind::Const(name) | TokenKind::Symbol(name) => return Some(name.clone()),
            TokenKind::IVar(name) => return Some(name.trim_start_matches('@').to_string()),
            TokenKind::Str(parts) => {
                for part in parts {
                    if let StrPart::Interp(inner, span) = part
                        && span.start <= offset
                        && offset <= span.end
                    {
                        return find(inner, offset);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// The hover text of a definition: its first line, then its documentation.
pub(crate) fn hover(def: &Definition, sources: &Sources) -> String {
    let start = def.span.start as usize;
    let text = &sources.text[start.min(sources.text.len())..];
    let signature = text.lines().next().unwrap_or("").trim_end();
    let mut markdown = format!("```ruby\n{signature}\n```");
    if let Some(doc) = def.doc {
        markdown.push_str(&format!("\n\n{doc}"));
    }
    markdown
}

/// The definition of `name`: top-level first, then members.
pub(crate) fn lookup<'d, 'p>(defs: &'d [Definition<'p>], name: &str) -> Option<&'d Definition<'p>> {
    defs.iter().find(|d| d.name == name && d.top_level).or_else(|| defs.iter().find(|d| d.name == name))
}
