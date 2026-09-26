//! The files of a server over SFTP: the methods of an `Sftp` record (and
//! the transfers of an `SshSession`). Paths are the server's; a transfer's
//! local file is checked against `fs.read` (`upload`) or `fs.write`
//! (`download`). What is read is untrusted: a listing, a file's content.

use std::path::Path;
use std::time::UNIX_EPOCH;

use grenat_ssh::Entry;

use super::outgoing;
use crate::builtins::{arg, str_arg};
use crate::prelude::*;

/// What `server.sftp` returns.
pub(crate) const SFTP: &str = "Sftp";
/// An entry of `sftp.list`.
pub(crate) const SFTP_ENTRY: &str = "SftpEntry";

/// `list`, `read`, `write`, `upload`, `download`, `remove`, `mkdir`,
/// `rename`, `exists?`.
pub(crate) fn file_method<'p>(interp: &mut Interp<'p>, fields: &Fields<'p>, name: &str, args: &Args<'p>) -> R<'p> {
    outgoing(args, name)?;
    let path = |i: usize| str_arg(args, i, name).map(|p| p.to_string());
    match name {
        "list" => {
            let dir = path(0)?;
            let entries = interp.on_server(fields, |c, label| c.files(label, |files| files.list(&dir)))?;
            interp.log_ssh(fields, &format!("list {dir}"));
            Ok(Value::array(entries.into_iter().map(entry).collect()).taint())
        }
        "read" => {
            let file = path(0)?;
            let bytes = interp.on_server(fields, |c, label| c.files(label, |files| files.read(&file)))?;
            interp.log_ssh(fields, &format!("read {file}"));
            Ok(Value::str(String::from_utf8_lossy(&bytes)).taint())
        }
        "write" => {
            let (file, content) = (path(0)?, arg(args, 1, name)?.to_display());
            interp.on_server(fields, |c, label| c.files(label, |files| files.write(&file, content.as_bytes())))?;
            interp.log_ssh(fields, &format!("write {file}"));
            Ok(Value::Nil)
        }
        "upload" => {
            let (local, remote) = (path(0)?, path(1)?);
            interp.check_fs("fs.read", &local)?;
            interp.on_server(fields, |c, label| c.files(label, |files| files.upload(Path::new(&local), &remote)))?;
            interp.log_ssh(fields, &format!("upload {local} → {remote}"));
            Ok(Value::Nil)
        }
        "download" => {
            let (remote, local) = (path(0)?, path(1)?);
            interp.check_fs("fs.write", &local)?;
            interp.on_server(fields, |c, label| c.files(label, |files| files.download(&remote, Path::new(&local))))?;
            interp.log_ssh(fields, &format!("download {remote} → {local}"));
            Ok(Value::Nil)
        }
        "remove" | "mkdir" => {
            let target = path(0)?;
            interp.on_server(fields, |c, label| {
                c.files(label, |files| if name == "remove" { files.remove(&target) } else { files.mkdir(&target) })
            })?;
            interp.log_ssh(fields, &format!("{name} {target}"));
            Ok(Value::Nil)
        }
        "rename" => {
            let (from, to) = (path(0)?, path(1)?);
            interp.on_server(fields, |c, label| c.files(label, |files| files.rename(&from, &to)))?;
            interp.log_ssh(fields, &format!("rename {from} → {to}"));
            Ok(Value::Nil)
        }
        "exists?" => {
            let target = path(0)?;
            let found = interp.on_server(fields, |c, label| c.files(label, |files| files.exists(&target)))?;
            Ok(Value::Bool(found))
        }
        _ => raise("NoMethodError", format!("unknown method `{name}` for SFTP")),
    }
}

/// `SftpEntry(name:, size:, dir?:, modified:)`, `modified` in seconds since
/// the epoch (`nil` when the server does not say).
fn entry<'p>(entry: Entry) -> Value<'p> {
    let modified = entry.modified.and_then(|t| t.duration_since(UNIX_EPOCH).ok());
    Value::record(
        SFTP_ENTRY,
        vec![
            ("name".into(), Value::str(entry.name)),
            ("size".into(), Value::Int(entry.size as i64)),
            ("dir?".into(), Value::Bool(entry.is_dir)),
            ("modified".into(), modified.map_or(Value::Nil, |d| Value::Float(d.as_secs_f64()))),
        ],
    )
}
