//! Rate limits shared by every task of a process: what `limit net("host"),
//! 10.per_second, concurrent: 4` declares. Nothing here knows the Grenat
//! language.
//!
//! **A rate** ([`Rate`]): so many calls per period, `10` per second, `5000`
//! per hour.
//!
//! **A token bucket** ([`Bucket`]): as many calls as the rate allows at once
//! (a full bucket, after a quiet while), then one each `period / count`.
//! A call over the limit is not refused: it is given the instant it may go,
//! and waits until then. Calls go in the order they asked.
//!
//! **Slots** ([`Slots`]): at most so many calls in flight at once, the
//! others queued, first come first served.
//!
//! **A limiter** ([`Limiter`]): both, and a pause (a server answered `429`
//! with `Retry-After`: nobody calls it before then). [`Limiter::acquire`]
//! waits through a function the caller gives — a green sleep, cancellable,
//! in the interpreter; a clock moved by hand, in tests — and returns a
//! [`Permit`], whose slot is given back when it is dropped.
//!
//! **`Retry-After`** ([`retry_after`]): seconds, or an HTTP date in any of
//! the three formats a recipient must accept (RFC 9110, §10.2.3 and §5.6.7).

mod bucket;
mod limiter;
mod rate;
pub mod retry_after;
mod slots;

pub use bucket::{Bucket, Turn};
pub use limiter::{Limiter, Permit, SLOT_POLL};
pub use rate::Rate;
pub use slots::Slots;
