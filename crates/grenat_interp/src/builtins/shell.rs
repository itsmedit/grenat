//! `Shell`: running programs.
//!
//! ```ruby
//! res = Shell.run(["kubectl", "rollout", "restart", "deploy/api"], timeout: 60)
//! res.ok?; res.status; res.stdout; res.stderr
//! Shell.run(["pytest", "-q"], cwd: "./app", env: {"CI" => "1"}, network: false)
//! ```
//!
//! A program is an argument vector, never a shell line: no argument is ever
//! interpreted by a shell. Running one is a `shell` effect, restricted by
//! program (`uses shell("kubectl")`). Nothing untrusted goes in (arguments,
//! environment), and what comes out (`stdout`, `stderr`) is untrusted.
//! Programs run with a clean environment, a timeout (60 s by default), and
//! without network with `network: false`.

use std::time::Duration;

use crate::prelude::*;
use crate::process::{ProcessReply, ProcessRequest};


/// The record `Shell.run` returns.
pub(crate) const SHELL_RESULT: &str = "ShellResult";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) fn call_shell<'p>(interp: &mut Interp<'p>, name: &str, args: Args<'p>) -> R<'p> {
    if name != "run" {
        return raise("NoMethodError", format!("unknown method `Shell.{name}`"));
    }
    if args.pos.iter().chain(args.named.iter().map(|(_, v)| v)).any(Value::contains_taint) {
        return raise("TaintError", "an untrusted value reaches `Shell.run` (effect `shell`) without validation");
    }
    let argv: Vec<String> = match args.pos.first() {
        Some(Value::Array(items)) if !items.borrow().is_empty() => items.borrow().iter().map(Value::to_display).collect(),
        _ => return raise("ArgumentError", "`Shell.run` expects a program and its arguments: `Shell.run([\"git\", \"status\"])`"),
    };
    let program = std::path::Path::new(&argv[0]).file_name().map_or(argv[0].clone(), |n| n.to_string_lossy().into_owned());
    interp.check_program(&program)?;
    let mut request = ProcessRequest { argv, cwd: None, env: Vec::new(), timeout: DEFAULT_TIMEOUT, network: true };
    for (option, value) in &args.named {
        match (option.as_str(), value) {
            ("cwd", Value::Str(dir)) => request.cwd = Some(dir.to_string()),
            ("env", Value::Hash(entries)) => {
                request.env = entries.borrow().iter().map(|(k, v)| (k.to_display(), v.to_display())).collect();
            }
            ("network", Value::Bool(allowed)) => request.network = *allowed,
            ("timeout", Value::Int(n)) if *n > 0 => request.timeout = Duration::from_secs(*n as u64),
            ("timeout", Value::Float(s) | Value::Duration(s)) if *s > 0.0 => {
                request.timeout = Duration::from_secs_f64(*s);
            }
            (option, value) => {
                return raise("ArgumentError", format!("invalid `Shell.run` option `{option}: {}`", value.inspect()));
            }
        }
    }
    let reply = interp.process(&request)?;
    if interp.log {
        interp.write_err(&format!("[shell] {} → {}\n", request.argv.join(" "), reply.status));
    }
    Ok(result(reply))
}

/// `ShellResult(status:, stdout:, stderr:)`, the outputs tainted.
fn result<'p>(reply: ProcessReply) -> Value<'p> {
    Value::record(
        SHELL_RESULT,
        vec![
            ("status".into(), Value::Int(reply.status)),
            ("stdout".into(), Value::str(reply.stdout).taint()),
            ("stderr".into(), Value::str(reply.stderr).taint()),
        ],
    )
}

/// `res.ok?`: the program exited with 0.
pub(crate) fn shell_result_method<'p>(fields: &Fields<'p>, name: &str) -> Option<R<'p>> {
    let status = fields.iter().find(|(k, _)| &**k == "status").map(|(_, v)| v.clone());
    (name == "ok?").then(|| Ok(Value::Bool(matches!(status, Some(Value::Int(0))))))
}
