//! Packages on the command line: `grenat new`, `grenat update`, and the
//! commands that default to the current package when given no file.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use grenat_package::Package;

/// The package the current directory is in.
pub fn current() -> Result<Package, String> {
    match grenat_package::find_package(Path::new(".")) {
        Ok(Some(package)) => Ok(package),
        Ok(None) => Err(format!("no file given, and no {} here or above", grenat_package::MANIFEST)),
        Err(e) => Err(e),
    }
}

/// A path as the user would type it: relative to the current directory when inside it.
pub fn shown(path: &Path) -> String {
    let cwd = std::env::current_dir().ok().and_then(|d| d.canonicalize().ok());
    match cwd.and_then(|cwd| path.strip_prefix(cwd).ok().map(Path::to_path_buf)) {
        Some(relative) => relative.to_string_lossy().into_owned(),
        None => path.to_string_lossy().into_owned(),
    }
}

/// The current package's program (`package.main`).
pub fn main_file() -> Result<String, String> {
    let package = current()?;
    Ok(shown(&package.main()))
}

/// Every `.grn` file of the current package's `src/`, `tests/` and
/// `db/migrations/`.
pub fn files() -> Result<Vec<String>, String> {
    let package = current()?;
    let mut files = Vec::new();
    for dir in ["src", "tests", "db/migrations"] {
        collect(&package.root.join(dir), &mut files);
    }
    files.sort();
    Ok(files.iter().map(|f| shown(f)).collect())
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().is_some_and(|e| e == "grn") {
            files.push(path);
        }
    }
}

/// `paths`, or the current package's files when there are none.
pub fn or_package_files(paths: &[String]) -> Result<Vec<String>, ExitCode> {
    if !paths.is_empty() {
        return Ok(paths.to_vec());
    }
    files().map_err(|e| {
        eprintln!("error: {e}");
        ExitCode::from(2)
    })
}

/// `grenat new <name>`: a package in the new directory `<name>`.
pub fn new(args: &[String]) -> ExitCode {
    let [name] = args else {
        eprintln!("usage: grenat new <name>");
        return ExitCode::from(2);
    };
    let dir = Path::new(name);
    let package = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match grenat_package::create(dir, &package) {
        Ok(()) => {
            eprintln!("✓ created the package `{package}` in {}", dir.display());
            eprintln!("  cd {} && grenat run && grenat test", dir.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `grenat update`: fetches the latest commits of the git dependencies and
/// rewrites `grenat.lock`.
pub fn update() -> ExitCode {
    let main = match main_file() {
        Ok(main) => main,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    match grenat_driver::load_with(&main, false, true) {
        Some(_) => {
            eprintln!("✓ dependencies up to date");
            ExitCode::SUCCESS
        }
        None => ExitCode::FAILURE,
    }
}
