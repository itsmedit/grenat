//! `grenat build`: compiles a program ahead of time into an executable.
//!
//! The eligible functions become machine code in an object file (see
//! `grenat_codegen::aot`), linked with the host library `libgrenat_host.a`,
//! which runs the rest of the program. The executable needs neither
//! `grenat` nor the source file.
//!
//! With `--native`, the whole program is compiled (`grenat_codegen::standalone`)
//! and linked with `libgrenat_standalone.a` only: no interpreter inside.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::{env, fs};

use grenat_driver::load;

use crate::link::link;

pub fn build(args: &[String]) -> ExitCode {
    let native = args.iter().any(|a| a == "--native");
    let args: Vec<&String> = args.iter().filter(|a| *a != "--native").collect();
    let (path, output) = match args.as_slice() {
        [path] => (*path, default_output(path)),
        [path, flag, out] | [flag, out, path] if *flag == "-o" => (*path, PathBuf::from(out)),
        _ => {
            eprintln!("usage: grenat build [--native] <file.grn> [-o <executable>]");
            return ExitCode::from(2);
        }
    };
    match compile(path, &output, native) {
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

fn compile(path: &str, output: &Path, standalone: bool) -> Result<String, String> {
    // diagnostics are printed by `load`
    let (src, program) = load(path, false).ok_or_else(String::new)?;
    let (object, library) = if standalone {
        let object = grenat_codegen::standalone::object(&program, &src, path).map_err(|reasons| {
            let list: Vec<String> = reasons.iter().map(|r| format!("  - {r}")).collect();
            format!("{path} cannot be compiled without the interpreter:\n{}", list.join("\n"))
        })?;
        (object, "libgrenat_standalone.a")
    } else {
        (grenat_codegen::aot::object(&program, &src)?, HOST)
    };
    let native = object.report.compiled.len();
    let host = library_path(library)?;

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
    let kind = if standalone { "a native program of" } else { "with" };
    Ok(format!("{kind} {native} native function(s), {:.1} MB", size as f64 / 1e6))
}

const HOST: &str = "libgrenat_host.a";

/// A library of the toolchain, looked up relative to the `grenat` binary:
/// in `$GRENAT_HOME/lib/grenat/`, in `<prefix>/lib/grenat/` for an installed
/// `<prefix>/bin/grenat` (symbolic links followed), or next to it (`cargo build`).
fn library_path(name: &str) -> Result<PathBuf, String> {
    if let Some(home) = env::var_os("GRENAT_HOME") {
        return Ok(Path::new(&home).join("lib/grenat").join(name));
    }
    let exe = env::current_exe().map_err(|e| e.to_string())?;
    let real = fs::canonicalize(&exe).unwrap_or_else(|_| exe.clone());
    let installed = |exe: &Path| exe.parent().and_then(Path::parent).map(|prefix| prefix.join("lib/grenat").join(name));
    let candidates = [installed(&exe), installed(&real), Some(exe.with_file_name(name)), Some(real.with_file_name(name))];
    candidates
        .into_iter()
        .flatten()
        .find(|p| p.exists())
        .ok_or_else(|| format!("cannot find {name} for {} (set $GRENAT_HOME)", exe.display()))
}
