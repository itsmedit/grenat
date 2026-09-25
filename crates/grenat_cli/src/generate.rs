//! `grenat new --app` and `grenat generate`: an application, and its parts.

use std::path::Path;
use std::process::ExitCode;

use grenat_generate::{Change, Kind};

/// `grenat new --app <name>`.
pub fn new_app(dir: &str) -> ExitCode {
    let dir = Path::new(dir);
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match grenat_generate::create_app(dir, &name) {
        Ok(changes) => {
            report(&changes);
            eprintln!("✓ created the application `{name}` in {}", dir.display());
            eprintln!("  cd {} && grenat test && grenat generate agent <name>", dir.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `grenat generate <kind> <name> [field:Type…]`, in the current application.
pub fn generate(args: &[String]) -> ExitCode {
    let usage = || {
        eprintln!("usage: grenat generate {} <name> [field:Type…]", Kind::ALL);
        ExitCode::from(2)
    };
    let [kind, name, fields @ ..] = args else { return usage() };
    let Some(kind) = Kind::parse(kind) else { return usage() };
    let package = match crate::package::current() {
        Ok(package) => package,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    match grenat_generate::generate(&package.root, kind, name, fields, now) {
        Ok(changes) => {
            report(&changes);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn report(changes: &[Change]) {
    for change in changes {
        match change {
            Change::Created(path) => eprintln!("  create  {}", path.display()),
            Change::Updated(path) => eprintln!("  update  {}", path.display()),
        }
    }
}
