//! A folder watched for files dropped in it: what wakes `on_file`.
//! Nothing here knows the Grenat language.
//!
//! **Patterns** ([`Glob`]): `inbox/*.pdf`, `drive/**/*.{pdf,PDF}`; the
//! directory written out is the *base*, what is watched and scanned.
//!
//! **Scans decide** ([`scan`]): the files under the base that match, each
//! with its size and time of last change ([`Stamp`]). A link is followed
//! only to a file inside the base; one leading out is [`Refused`].
//!
//! **The system wakes** ([`Wake`], the `watch` feature): `notify` tells of
//! a change (inotify on Linux, FSEvents on macOS) and a scan follows at
//! once; without it — a directory not there yet, a network drive, the
//! system's watch limit reached, a registration still under way — scans
//! run on a timer only. Tests may tell of changes themselves
//! ([`FakeNotifier`]).
//!
//! **Opened, then checked** ([`open_inside`]): what a scan saw may be
//! swapped for a link before it is read, so a file is opened without
//! following a link and the file opened must be a regular file inside the
//! base; its content is read from that descriptor.
//!
//! **Settled, then once per content** ([`Tracker`]): a file is handed over
//! once its stamp has stopped changing for the settling time, and only if
//! its content ([`digest`], SHA-256) is not the one handed over last for its
//! path; a failed run is tried again a few times, then given up.

mod digest;
#[cfg(feature = "watch")]
mod fake;
mod glob;
mod open;
mod scan;
mod settle;
#[cfg(feature = "watch")]
mod wake;

pub use digest::digest;
#[cfg(feature = "watch")]
pub use fake::FakeNotifier;
pub use glob::{Glob, MAX_ALTERNATIVES};
pub use open::{LEADING_OUT, OpenError, open_inside};
pub use scan::{Found, Refused, Scan, Stamp, scan};
pub use settle::{MAX_ATTEMPTS, Observed, Tracker};
#[cfg(feature = "watch")]
pub use wake::{Waiter, Wake};
