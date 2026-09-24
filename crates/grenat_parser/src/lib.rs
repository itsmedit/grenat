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

mod blocks;
mod calls;
mod control;
mod cursor;
mod docs;
mod exprs;
mod items;
mod patterns;
mod stmts;
mod types;

use grenat_ast::Program;
use grenat_lexer::lex;

pub(crate) use calls::*;
pub(crate) use cursor::*;
pub(crate) use docs::*;
pub(crate) use exprs::*;

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
