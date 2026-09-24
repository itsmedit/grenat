//! `grenat build`: compiles a program ahead of time into an executable.
//!
//! The eligible functions become machine code in an object file (see
//! `grenat_codegen::aot`), linked with the host library `libgrenat_host.a`,
//! which runs the rest of the program. The executable needs neither
//! `grenat` nor the source file.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::{env, fs};

use grenat_driver::load;

use crate::link::link;

pub fn build(args: &[String]) -> ExitCode {
    let (path, output) = match args {
        [path] => (path, default_output(path)),
        [path, flag, out] | [flag, out, path] if flag == "-o" => (path, PathBuf::from(out)),
        _ => {
            eprintln!("usage: grenat build <file.grn> [-o <executable>]");
            return ExitCode::from(2);
        }
    };
    match compile(path, &output) {
        Ok(summary) => {
            eprintln!("✓ built {} ({summary})", output.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            if !error.is_empty() {
                eprintln!("error: {error}");
            }
            ExitCode::FAILURE
        }
    }
}

/// `app.grn` → `./app`
fn default_output(path: &str) -> PathBuf {
    PathBuf::from(Path::new(path).file_stem().unwrap_or_default())
}

fn compile(path: &str, output: &Path) -> Result<String, String> {
    // diagnostics are printed by `load`
    let (src, program) = load(path, false).ok_or_else(String::new)?;
    let object = grenat_codegen::aot::object(&program, &src)?;
    let native = object.report.compiled.len();
    let host = host_library()?;

    let dir = env::temp_dir().join(format!("grenat-build-{}", std::process::id()));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let object_path = dir.join("program.o");
    fs::write(&object_path, object.bytes).map_err(|e| e.to_string())?;
    let linked = link(&object_path, &host, output);
    if env::var_os("GRENAT_KEEP_OBJECT").is_none() {
        let _ = fs::remove_dir_all(&dir);
    }
    linked?;

    let size = fs::metadata(output).map_or(0, |m| m.len());
    Ok(format!("{native} native function(s), {:.1} MB", size as f64 / 1e6))
}

const HOST: &str = "libgrenat_host.a";

/// `libgrenat_host.a`, looked up relative to the `grenat` binary: in
/// `$GRENAT_HOME/lib/grenat/`, in `<prefix>/lib/grenat/` for an installed
/// `<prefix>/bin/grenat` (symbolic links followed), or next to it (`cargo build`).
fn host_library() -> Result<PathBuf, String> {
    if let Some(home) = env::var_os("GRENAT_HOME") {
        return Ok(Path::new(&home).join("lib/grenat").join(HOST));
    }
    let exe = env::current_exe().map_err(|e| e.to_string())?;
    let real = fs::canonicalize(&exe).unwrap_or_else(|_| exe.clone());
    let installed = |exe: &Path| exe.parent().and_then(Path::parent).map(|prefix| prefix.join("lib/grenat").join(HOST));
    let candidates = [installed(&exe), installed(&real), Some(exe.with_file_name(HOST)), Some(real.with_file_name(HOST))];
    candidates
        .into_iter()
        .flatten()
        .find(|p| p.exists())
        .ok_or_else(|| format!("cannot find {HOST} for {} (set $GRENAT_HOME)", exe.display()))
}
