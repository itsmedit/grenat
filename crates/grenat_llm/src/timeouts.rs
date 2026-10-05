//! How long a provider may take. A call answered whole has ten minutes in
//! all; a streamed one has no limit on the whole — a long answer keeps its
//! tokens coming for longer than that — but a limit on silence: no byte for
//! that long (Anthropic sends `ping`s in between) and the stream is broken.
//!
//! The silence is measured on each read of the connection, by a transport
//! wrapped around ureq's own (TCP, TLS, proxies).

use std::time::Duration;

use ureq::unversioned::resolver::DefaultResolver;
use ureq::unversioned::transport::time::Duration as Wait;
use ureq::unversioned::transport::{Buffers, ConnectionDetails, Connector, DefaultConnector, NextTimeout, Transport};
use ureq::{Agent, Timeout};

/// The limits of a provider's calls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timeouts {
    /// A call answered whole, from its start to its end.
    pub whole: Duration,
    /// Opening a connection.
    pub connect: Duration,
    /// A streamed call: the longest silence between two reads.
    pub silence: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            whole: Duration::from_secs(600),
            connect: Duration::from_secs(30),
            silence: Duration::from_secs(300),
        }
    }
}

impl Timeouts {
    /// The agent of calls answered whole.
    pub(crate) fn whole_agent(&self) -> Agent {
        Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(self.connect))
            .timeout_global(Some(self.whole))
            .build()
            .into()
    }

    /// The agent of streamed calls: no limit on the whole, one on silence.
    pub(crate) fn stream_agent(&self) -> Agent {
        let config = Agent::config_builder().http_status_as_error(false).timeout_connect(Some(self.connect)).build();
        let connector = DefaultConnector::new().chain(Silence { after: self.silence });
        Agent::with_parts(config, connector, DefaultResolver::default())
    }
}

/// Wraps each connection in a [`Quiet`] transport.
#[derive(Debug)]
struct Silence {
    after: Duration,
}

impl Connector<Box<dyn Transport>> for Silence {
    type Out = Quiet;

    fn connect(
        &self,
        _: &ConnectionDetails,
        chained: Option<Box<dyn Transport>>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        Ok(chained.map(|inner| Quiet { inner, after: self.after }))
    }
}

/// A connection whose reads wait no longer than `after`.
#[derive(Debug)]
struct Quiet {
    inner: Box<dyn Transport>,
    after: Duration,
}

impl Transport for Quiet {
    fn buffers(&mut self) -> &mut dyn Buffers {
        self.inner.buffers()
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        self.inner.transmit_output(amount, timeout)
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        let shorter = timeout.after.is_not_happening() || *timeout.after > self.after;
        // named as what it is, a silence while the body is read, not ureq's global limit
        let timeout =
            if shorter { NextTimeout { after: Wait::Exact(self.after), reason: Timeout::RecvBody } } else { timeout };
        self.inner.await_input(timeout)
    }

    fn is_open(&mut self) -> bool {
        self.inner.is_open()
    }

    fn is_tls(&self) -> bool {
        self.inner.is_tls()
    }
}
