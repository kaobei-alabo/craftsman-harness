//! Trusted, change-only OSC 7501 reports for the owned interactive terminal.
//!
//! Only fixed messages and locally assigned numeric child ids enter the wire.
//! Registry names are private lookup keys, never terminal text or identities.

use crate::config::Config;
use craftsman_harness::{AgentRegistry, AgentStatus};
use std::io::{self, Write};

const MAX_CHILDREN: usize = 32;
const MAX_NAME_BYTES: usize = 512;

pub(super) fn enabled(cfg: &Config) -> Result<bool, &'static str> {
    let Some(terminal) = cfg.extra.get("terminal") else {
        return Ok(true);
    };
    let terminal = terminal
        .as_object()
        .ok_or("Invalid terminal settings: terminal must be an object.")?;
    terminal.get("osc7501").map_or(Ok(true), |value| {
        value
            .as_bool()
            .ok_or("Invalid terminal.osc7501: use true or false.")
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Status {
    Idle,
    Working,
    Stopping,
    Permission,
    Question,
    Paused,
    Done,
    Error,
}

impl Status {
    fn wire_state(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working | Self::Stopping => "working",
            Self::Permission | Self::Question | Self::Paused => "blocked",
            Self::Done => "done",
            Self::Error => "error",
        }
    }

    fn kind(self) -> Option<&'static str> {
        match self {
            Self::Permission => Some("permission"),
            Self::Question | Self::Paused => Some("question"),
            _ => None,
        }
    }

    /// Standard base64 of fixed UTF-8 text, with no control characters.
    fn message(self) -> &'static str {
        match self {
            Self::Idle => "UmVhZHk=",                              // Ready
            Self::Working => "V29ya2luZw==",                       // Working
            Self::Stopping => "U3RvcHBpbmc=",                      // Stopping
            Self::Permission => "V2FpdGluZyBmb3IgYXBwcm92YWw=",    // Waiting for approval
            Self::Question => "V2FpdGluZyBmb3IgYW4gYW5zd2Vy",      // Waiting for an answer
            Self::Paused => "UGF1c2VkOyBzZW5kIGEgZm9sbG93LXVw",    // Paused; send a follow-up
            Self::Done => "UmVzcG9uc2UgcmVhZHk=",                  // Response ready
            Self::Error => "V29yayBzdG9wcGVkIGFmdGVyIGFuIGVycm9y", // Work stopped after an error
        }
    }

    fn from_agent(status: AgentStatus) -> Self {
        match status {
            AgentStatus::Running => Self::Working,
            AgentStatus::Suspended => Self::Permission,
            AgentStatus::Done => Self::Done,
            AgentStatus::Failed => Self::Error,
        }
    }

    fn live(self) -> bool {
        matches!(self, Self::Working | Self::Permission)
    }
}

#[derive(Clone)]
struct Child {
    name: String,
    id: u64,
    status: Status,
}

enum Record {
    Root(Status),
    Child(u64, Status),
    Overflow(Status),
    ClearChild(u64),
    ClearOverflow,
    ClearChildren,
}

