//! `setter`: the facets (libraries) of a Grenat package — who sets the
//! stones in the jewel.

mod commands;

use std::process::ExitCode;

const USAGE: &str = "\
setter — the facets (libraries) of a Grenat package

Usage:
  setter init                    add a Facetfile to the current package
  setter new <name>              create a facet (a library to share)
  setter add <name> [\"~> 1.2\"]   use a facet from the indexes (the latest, by default)
  setter add <name> --path <dir> | --git <url> [--tag <tag>]
  setter install                 install the facets of the Facetfile (Facetfile.lock)
  setter update                  install their latest allowed versions
  setter list                    the facets installed
  setter publish                 tag this facet's version (v<version>) for the indexes
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("init") if args.len() == 1 => commands::init(),
        Some("new") if args.len() == 2 => commands::new(&args[1]),
        Some("add") if args.len() >= 2 => commands::add(&args[1..]),
        Some("install") if args.len() == 1 => commands::install(false),
        Some("update") if args.len() == 1 => commands::install(true),
        Some("list") if args.len() == 1 => commands::list(),
        Some("publish") if args.len() == 1 => commands::publish(),
        Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(message) => {
            eprintln!("{message}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
