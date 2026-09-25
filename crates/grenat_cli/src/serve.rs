//! `grenat serve`: runs a program's triggers (`every`, `on_webhook`) until
//! the process is stopped.

use std::process::ExitCode;

use grenat_driver::{load, options_for, render_runtime_error};

/// Where webhooks are received, by default.
const DEFAULT_ADDRESS: &str = "127.0.0.1:3000";

pub fn serve(args: &[String]) -> ExitCode {
    let mut address = DEFAULT_ADDRESS.to_string();
    let mut rest = args;
    while let Some(flag) = rest.first().filter(|a| a.starts_with("--")) {
        match (flag.as_str(), rest.get(1)) {
            ("--listen", Some(value)) => {
                address = value.clone();
                rest = &rest[2..];
            }
            _ => {
                eprintln!("usage: grenat serve [--listen host:port] [<file.grn>]");
                return ExitCode::from(2);
            }
        }
    }
    let path = match rest.first() {
        Some(path) => path.clone(),
        None => match crate::package::main_file() {
            Ok(main) => main,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::from(2);
            }
        },
    };
    let Some(loaded) = load(&path, false) else { return ExitCode::FAILURE };
    let listening = |at: &str| eprintln!("listening on http://{at}");
    match grenat_interp::serve(&loaded.program, options_for(&path), &address, listening) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            render_runtime_error(&loaded.sources, &error);
            ExitCode::FAILURE
        }
    }
}
