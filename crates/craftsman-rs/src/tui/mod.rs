//! Ratatui-based interactive terminal client.
//!
//! The terminal has one owner: model deltas, tool activity, approvals, input,
//! and status updates all enter the application as events and are rendered in
//! a single frame. Headless/serve modes continue using their existing sinks.

mod app;
mod commands;
mod input;
mod modals;
mod model_commands;
mod render;
mod session_bridge;
mod shutdown;
mod terminal;

pub use app::run;
