//! Lexer de Grenat.
//!
//! Écrit à la main plutôt qu'avec un générateur : l'interpolation `"#{…}"`
//! et les heredocs `<<~ID` demandent des modes qu'un lexer régulier gère mal.
//!
//! Conventions à la Ruby gérées ici :
//! - `nom:` collé est un **label** (argument nommé, champ), `:nom` est un **symbole** ;
//! - `?` et `!` en fin d'identifiant minuscule font partie du nom (`empty?`, `save!`) ;
//! - une fin de ligne suivie de `.` ou `&.` continue l'expression (chaînage multi-ligne) ;
//! - les commentaires sont retirés du flux de tokens mais conservés à part
//!   (les `##` sont des commentaires de documentation, transmis aux LLM).

mod lexer;
mod strings;
mod token;

pub use lexer::lex;
pub(crate) use lexer::*;
pub(crate) use strings::*;
pub use token::*;

#[cfg(test)]
mod tests;
