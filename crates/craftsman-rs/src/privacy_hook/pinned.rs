//! The production reply source: one exact, approved, user-level gadget.

use super::{HookError, ReplySource};
use crate::gadgets::{ErrorKind, GadgetRuntime, PinnedSource, RuntimeOptions};
use std::path::Path;

/// A host's selection of one detector source: which gadget, which function
/// and the SHA-256 of its component and manifest files.
///
/// It is built by host code from the user's own policy. It is a selector, not
/// an approval: the call still requires the installed bytes to hash to these
/// values and a stored grant for those bytes. A project, a model, a gadget or
/// a detector reply cannot produce or widen one.
pub struct DetectorPin {
    source: PinnedSource,
    function: String,
}

impl DetectorPin {
    pub fn supplied_by_host(
        gadget: &str,
        function: &str,
        component_sha256: &str,
        manifest_sha256: &str,
    ) -> Result<Self, HookError> {
        let hash = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if gadget.is_empty()
            || gadget.len() > 64
            || function.is_empty()
            || function.len() > 64
            || !hash(component_sha256)
            || !hash(manifest_sha256)
        {
            return Err(HookError::Source);
        }
        Ok(Self {
            source: PinnedSource {
                id: gadget.to_owned(),
                component_sha256: component_sha256.to_owned(),
                manifest_sha256: manifest_sha256.to_owned(),
            },
            function: function.to_owned(),
        })
    }
}

/// Owns its own gadget runtime with the engine's default call bounds, no
/// prompt, no allow-list, no workspace trust and no persistent native-code
/// cache, so the component is always compiled from the bytes just hashed.
pub struct PinnedDetector {
    runtime: GadgetRuntime,
    pin: DetectorPin,
}

impl PinnedDetector {
    /// Whether a call would pass its source checks now: the installed bytes
    /// are the pinned ones, declare no capability and have a stored approval.
    /// Nothing is compiled, described or run, and nothing is approved.
    pub async fn source_approved(&self) -> Result<(), HookError> {
        match self.runtime.pinned_source_approved(&self.pin.source).await {
            Ok(()) => Ok(()),
            Err(_) => Err(HookError::Source),
        }
    }

    /// The detector as the user installed it under `config_root`, pinned to
    /// the bytes that are there now. A selector for the enable writer only:
    /// it approves nothing, and `source_approved` must still accept it.
    pub async fn installed(
        workspace: &Path,
        config_root: &Path,
        gadget: &str,
        function: &str,
    ) -> Result<Self, HookError> {
        let options = RuntimeOptions {
            disk_cache: false,
            ..RuntimeOptions::default()
        };
        let runtime =
            GadgetRuntime::new(workspace, config_root, options).map_err(|_| HookError::Source)?;
        let Ok(source) = runtime.installed_pin(gadget).await else {
            return Err(HookError::Source);
        };
        let pin = DetectorPin::supplied_by_host(
            gadget,
            function,
            &source.component_sha256,
            &source.manifest_sha256,
        )?;
        Ok(Self { runtime, pin })
    }

    /// The pinned SHA-256 of the component and of the manifest, as lowercase hex.
    pub fn pins(&self) -> (&str, &str) {
        (
            &self.pin.source.component_sha256,
            &self.pin.source.manifest_sha256,
        )
    }

    /// `workspace` is the session's workspace root and `config_root` the
    /// user's configuration root that holds `gadgets/<id>/`. Nothing is read
    /// from the gadget here; every call checks the source again.
    pub fn new(workspace: &Path, config_root: &Path, pin: DetectorPin) -> Result<Self, HookError> {
        let options = RuntimeOptions {
            disk_cache: false,
            ..RuntimeOptions::default()
        };
        let runtime =
            GadgetRuntime::new(workspace, config_root, options).map_err(|_| HookError::Source)?;
        Ok(Self { runtime, pin })
    }
}

#[async_trait::async_trait]
impl ReplySource for PinnedDetector {
    async fn reply(&self, request: &serde_json::Value) -> Result<String, HookError> {
        match self
            .runtime
            .call_pinned(&self.pin.source, &self.pin.function, request.clone())
            .await
        {
            // Only the returned document is used. Logs, timings and counters
            // of the call are not authority and are dropped here.
            Ok(result) => Ok(result.json),
            Err(error) => Err(match error.kind {
                ErrorKind::Trap
                | ErrorKind::Timeout
                | ErrorKind::OutOfMemory
                | ErrorKind::OutputLimit
                | ErrorKind::Failed => HookError::Execution,
                ErrorKind::NotFound
                | ErrorKind::NeedsApproval
                | ErrorKind::InvalidArgs
                | ErrorKind::DeniedCapability
                | ErrorKind::InvalidGadget
                | ErrorKind::Io => HookError::Source,
            }),
        }
    }
}
