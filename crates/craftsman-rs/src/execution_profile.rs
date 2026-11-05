//! Explicit security/capability profiles for Craftsman agent execution.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionProfile {
    /// User-owned local CLI/TUI/desktop execution.
    Local,
    /// Per-user isolated workspace pod execution.
    Workspace,
    /// Shared stateless runtime with virtual resources and no real filesystem.
    ServiceAgent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionCapabilities {
    pub real_filesystem: bool,
    pub virtual_filesystem: bool,
    pub shell: bool,
    pub local_sessions: bool,
    pub persistent_home: bool,
}

impl ExecutionProfile {
    pub const fn capabilities(self) -> ExecutionCapabilities {
        match self {
            Self::Local => ExecutionCapabilities {
                real_filesystem: true,
                virtual_filesystem: false,
                shell: true,
                local_sessions: true,
                persistent_home: true,
            },
            Self::Workspace => ExecutionCapabilities {
                real_filesystem: true,
                virtual_filesystem: false,
                shell: true,
                local_sessions: true,
                persistent_home: true,
            },
            Self::ServiceAgent => ExecutionCapabilities {
                real_filesystem: false,
                virtual_filesystem: true,
                shell: false,
                local_sessions: false,
                persistent_home: false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_agent_is_virtual_only() {
        let caps = ExecutionProfile::ServiceAgent.capabilities();
        assert!(caps.virtual_filesystem);
        assert!(!caps.real_filesystem);
        assert!(!caps.shell);
        assert!(!caps.local_sessions);
        assert!(!caps.persistent_home);
    }

    #[test]
    fn local_and_workspace_keep_existing_real_fs_behavior() {
        for profile in [ExecutionProfile::Local, ExecutionProfile::Workspace] {
            let caps = profile.capabilities();
            assert!(caps.real_filesystem);
            assert!(caps.shell);
            assert!(caps.local_sessions);
            assert!(caps.persistent_home);
        }
    }
}