impl Record {
    fn append(&self, bytes: &mut Vec<u8>) -> io::Result<()> {
        let (status, clear) = match self {
            Self::Root(status) | Self::Child(_, status) | Self::Overflow(status) => {
                (Some(*status), false)
            }
            Self::ClearChild(_) | Self::ClearOverflow | Self::ClearChildren => (None, true),
        };
        write!(
            bytes,
            "\x1b]7501;state={}:app=craftsman",
            if clear {
                "clear"
            } else {
                status
                    .expect("non-clear records carry a status")
                    .wire_state()
            }
        )?;
        match self {
            Self::Child(id, _) | Self::ClearChild(id) => {
                write!(bytes, ":id=craftsman/tasks/{id}")?;
            }
            Self::Overflow(_) | Self::ClearOverflow => {
                bytes.extend_from_slice(b":id=craftsman/tasks/overflow");
            }
            Self::ClearChildren => bytes.extend_from_slice(b":id=craftsman/tasks"),
            Self::Root(_) => {}
        }
        if let Some(status) = status {
            if let Some(kind) = status.kind() {
                write!(bytes, ":kind={kind}")?;
            }
            write!(bytes, ":msg={}", status.message())?;
        }
        bytes.extend_from_slice(b"\x1b\\");
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct Reporter {
    root: Option<Status>,
    children: Vec<Child>,
    overflow: Option<Status>,
    next_id: u64,
    finished: bool,
    failed: bool,
}

impl Reporter {
    pub(super) fn update(
        &mut self,
        output: &mut impl Write,
        root: Status,
        registry: Option<&AgentRegistry>,
        initializing: bool,
    ) -> io::Result<()> {
        if self.finished || self.failed {
            return Ok(());
        }
        let mut records = Vec::new();
        let mut children = self.children.clone();
        let mut overflow = self.overflow;
        let mut next_id = self.next_id;
        if let Some(registry) = registry {
            let observed = registry.list();
            children.retain_mut(|child| {
                let Ok(index) = observed.binary_search_by(|(name, _)| name.cmp(&child.name)) else {
                    records.push(Record::ClearChild(child.id));
                    return false;
                };
                let status = Status::from_agent(observed[index].1);
                if child.status != status {
                    records.push(Record::Child(child.id, status));
                    child.status = status;
                }
                true
            });
            for (name, status) in &observed {
                let status = Status::from_agent(*status);
                if !status.live()
                    || name.len() > MAX_NAME_BYTES
                    || children.iter().any(|child| &child.name == name)
                {
                    continue;
                }
                if children.len() == MAX_CHILDREN {
                    let Some(index) = children.iter().position(|child| !child.status.live()) else {
                        continue;
                    };
                    records.push(Record::ClearChild(children.remove(index).id));
                }
                // Exhaustion goes into the aggregate; never wrap or reuse an id.
                let Some(id) = next_id.checked_add(1) else {
                    continue;
                };
                next_id = id;
                children.push(Child {
                    name: name.clone(),
                    id,
                    status,
                });
                records.push(Record::Child(id, status));
            }
            let current_overflow = observed
                .iter()
                .filter_map(|(name, status)| {
                    let status = Status::from_agent(*status);
                    (status.live() && !children.iter().any(|child| &child.name == name))
                        .then_some(status)
                })
                .reduce(|left, right| {
                    if left == Status::Permission || right == Status::Permission {
                        Status::Permission
                    } else {
                        Status::Working
                    }
                });
            if current_overflow != overflow {
                records.push(match current_overflow {
                    Some(status) => Record::Overflow(status),
                    None => Record::ClearOverflow,
                });
                overflow = current_overflow;
            }
        } else if !initializing && (!children.is_empty() || overflow.is_some()) {
            records.push(Record::ClearChildren);
            children.clear();
            overflow = None;
        }
        // Children precede the root verdict so hosts see remaining child work.
        if self.root != Some(root) {
            records.push(Record::Root(root));
        }
        self.send(output, &records)?;
        self.root = Some(root);
        self.children = children;
        self.overflow = overflow;
        self.next_id = next_id;
        Ok(())
    }

    pub(super) fn finish(&mut self, output: &mut impl Write, status: Status) -> io::Result<()> {
        if self.finished || self.failed {
            return Ok(());
        }
        // One attempt, including a partial write. No status IO retries.
        self.finished = true;
        self.send(output, &[Record::ClearChildren, Record::Root(status)])
    }

    fn send(&mut self, output: &mut impl Write, records: &[Record]) -> io::Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        let mut bytes = Vec::new();
        for record in records {
            record.append(&mut bytes)?;
        }
        let result = output.write_all(&bytes).and_then(|()| output.flush());
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

#[cfg(test)]
mod tests;
