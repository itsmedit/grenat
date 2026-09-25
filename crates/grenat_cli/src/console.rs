//! `grenat console`: the operations console of an application, in a browser.

use std::path::Path;
use std::process::ExitCode;

use grenat_driver::{load, options_for, render_runtime_error};

/// Where the console listens, by default: this machine only.
const DEFAULT_ADDRESS: &str = "127.0.0.1:4000";
/// Where the token comes from, rather than the command line (which other
/// users of the machine can see).
const TOKEN_VARIABLE: &str = "GRENAT_CONSOLE_TOKEN";

pub fn console(args: &[String]) -> ExitCode {
    let mut address = DEFAULT_ADDRESS.to_string();
    let mut token = std::env::var(TOKEN_VARIABLE).ok().filter(|t| !t.is_empty());
    let mut rest = args;
    while let Some(flag) = rest.first().filter(|a| a.starts_with("--")) {
        match (flag.as_str(), rest.get(1)) {
            ("--listen", Some(value)) => address = value.clone(),
            ("--token", Some(value)) => token = Some(value.clone()),
            _ => {
                eprintln!("usage: grenat console [--listen host:port] [--token <token>] [<file.grn>]  ({TOKEN_VARIABLE}: the token)");
                return ExitCode::from(2);
            }
        }
        rest = &rest[2..];
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
    let name = match &loaded.package {
        Some(package) => package.manifest.name.clone(),
        None => Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
    };
    let listening = |at: &str| eprintln!("console on http://{at}");
    match grenat_interp::console(&loaded.program, options_for(&path), &name, &address, token, listening) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            render_runtime_error(&loaded.sources, &error);
            ExitCode::FAILURE
        }
    }
}
