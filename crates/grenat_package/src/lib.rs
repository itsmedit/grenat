//! Grenat packages and programs made of several files.
//!
//! A program is a file and every file it `require`s, transitively:
//! `require "./helpers"` is a file next to the requiring one,
//! `require "http"` (or `"http/client"`) a file of the dependency `http`
//! declared in the package's `grenat.toml`. Requires are resolved before
//! anything else, and every file is loaded once, after the files it requires,
//! into one [`Sources`](grenat_report::Sources): all the files share one
//! namespace, as in Ruby.
//!
//! Dependencies are local directories (`path = "../utils"`) or git
//! repositories (`git = "…"`, with a `tag`, `branch` or `rev`), fetched into
//! the root package's `.grenat/deps/` and pinned in its `grenat.lock`.

mod bundle;
pub mod facetfile;
pub mod facets;
mod git;
mod lock;
mod manifest;
mod requires;
mod resolve;
mod scaffold;
pub mod version;

pub use bundle::{Bundle, LoadError, load, load_with};
pub use lock::Lock;
pub use manifest::{Dependency, Manifest, Reference, Source};
pub use requires::{is_require, strip_requires};
pub use resolve::{Package, find_package};
pub use scaffold::{create, create_facet};

/// The manifest's file name.
pub const MANIFEST: &str = "grenat.toml";
/// The lock file's name, next to the root manifest.
pub const LOCK: &str = "grenat.lock";
