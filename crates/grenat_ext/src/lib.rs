//! The SDK of **native facets**: Rust code a Grenat facet ships, called by
//! Grenat programs as ordinary functions — as Ruby calls a gem's C code.
//!
//! ```ignore
//! use grenat_ext::{GrenatType, export};
//! use serde::{Deserialize, Serialize};
//!
//! /// A cell of a sheet.
//! #[derive(Serialize, Deserialize, GrenatType)]
//! pub struct Cell { pub row: i64, pub text: String }
//!
//! /// Reads a sheet: one row per line.
//! #[export(effects = "fs.read")]
//! pub fn read_sheet(path: String) -> Result<Vec<Vec<String>>, std::io::Error> { … }
//!
//! #[export(pure)]
//! pub fn add(a: i64, b: i64) -> i64 { a + b }
//! ```
//!
//! The facet's crate is a `cdylib`. Each exported function gets a C entry
//! point of a versioned ABI ([`abi`]) that never shows a Rust type to the
//! other side: arguments and results travel as JSON, panics are caught and
//! become Grenat errors, and the library describes itself in its
//! [`manifest`] — from which `setter install` writes the Grenat declarations
//! (`native def add(a: Int, b: Int) -> Int pure`).
//!
//! Types that cross: `String`, integers (`Int`), `f64` (`Float`), `bool`,
//! `Vec<T>`, `Option<T>`, `HashMap<String, T>` and structs deriving serde's
//! `Serialize`/`Deserialize` and [`GrenatType`]; an exported function may
//! return `Result<T, E: Display>`, whose `Err` raises a Grenat error
//! (`NativeError`, or the type `error = "…"` names).

pub mod abi;
pub mod manifest;
mod panics;
mod registry;
mod runtime;
mod types;

pub use grenat_ext_macros::{GrenatType, export};
pub use registry::{grenat_ext_abi_version, grenat_ext_free, grenat_ext_manifest, manifest};
pub use types::GrenatType;

/// What the generated code uses; not an API.
#[doc(hidden)]
pub mod __private {
    pub use crate::registry::Export;
    pub use crate::runtime::{Args, Failure, entry, raised, returned};
    pub use inventory;
}
