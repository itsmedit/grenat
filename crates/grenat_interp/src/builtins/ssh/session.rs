//! The methods of an `SshSession`: `run` a command (an `SshResult`),
//! `upload` and `download` a file, open `sftp`, `close`.

use super::sftp::{SFTP, file_method};
use super::{errors, outgoing};
use crate::prelude::*;

/// The record `run` returns.
pub(crate) const SSH_RESULT: &str = "SshResult";

pub(crate) fn ssh_session_method<'p>(
    interp: &mut Interp<'p>,
    fields: &Fields<'p>,
    name: &str,
    args: Args<'p>,
) -> R<'p> {
    match name {
        "run" => run(interp, fields, &args),
        "upload" | "download" => file_method(interp, fields, name, &args),
        "sftp" => {
            outgoing(&args, name)?;
            interp.on_server(fields, |connection, label| connection.files(label, |_| Ok(())))?;
            let host = field(fields, "host").cloned().unwrap_or(Value::Nil);
            let id = field(fields, "id").cloned().unwrap_or(Value::Nil);
            Ok(Value::record(SFTP, vec![("id".into(), id), ("host".into(), host)]))
        }
        "close" => {
            let entry = interp.ssh_entry(fields)?;
            let mut guard = entry.connection.lock();
            let connection: &mut crate::ssh::Connection = &mut guard;
            grenat_green::blocking(|| connection.close()).or_else(errors::ssh_error)?;
            Ok(Value::Nil)
        }
        _ => raise("NoMethodError", format!("unknown method `{name}` for an SSH session")),
    }
}

/// `server.run(["systemctl", "restart", "shop"])`: each argument arrives
/// as one argument, whatever it holds (`grenat_ssh` quotes it).
fn run<'p>(interp: &mut Interp<'p>, fields: &Fields<'p>, args: &Args<'p>) -> R<'p> {
    outgoing(args, "run")?;
    let argv: Vec<String> = match args.pos.first() {
        Some(Value::Array(items)) if !items.borrow().is_empty() && args.named.is_empty() => {
            items.borrow().iter().map(Value::to_display).collect()
        }
        _ => {
            return raise(
                "ArgumentError",
                "`run` expects a command and its arguments: `server.run([\"systemctl\", \"restart\", \"shop\"])`",
            );
        }
    };
    let output = interp.on_server(fields, |connection, label| connection.run(&argv, label))?;
    let status = output.status.map_or(-1, i64::from);
    interp
        // the server names the signal: escaped, so that it cannot forge a log line
        .log_ssh(
            fields,
            &format!(
                "{} → {}",
                argv.join(" "),
                output.signal.as_deref().map_or(status.to_string(), |s| s.escape_debug().to_string())
            ),
        );
    let text = |bytes: Vec<u8>| Value::str(String::from_utf8_lossy(&bytes)).taint();
    Ok(Value::record(
        SSH_RESULT,
        vec![
            ("status".into(), Value::Int(status)),
            // the server names the signal as it likes: untrusted too
            ("signal".into(), output.signal.map_or(Value::Nil, |name| Value::str(name).taint())),
            ("stdout".into(), text(output.stdout)),
            ("stderr".into(), text(output.stderr)),
        ],
    ))
}

/// `res.ok?`: the command exited with 0.
pub(crate) fn ssh_result_method<'p>(fields: &Fields<'p>, name: &str) -> Option<R<'p>> {
    (name == "ok?").then(|| Ok(Value::Bool(matches!(field(fields, "status"), Some(Value::Int(0))))))
}
