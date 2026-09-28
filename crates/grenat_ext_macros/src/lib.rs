//! The macros of `grenat_ext`, the SDK of native facets.
//!
//! - `#[export]` turns a plain Rust function into an entry point a Grenat
//!   program calls: a C function of the versioned ABI (JSON in, JSON out,
//!   panics caught), described in the library's manifest.
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
