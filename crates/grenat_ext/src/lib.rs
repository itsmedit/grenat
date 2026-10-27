//! The SDK of **native facets**: Rust code a Grenat facet ships, called by
//! Grenat programs as ordinary functions — as Ruby calls a gem's C code.
//!
//! ```
//! use grenat_ext::{GrenatType, export};
//! use serde::{Deserialize, Serialize};
//!
//! /// A cell of a sheet.
//! #[derive(Serialize, Deserialize, GrenatType)]
//! pub struct Cell {
//!     pub row: i64,
//!     pub text: String,
//! }
//!
//! /// Reads a sheet: one row per line, cells separated by commas.
//! #[export(effects = "fs.read")]
//! pub fn read_sheet(path: String) -> Result<Vec<Vec<String>>, std::io::Error> {
//!     let text = std::fs::read_to_string(path)?;
//!     Ok(text.lines().map(|line| line.split(',').map(str::to_string).collect()).collect())
//! }
//!
//! /// The cell at `row` holding `text`.
//! #[export(pure)]
//! pub fn cell(row: i64, text: String) -> Cell {
//!     Cell { row, text }
//! }
//!
//! fn main() {
//!     // what `setter install` reads from the built library
//!     let manifest = grenat_ext::manifest();
//!     assert_eq!(manifest.abi, grenat_ext::abi::ABI_VERSION);
//!     let read = manifest.function("read_sheet").unwrap();
//!     assert_eq!(read.symbol, "grenat_ext_v1_read_sheet");
//!     assert_eq!(read.returns, "Array(Array(String))");
//!     assert_eq!(read.effects, ["fs.read"]);
//!     let cell = serde_json::to_value(manifest.function("cell").unwrap()).unwrap();
//!     assert_eq!(cell["pure"], true);
//!     assert_eq!(cell["params"][0], serde_json::json!({"name": "row", "type": "Int", "doc": null}));
//!     assert_eq!(manifest.structs[0].name, "Cell");
//! }
//! ```
//!
//! # A facet's crate
//!
//! The facet's `grenat.toml` names its crate (`[native] path = "native"`),
//! a `cdylib` depending on this crate by version:
//!
//! ```toml
//! [lib]
//! crate-type = ["cdylib"]
//!
//! [dependencies]
//! grenat_ext = "0.1"
//! serde = { version = "1", features = ["derive"] }
//! ```
//!
//! `setter install` builds it (`cargo build --release`), unless the facet
//! ships the library prebuilt for the platform
//! (`native/prebuilt/<arch>-<os>/lib<facet>.<so|dylib>`), loads it once to
//! read its manifest, and writes the Grenat declarations it stands for
//! (`native def cell(row: Int, text: String) -> Cell pure`). The
//! application trusts the facet explicitly (`facet "sheets", "~> 0.1",
//! native: true`): native code runs outside Grenat's sandbox.
//!
//! # Types
//!
//! Types that cross: `String`, integers (`Int`), `f64` (`Float`), `bool`,
//! `Vec<T>`, `Option<T>`, `HashMap<String, T>` and structs deriving serde's
//! `Serialize`/`Deserialize` and [`GrenatType`]; an exported function may
//! return `Result<T, E: Display>`, whose `Err` raises a Grenat error
//! (`NativeError`, or the type `error = "…"` names). A panic is caught and
//! raised as a Grenat error too: it never unwinds into Grenat.
//!
//! # The ABI (version 1)
//!
//! Each exported function gets a C entry point of a versioned ABI
//! ([`abi`], [`abi::ABI_VERSION`]) that never shows a Rust type to the
//! other side: arguments and results travel as JSON. A library exports
//! `grenat_ext_abi_version() -> u32` (checked first: a library built for
//! another version is refused), `grenat_ext_manifest`,
//! `grenat_ext_free`, and `grenat_ext_v1_<name>(args, len, out) -> status`
//! per function: `args` a JSON array of the arguments, `out` the result's
//! JSON or an error `{"type": "…", "message": "…"}`, the status
//! [`abi::STATUS_OK`], [`abi::STATUS_ERROR`] or [`abi::STATUS_PANIC`].
//! The ABI version changes only when this contract does; versions of this
//! crate that speak the same ABI are interchangeable for Grenat.
//!
//! # The manifest
//!
//! What a library says of itself ([`manifest::Manifest`]), as JSON, which
//! `setter install` keeps next to the library (`.grenat/native/manifest.json`
//! in the installed facet):
//!
//! ```json
//! {
//!   "abi": 1,
//!   "functions": [
//!     {
//!       "name": "cell",
//!       "symbol": "grenat_ext_v1_cell",
//!       "doc": "The cell at `row` holding `text`.",
//!       "params": [
//!         { "name": "row", "type": "Int", "doc": null },
//!         { "name": "text", "type": "String", "doc": null }
//!       ],
//!       "returns": "Cell",
//!       "effects": [],
//!       "pure": true,
//!       "error": "NativeError"
//!     }
//!   ],
//!   "structs": [
//!     {
//!       "name": "Cell",
//!       "doc": "A cell of a sheet.",
//!       "fields": [
//!         { "name": "row", "type": "Int", "doc": null },
//!         { "name": "text", "type": "String", "doc": null }
//!       ]
//!     }
//!   ]
//! }
//! ```
//!
//! Types are written as Grenat writes them (`Array(Array(String))`, `Int?`,
//! `Hash(String, Float)`, `Nil`), effects too (`fs.read`,
//! `net("api.x.com")`); `pure` means the result depends on the arguments
//! only (trusted, not tainted); `error` is the Grenat error an `Err` raises.

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
