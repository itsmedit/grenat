//! The private asynchronous runtime under the blocking API: one
//! current-thread tokio runtime per session, driven only while a call
//! blocks on it. Callers never see tokio.

use std::future::Future;
use std::time::Duration;

use crate::error::{Error, ErrorKind};

pub struct Runtime(tokio::runtime::Runtime);

impl Runtime {
    pub fn new() -> Result<Runtime, Error> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map(Runtime)
            .map_err(|e| Error::new(ErrorKind::Connect, format!("cannot start the SSH client: {e}")))
    }

    /// Runs `work` to completion, blocking the calling thread.
    pub fn block_on<T>(&self, work: impl Future<Output = Result<T, Error>>) -> Result<T, Error> {
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(Error::new(
                ErrorKind::Connect,
                "the blocking SSH client cannot be called from inside an asynchronous runtime",
            ));
        }
        self.0.block_on(work)
    }
}

/// `work`, or a [`ErrorKind::Timeout`] error saying what took too long.
pub async fn within<T>(
    limit: Option<Duration>,
    what: impl FnOnce() -> String,
    work: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    match limit {
        None => work.await,
        Some(limit) => tokio::time::timeout(limit, work).await.unwrap_or_else(|_| {
            Err(Error::new(ErrorKind::Timeout, format!("{} took longer than {}", what(), seconds(limit))))
        }),
    }
}

fn seconds(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 1.0 {
        format!("{} ms", d.as_millis())
    } else if s == s.trunc() {
        format!("{s} s")
    } else {
        format!("{s:.1} s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_and_times_out() {
        let rt = Runtime::new().unwrap();
        assert_eq!(rt.block_on(async { Ok(1) }).unwrap(), 1);
        let slow = rt.block_on(within(Some(Duration::from_millis(20)), || "sleeping".into(), async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(())
        }));
        let e = slow.unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Timeout);
        assert_eq!(e.message(), "sleeping took longer than 20 ms");
        assert_eq!(rt.block_on(within(None, || unreachable!(), async { Ok(2) })).unwrap(), 2);
        assert_eq!(seconds(Duration::from_secs(30)), "30 s");
        assert_eq!(seconds(Duration::from_millis(1500)), "1.5 s");
    }

    #[test]
    fn refuses_nested_runtimes() {
        let outer = Runtime::new().unwrap();
        let inner = Runtime::new().unwrap();
        let e = outer.block_on(async { inner.block_on(async { Ok(()) }) }).unwrap_err();
        assert!(e.message().contains("asynchronous runtime"), "{e}");
    }
}
