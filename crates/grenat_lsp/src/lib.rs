//! The Grenat language server: `grenat lsp`, spoken over standard input and
//! output (JSON-RPC, the Language Server Protocol).
//!
//! Each change of a document checks its whole program — the document, the
//! files it requires, open buffers taking precedence over the disk — and
//! publishes its diagnostics. It also formats documents (`grenat fmt`),
//! shows the signature and documentation of what is under the cursor, goes
//! to definitions (across files) and lists a document's symbols.

mod analysis;
mod navigation;
mod position;
mod server;
mod transport;
mod uri;

pub use server::{Server, serve};
pub use transport::{read_message, write_message};
