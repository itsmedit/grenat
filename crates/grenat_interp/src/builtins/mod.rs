//! Bibliothèque intégrée, un module par famille de fonctions.

mod args;
mod collections;
mod globals;
mod methods;
mod modules;
mod numbers;
mod strings;

pub(crate) use args::*;
pub(crate) use collections::*;
pub(crate) use globals::*;
pub(crate) use methods::*;
pub(crate) use modules::*;
pub(crate) use numbers::*;
pub(crate) use strings::*;
