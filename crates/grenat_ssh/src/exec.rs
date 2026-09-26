//! Running one remote command on its own channel and collecting what it
//! wrote and how it ended. A command that fails still gives an [`Output`];
//! only a command that cannot start is an error.

use russh::client::Handle;
use russh::{ChannelMsg, Sig};

use crate::error::{Error, ErrorKind};
use crate::handler::Client;

/// What a remote command did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    /// Its exit status; `None` when it was killed by a signal or the server
    /// did not say.
    pub status: Option<u32>,
    /// The signal that killed it (`TERM`, `KILL`…).
    pub signal: Option<String>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    /// Whether it exited with status 0.
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }
}

/// Runs `command_line` (already quoted) and waits for it to end.
pub async fn run(handle: &Handle<Client>, command_line: &str) -> Result<Output, Error> {
    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|e| Error::new(ErrorKind::Command, format!("cannot open a channel for the command: {e}")))?;
    channel
        .exec(true, command_line)
        .await
        .map_err(|e| Error::new(ErrorKind::Command, format!("cannot send the command: {e}")))?;
    let mut output = Output::default();
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } => output.stdout.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, ext: 1 } => output.stderr.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => output.status = Some(exit_status),
            ChannelMsg::ExitSignal { signal_name, .. } => output.signal = Some(signal(signal_name)),
            ChannelMsg::Failure => {
                return Err(Error::new(ErrorKind::Command, "the server refused to run the command"));
            }
            _ => {}
        }
    }
    Ok(output)
}

fn signal(sig: Sig) -> String {
    match sig {
        Sig::Custom(name) => name,
        known => format!("{known:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outputs_and_signals() {
        assert!(Output { status: Some(0), ..Output::default() }.success());
        assert!(!Output { status: Some(1), ..Output::default() }.success());
        assert!(!Output::default().success());
        assert_eq!(signal(Sig::TERM), "TERM");
        assert_eq!(signal(Sig::Custom("WINCH".into())), "WINCH");
    }
}
