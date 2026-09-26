//! SSH connections as the interpreter keeps them: where they go, a real
//! session (`grenat_ssh`) or a test double (`mock_ssh`) behind the same
//! operations. Nothing here knows about the language's values.

mod connection;
mod double;
mod files;
mod target;

pub(crate) use connection::{Connection, Live, SharedSsh, SshEntry};
pub(crate) use double::Double;
pub(crate) use target::Target;
