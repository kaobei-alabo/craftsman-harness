//! Suspension interrupts: a paused turn awaiting a human decision. Fully
//! serializable so Plan 3's checkpointer can persist and resume it.

use crate::state::ToolCall;
use serde::{Deserialize, Serialize};

/// One level of the agent-nesting path a suspension travelled through.
/// `frames[0]` is the top-level agent; the last frame is where the gate fired.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SuspensionFrame {
    pub agent_name: String,
}

/// The approval being requested.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HitlRequest {
    pub tool_call: ToolCall,
    /// Human-readable one-liner the app renders in its prompt.
    pub summary: String,
}

/// A paused turn.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Suspension {
    pub frames: Vec<SuspensionFrame>,
    pub request: HitlRequest,
}

/// The human's resolution of a suspended tool call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum HitlAction {
    Approve,
    ApproveEdited { args: serde_json::Value },
    Deny { feedback: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suspension_round_trips_including_nesting() {
        let s = Suspension {
            frames: vec![
                SuspensionFrame {
                    agent_name: "main".into(),
                },
                SuspensionFrame {
                    agent_name: "researcher".into(),
                },
            ],
            request: HitlRequest {
                tool_call: crate::state::ToolCall {
                    id: "c9".into(),
                    name: "execute".into(),
                    args: serde_json::json!({"command": "rm x"}),
                },
                summary: "execute rm x".into(),
            },
        };
        let back: Suspension = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.frames.len(), 2);
        assert_eq!(back.frames[1].agent_name, "researcher");
        assert_eq!(back.request.tool_call.name, "execute");
    }

    #[test]
    fn hitl_action_serializes_tagged() {
        let a = HitlAction::Deny {
            feedback: "no".into(),
        };
        let j = serde_json::to_string(&a).unwrap();
        assert!(j.contains("\"action\":\"deny\""));
        let b: HitlAction = serde_json::from_str(&j).unwrap();
        assert_eq!(a, b);
    }
}
