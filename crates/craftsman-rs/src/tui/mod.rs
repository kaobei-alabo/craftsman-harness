//! Ratatui-based interactive terminal client.
//!
//! The terminal has one owner: model deltas, tool activity, approvals, input,
//! and status updates all enter the application as events and are rendered in
//! a single frame. ACP and noninteractive modes keep their existing sinks.

mod app;
mod commands;
mod input;
mod modals;
mod model_commands;
mod program_status;
mod provider_setup;
mod render;
mod session_bridge;
#[cfg(feature = "gadgets")]
pub(crate) use session_bridge::CaptureBuildOwner;
#[cfg(all(test, feature = "gadgets"))]
pub(crate) use session_bridge::script_capture_tests::script_capture_fixture;
mod shutdown;
mod terminal;

pub use app::run;
