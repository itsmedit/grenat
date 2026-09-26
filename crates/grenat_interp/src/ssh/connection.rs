//! One SSH connection as the interpreter keeps it: a real session and its
//! SFTP channel (opened when first needed), a test double, or closed. Every
//! call blocks; callers make it in a blocking section.

use std::sync::{Arc, Mutex};

use grenat_ssh::{Error, ErrorKind, Output, Session, Sftp};

use super::double::Double;
use super::files::RemoteFiles;
use super::target::Target;

pub(crate) enum Connection {
    Real(Box<Live>),
    /// A `mock_ssh` server, shared by the connections a test opens to it.
    Double(Arc<Mutex<Double>>),
    Closed,
}

/// A real session, and its SFTP channel once opened.
pub(crate) struct Live {
    pub session: Session,
    pub sftp: Option<Sftp>,
}

/// A connection and where it goes (the port resolved), used by one task at a time.
pub(crate) struct SshEntry {
    pub target: Target,
    pub connection: grenat_green::Mutex<Connection>,
}

pub(crate) type SharedSsh = Arc<SshEntry>;

impl SshEntry {
    pub(crate) fn new(target: Target, connection: Connection) -> SharedSsh {
        Arc::new(SshEntry { target, connection: grenat_green::Mutex::new(connection) })
    }
}

impl Connection {
    /// Runs `argv` on the server; `label` names it if the connection is closed.
    pub(crate) fn run(&mut self, argv: &[String], label: &str) -> Result<Output, Error> {
        match self {
            Connection::Real(live) => live.session.run(argv),
            Connection::Double(double) => double.lock().unwrap_or_else(|e| e.into_inner()).run(argv),
            Connection::Closed => Err(closed(label)),
        }
    }

    /// Calls `f` with the server's files (opening SFTP first if needed).
    pub(crate) fn files<T>(
        &mut self,
        label: &str,
        f: impl FnOnce(&mut dyn RemoteFiles) -> Result<T, Error>,
    ) -> Result<T, Error> {
        match self {
            Connection::Real(live) => {
                if live.sftp.is_none() {
                    live.sftp = Some(live.session.sftp()?);
                }
                f(live.sftp.as_mut().expect("opened"))
            }
            Connection::Double(double) => f(&mut *double.lock().unwrap_or_else(|e| e.into_inner())),
            Connection::Closed => Err(closed(label)),
        }
    }

    /// Ends the connection; closing it again does nothing.
    pub(crate) fn close(&mut self) -> Result<(), Error> {
        match std::mem::replace(self, Connection::Closed) {
            Connection::Real(live) => {
                let Live { session, sftp } = *live;
                drop(sftp);
                session.close()
            }
            Connection::Double(_) | Connection::Closed => Ok(()),
        }
    }
}

fn closed(label: &str) -> Error {
    Error::new(ErrorKind::Connect, format!("the SSH connection to {label} is closed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_connection_refuses_everything() {
        let double = Double::new("a@b", Vec::new(), vec![("/x".into(), b"1".to_vec())]);
        let mut connection = Connection::Double(Arc::new(Mutex::new(double)));
        assert_eq!(connection.files("a@b", |files| files.read("/x")).unwrap(), b"1");
        connection.close().unwrap();
        let e = connection.run(&["true".into()], "a@b").unwrap_err();
        assert_eq!((e.kind(), e.message()), (ErrorKind::Connect, "the SSH connection to a@b is closed"));
        assert!(connection.files("a@b", |files| files.exists("/x")).is_err());
        connection.close().unwrap();
    }
}
