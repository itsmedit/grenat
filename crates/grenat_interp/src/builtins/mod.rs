//! Standard library, one module per family of functions.

mod args;
mod attachments;
mod audio;
mod collections;
mod db;
mod events;
mod globals;
mod html;
mod http;
pub(crate) mod iso8601;
mod mail;
mod methods;
mod modules;
mod numbers;
mod shell;
mod ssh;
mod strings;
mod time;
mod triggers;
mod web;

pub(crate) use args::*;
pub(crate) use attachments::*;
pub(crate) use audio::*;
pub(crate) use collections::*;
pub(crate) use db::*;
pub(crate) use events::*;
pub(crate) use globals::*;
pub(crate) use http::*;
pub(crate) use mail::*;
pub(crate) use methods::*;
pub(crate) use modules::*;
pub(crate) use numbers::*;
pub(crate) use shell::*;
pub(crate) use ssh::*;
pub(crate) use strings::*;
pub(crate) use time::*;
pub(crate) use triggers::*;
pub(crate) use web::*;

/// The records Grenat builds itself, which a program cannot declare: a
/// `struct Attachment` of its own would pass for audio read from a file.
/// Mirrors `grenat_types::builtins::RECORD_NAMES`.
pub(crate) const RECORD_NAMES: &[&str] = &[
    ATTACHMENT,
    crate::eval::transcription::TRANSCRIPT_SEGMENT,
    crate::eval::conversation::CONVERSATION,
    DATABASE,
    RESPONSE,
    REQUEST,
    RESPONSE_RECORD,
    MAILER,
    SHELL_RESULT,
    SSH_SESSION,
    SSH_RESULT,
    SFTP,
    SFTP_ENTRY,
    EVENT_STREAM,
];
