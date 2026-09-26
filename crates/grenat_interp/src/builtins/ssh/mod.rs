//! `Ssh`: commands and files on other machines, over SSH and SFTP (see
//! `grenat_ssh`).
//!
//! ```ruby
//! server = Ssh.connect("deploy@api.acme.com", key: Credentials.fetch(:deploy, :ssh_key))
//! res = server.run(["systemctl", "restart", "shop"])   # SshResult: status, ok?, stdout, stderr
//! server.upload("dist/app.tar.gz", "/srv/app.tar.gz")  # and download(remote, local)
//! sftp = server.sftp
//! sftp.list("/var/log")                                # SftpEntry: name, size, dir?, modified
//! sftp.read("/srv/app/VERSION")                        # write, remove, mkdir, rename, exists?
//! server.close
//! ```
//!
//! Reaching a server is an `ssh` effect restricted by host
//! (`uses ssh("api.acme.com")`), checked when connecting and at every call;
//! a transfer also reads (`upload`, `fs.read`) or writes (`download`,
//! `fs.write`) a local file. Nothing untrusted reaches the server (the
//! target, a command, a path, a file's content) and what comes back is
//! untrusted. Secrets serve to connect (`key:`, `passphrase:`, `password:`,
//! `proxy:`) and go nowhere else. Tests reach no server: `mock_ssh` stands
//! for one.

mod connect;
mod errors;
mod mock;
mod session;
mod sftp;

pub(crate) use connect::*;
pub(crate) use session::*;
pub(crate) use sftp::*;

use crate::prelude::*;
use crate::ssh::{Connection, SharedSsh};

/// Refuses what may not reach the server through `target`: an untrusted
/// value (unless validated) or a secret.
fn outgoing<'p>(args: &Args<'p>, target: &str) -> Result<(), Ctrl<'p>> {
    let values = || args.pos.iter().chain(args.named.iter().map(|(_, v)| v));
    if values().any(Value::contains_taint) {
        return raise("TaintError", format!("an untrusted value reaches `{target}` (effect `ssh`) without validation"));
    }
    if values().any(Value::contains_secret) {
        return raise(
            "SecretError",
            format!(
                "a secret never goes to the server through `{target}`: it serves to connect \
                 (`key:`, `passphrase:`, `password:`, `proxy:`)"
            ),
        );
    }
    Ok(())
}

impl<'p> Interp<'p> {
    /// The connection behind an `SshSession` or `Sftp` record (its `id`).
    fn ssh_entry(&self, fields: &Fields<'p>) -> Result<SharedSsh, Ctrl<'p>> {
        let found = match field(fields, "id") {
            Some(Value::Int(id)) => self.ssh_sessions.borrow().get(*id as usize).cloned(),
            _ => None,
        };
        found.map_or_else(|| raise("SshError", "not an SSH connection"), Ok)
    }

    /// Runs `op` on the connection of `fields` in a blocking section, once
    /// the `ssh` capability allows its host.
    fn on_server<T: Send>(
        &self,
        fields: &Fields<'p>,
        op: impl FnOnce(&mut Connection, &str) -> Result<T, grenat_ssh::Error> + Send,
    ) -> Result<T, Ctrl<'p>> {
        let entry = self.ssh_entry(fields)?;
        self.check_ssh(&entry.target.host)?;
        let label = entry.target.label();
        let mut guard = entry.connection.lock();
        let connection: &mut Connection = &mut guard;
        grenat_green::blocking(|| op(connection, &label)).or_else(errors::ssh_error)
    }

    /// `[ssh] deploy@api.acme.com: <what>` under `--log`.
    fn log_ssh(&self, fields: &Fields<'p>, what: &str) {
        if self.log
            && let Ok(entry) = self.ssh_entry(fields)
        {
            self.write_err(&format!("[ssh] {}: {what}\n", entry.target.label()));
        }
    }
}
