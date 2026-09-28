//! **Native facets**, Grenat's side: a facet ships Rust code (a crate
//! depending on `grenat_ext`), which Grenat builds, describes and calls.
//!
//! - [`build`]: `cargo build --release` of the facet's crate, a `cdylib`;
//! - [`install`]: the library copied into the installed facet
//!   ([`layout`]), its manifest read by loading it once, and the Grenat
//!   declarations it stands for written next to it ([`declarations`]):
//!   `native def read_sheet(path: String) -> ~Array(Array(String)) uses fs.read`;
//! - [`library`]: loading a library (once per process, never unloaded), its
//!   ABI version checked, and calls — JSON arguments in, JSON result out;
//! - [`registry`]: the native functions of a program, by name, their
//!   libraries loaded on first call (what the interpreter holds).
//!
//! Native code escapes Grenat's sandbox: which facets may ship it is the
//! application's decision (`facet "sheets", native: true`), enforced by
//! `grenat_package` before anything here runs.

pub mod build;
pub mod declarations;
#[cfg(feature = "fixture")]
pub mod fixture;
pub mod install;
pub mod layout;
pub mod library;
pub mod registry;

pub use grenat_ext::manifest::{Field, Function, Manifest, Struct};
pub use install::{Installed, install};
pub use library::{Library, Outcome};
pub use registry::Registry;
