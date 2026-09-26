//! The configuration of a Grenat application, as a Rails developer would
//! expect it:
//!
//! - `config/credentials.yml.enc`: secrets in YAML, encrypted (AES-256-GCM)
//!   with `config/master.key` or `GRENAT_MASTER_KEY`; one file per
//!   environment when needed (`config/credentials/production.yml.enc`, its
//!   key `config/credentials/production.key`), the environment being
//!   `GRENAT_ENV` (`development` by default);
//! - `config/*.yml`: plain YAML (the models, …).
//!
//! Values come out as JSON trees; nothing here knows the language.

pub mod credentials;
pub mod yaml;

/// The environment: `GRENAT_ENV`, or `development`.
pub fn environment() -> String {
    std::env::var("GRENAT_ENV").ok().filter(|e| !e.is_empty()).unwrap_or_else(|| "development".into())
}
