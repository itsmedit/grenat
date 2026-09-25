//! `grenat fmt`: rewrites files in the canonical layout (see `grenat_fmt`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use grenat_driver::{read, report};
use grenat_fmt::FmtError;

pub fn fmt(args: &[String]) -> ExitCode {
    let check = args.iter().any(|a| a == "--check");
    let paths: Vec<&String> = args.iter().filter(|a| *a != "--check").collect();
    if paths.is_empty() {
        eprintln!("usage: grenat fmt [--check] <file.grn | directory>...");
        return ExitCode::from(2);
    }
    let files: Vec<PathBuf> = paths.iter().flat_map(|p| grenat_files(Path::new(p))).collect();
    let (mut changed, mut failed) = (0, 0);
    for file in &files {
        let path = file.display().to_string();
        let Some(src) = read(&path) else {
            failed += 1;
            continue;
        };
        match grenat_fmt::format(&src) {
            Ok(out) if out == src => {}
            Ok(_) if check => {
                changed += 1;
                eprintln!("{path} is not formatted");
            }
            Ok(out) => match std::fs::write(file, out) {
                Ok(()) => {
                    changed += 1;
                    eprintln!("formatted {path}");
                }
                Err(e) => {
                    failed += 1;
                    eprintln!("error: cannot write {path}: {e}");
                }
            },
            Err(FmtError::Syntax) => {
                failed += 1;
                report(&path, &src, &grenat_parser::parse(&src).diagnostics);
            }
            Err(FmtError::Unsafe(why)) => {
                failed += 1;
                eprintln!("error: {path} left as it is: formatting would have changed it ({why}); please report it");
            }
        }
    }
    let total = files.len();
    if failed > 0 || (check && changed > 0) {
        let verb = if check { "to format" } else { "formatted" };
        eprintln!("✗ {failed} failed, {changed} {verb}, of {total} file(s)");
        return ExitCode::FAILURE;
    }
    eprintln!("✓ {total} file(s) formatted ({changed} changed)");
    ExitCode::SUCCESS
}

/// `path`, or the `.grn` files under it (hidden directories and `target` skipped).
fn grenat_files(path: &Path) -> Vec<PathBuf> {
    if !path.is_dir() {
        return vec![path.to_path_buf()];
    }
    let Ok(entries) = std::fs::read_dir(path) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.') || n == "target"))
        .flat_map(|p| if p.is_dir() { grenat_files(&p) } else if p.extension().is_some_and(|x| x == "grn") { vec![p] } else { Vec::new() })
        .collect();
    files.sort();
    files
}
