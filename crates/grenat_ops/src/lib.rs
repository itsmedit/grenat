//! The operations store of a Grenat application: what the runtime keeps in
//! the application's database — jobs, approvals that wait for a human, model
//! calls and their cost, events (failures, refusals), eval runs — and the
//! journals of workflows, on disk.
//!
//! The runtime writes it (`grenat serve`, `grenat eval`); `grenat console`
//! reads it and acts on it (approve, retry). Tables are created when first
//! used, in SQL that SQLite and PostgreSQL both accept; times are seconds
//! since the epoch. Nothing here knows the language: values are the
//! runtime's own encoding (JSON).

pub mod approvals;
pub mod calls;
pub mod evals;
pub mod events;
pub mod jobs;
pub mod journal;
mod row;

pub use grenat_db::Connection;

/// What a store operation failed with (the database's message).
pub type Result<T> = std::result::Result<T, String>;
