use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("cancelled by the user")]
    Cancelled,
    #[error("model error: {0}")]
    Model(String),
    #[error("tool '{name}' error: {message}")]
    Tool { name: String, message: String },
    #[error("middleware '{name}' error: {message}")]
    Middleware { name: String, message: String },
    #[error("max rounds ({0}) exceeded")]
    MaxRounds(usize),
    #[error("state error: {0}")]
    State(String),
    #[error("suspended pending human approval")]
    Suspended(Box<crate::interrupt::Suspension>),
}
