//! User configuration loading.
//!
//! Settings live in `~/.config/tau/` as Nickel: `cli.ncl` and
//! `harness.ncl`, each with an optional `*.d/*.ncl` drop-in directory
//! for layered overrides. See
//! [`settings`] for the schema and loader entry points.
//!
//! Resolved-harness types and the user-vs-builtin extension resolver
//! live in `tau-harness` — this crate just owns the on-disk schema.

pub mod atomic;
pub mod settings;
