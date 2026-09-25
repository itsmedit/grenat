//! Loading and running Grenat programs: what `grenat run` does, shared with
//! the executables built by `grenat build`, which must behave identically.
//!
//! A program is a file and the files it `require`s (see `grenat_package`):
//! its [`Sources`], spans pointing into them.

pub use grenat_report as report;
pub use grenat_report::Sources;

mod flags;
mod loading;
mod running;

pub use flags::{log_from_env, native_from_env, record_from_env, use_color};
pub use loading::{Loaded, load, load_with, parse, read, report};
pub use running::{execute, options_for, render_runtime_error};
