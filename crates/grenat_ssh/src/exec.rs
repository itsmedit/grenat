//! Running one remote command on its own channel and collecting what it
//! wrote and how it ended. A command that fails still gives an [`Output`];
//! a command that cannot start is an error, as is a connection lost before
//! the command ended.

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
    let mut closed = false;
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } => output.stdout.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, ext: 1 } => output.stderr.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => output.status = Some(exit_status),
            ChannelMsg::ExitSignal { signal_name, .. } => output.signal = Some(signal(signal_name)),
            ChannelMsg::Failure => {
                return Err(Error::new(ErrorKind::Command, "the server refused to run the command"));
            }
            ChannelMsg::Close => closed = true,
            _ => {}
        }
    }
    ended(output, closed, command_line)
}

/// What the channel's messages say, once it is gone: `closed` when the
/// server closed it. Without that, nor how the command ended, the
/// connection broke while it ran: the command's fate is unknown.
fn ended(output: Output, closed: bool, command_line: &str) -> Result<Output, Error> {
    if closed || output.status.is_some() || output.signal.is_some() {
        Ok(output)
    } else {
        Err(Error::new(ErrorKind::Connect, format!("the SSH server closed the connection while `{command_line}` ran")))
    }
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

    #[test]
    fn a_channel_gone_without_an_end_is_a_lost_connection() {
        let exited = Output { status: Some(0), ..Output::default() };
        assert_eq!(ended(exited.clone(), false, "true"), Ok(exited));
        let killed = Output { signal: Some("TERM".into()), ..Output::default() };
        assert_eq!(ended(killed.clone(), false, "sleep 9"), Ok(killed));
        // the server closed the channel without saying how the command ended
        assert_eq!(ended(Output::default(), true, "x"), Ok(Output::default()));
        let e =
            ended(Output { stdout: b"started".to_vec(), ..Output::default() }, false, "sh -c 'sleep 3'").unwrap_err();
        assert_eq!(
            (e.kind(), e.message()),
            (ErrorKind::Connect, "the SSH server closed the connection while `sh -c 'sleep 3'` ran")
        );
    }
}
