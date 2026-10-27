//! The macros of [`grenat_ext`](https://docs.rs/grenat_ext), the SDK of
//! native facets. Use them through `grenat_ext`, which re-exports them and
//! holds what the code they generate calls:
//!
//! ```
//! use grenat_ext::{GrenatType, export};
//! use serde::{Deserialize, Serialize};
//!
//! /// A point of the plane.
//! #[derive(Serialize, Deserialize, GrenatType)]
//! pub struct Point {
//!     pub x: f64,
//!     pub y: f64,
//! }
//!
//! /// The middle of two points.
//! #[export(pure)]
//! pub fn middle(a: Point, b: Point) -> Point {
//!     Point { x: (a.x + b.x) / 2.0, y: (a.y + b.y) / 2.0 }
//! }
//!
//! fn main() {
//!     let manifest = grenat_ext::manifest();
//!     assert_eq!(manifest.function("middle").unwrap().returns, "Point");
//! }
//! ```
//!
//! - `#[export]` turns a plain Rust function into an entry point a Grenat
//!   program calls: a C function of the versioned ABI (JSON in, JSON out,
//!   panics caught), described in the library's manifest. It takes
//!   `effects = "fs.read, net(\"api.x.com\")"`, `pure` and
//!   `error = "SheetError"`.
//! - `#[derive(GrenatType)]` gives a struct its Grenat declaration, so that
//!   exported functions can take and return it.
//!
//! The work is done on `proc_macro2` tokens (the `export` and `derive`
//! modules), tested without a compiler in between.

mod attrs;
mod derive;
mod docs;
mod export;
mod grenat;
mod signature;

use proc_macro::TokenStream;

/// Exports a function to Grenat: `#[grenat_ext::export]`,
/// `#[grenat_ext::export(effects = "fs.read")]`, `#[grenat_ext::export(pure)]`,
/// `#[grenat_ext::export(error = "SheetError")]`.
#[proc_macro_attribute]
pub fn export(attr: TokenStream, item: TokenStream) -> TokenStream {
    export::expand(attr.into(), item.into()).into()
}

/// Declares a struct's fields to Grenat (it must also derive serde's
/// `Serialize` and `Deserialize`).
#[proc_macro_derive(GrenatType)]
pub fn derive_grenat_type(item: TokenStream) -> TokenStream {
    derive::expand(item.into()).into()
}
