//! `grenat migrate`: applies the migrations the database has not seen yet.

use std::process::ExitCode;

use grenat_driver::{load, options_for, render_runtime_error};

pub fn migrate(args: &[String]) -> ExitCode {
    let path = match args {
        [path] => path.clone(),
        [] => match crate::package::main_file() {
            Ok(main) => main,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::from(2);
            }
        },
        _ => {
            eprintln!("usage: grenat migrate [<file.grn>]");
            return ExitCode::from(2);
        }
    };
    let Some(loaded) = load(&path, false) else { return ExitCode::FAILURE };
    match grenat_interp::migrate(&loaded.program, options_for(&path)) {
        Ok(applied) if applied.is_empty() => {
            eprintln!("✓ the database is up to date");
            ExitCode::SUCCESS
        }
        Ok(applied) => {
            for name in &applied {
                eprintln!("  applied {name}");
            }
            eprintln!("✓ {} migration(s) applied", applied.len());
            ExitCode::SUCCESS
        }
        Err(error) => {
            render_runtime_error(&loaded.sources, &error);
            ExitCode::FAILURE
        }
    }
}
