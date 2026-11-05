//! Private foundations for the accepted headless v1 contract.
//!
//! There is deliberately no CLI entry point yet: the private prepared runtime
//! selects confinement, but frontend/stdin input, guest-host, redacted storage, shared usage,
//! output/trace and provider boundaries must exist before exposing `craftsman exec`.

pub(crate) mod confined_fs;
pub(crate) mod construction;
#[allow(dead_code)] // Private input factory; CLI/stdin/public producers remain unwired.
pub(crate) mod input;
pub(crate) mod permissions;
#[allow(dead_code)] // Private seeded foundation; publication adapters remain open.
pub(crate) mod redaction;
pub(crate) mod runtime;

pub(crate) mod shell;

pub(crate) mod web;

pub(crate) mod framework;

#[allow(dead_code)] // Private retained-message view; no output/storage producer yet.
pub(crate) mod messages;
pub(crate) mod provider;

#[allow(dead_code)] // Private finalization authority, not an IO/public result writer.
pub(crate) mod terminal;

#[allow(dead_code)] // Private pre-run file plan/terminal publisher, no CLI or public IO.
pub(crate) mod output;

// Actual-builder shared request-only admission; token/usage/public adapters remain open.
pub(crate) mod meter;
