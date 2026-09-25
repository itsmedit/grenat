//! Standard library, one module per family of functions.

mod args;
mod attachments;
mod collections;
mod db;
mod globals;
mod html;
mod http;
mod mail;
mod methods;
mod modules;
mod numbers;
mod shell;
mod strings;
mod triggers;
mod web;

pub(crate) use args::*;
pub(crate) use attachments::*;
pub(crate) use collections::*;
pub(crate) use db::*;
pub(crate) use globals::*;
pub(crate) use http::*;
pub(crate) use mail::*;
pub(crate) use methods::*;
pub(crate) use modules::*;
pub(crate) use numbers::*;
pub(crate) use shell::*;
pub(crate) use strings::*;
pub(crate) use triggers::*;
pub(crate) use web::*;
