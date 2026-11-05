use super::{ErrorKind, GadgetError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;
pub const MAX_COMPONENT_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_GADGETS_PER_ROOT: usize = 64;

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    #[serde(default)]
    pub read: BTreeSet<String>,
    #[serde(default)]
    pub write: BTreeSet<String>,
    #[serde(default)]
    pub http: BTreeSet<String>,
}

impl Capabilities {
    pub fn validate(&self) -> Result<(), GadgetError> {
        if self
            .read
            .iter()
            .chain(&self.write)
            .any(|s| s != "workspace")
        {
            return Err(GadgetError::new(
                ErrorKind::InvalidGadget,
                "read/write scopes must be `workspace`",
            ));
        }
        if self.http.len() > 64 {
            return Err(GadgetError::new(
                ErrorKind::InvalidGadget,
                "at most 64 HTTP hosts are allowed",
            ));
        }
        for host in &self.http {
            let parsed = url::Url::parse(&format!("https://{host}/"))
                .map_err(|_| GadgetError::new(ErrorKind::InvalidGadget, "invalid HTTP host"))?;
            let hostname = host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && label
                        .as_bytes()
                        .first()
                        .is_some_and(u8::is_ascii_alphanumeric)
                    && label
                        .as_bytes()
                        .last()
                        .is_some_and(u8::is_ascii_alphanumeric)
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            });
            if !hostname
                || host.is_empty()
                || host.len() > 253
                || parsed.host_str() != Some(host.as_str())
                || parsed.port().is_some()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
                || parsed.path() != "/"
                || parsed.query().is_some()
                || parsed.fragment().is_some()
                || host.contains(['/', ':', '@', '?', '#', '\\'])
                || host.ends_with('.')
            {
                return Err(GadgetError::new(
                    ErrorKind::InvalidGadget,
                    format!(
                        "HTTP capability must be an exact lowercase hostname (no URL, port, wildcard or credentials): {host}"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// A rebuild cannot inherit a grant that is narrower than its declaration.
    pub fn is_subset_of(&self, granted: &Self) -> bool {
        self.read.is_subset(&granted.read)
            && self.write.is_subset(&granted.write)
            && self.http.is_subset(&granted.http)
    }
}

/// Records the legacy merge identity and effective file order. Plugin names
/// allow identifiers that gadget directory ids do not; migration normalizes the
/// directory id without changing collisions or command/agent names.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Migration {
    pub plugin_name: String,
    pub order: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<crate::extensions::Command>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<crate::extensions::Agent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migration: Option<Migration>,
}

impl Manifest {
    pub fn parse(text: &str, directory_name: &str) -> Result<Self, GadgetError> {
        if text.len() > MAX_MANIFEST_BYTES {
            return Err(GadgetError::new(
                ErrorKind::InvalidGadget,
                "manifest exceeds 256 KiB",
            ));
        }
        let manifest: Self = toml::from_str(text).map_err(|e| {
            GadgetError::new(
                ErrorKind::InvalidGadget,
                format!("invalid gadget.toml: {e}"),
            )
        })?;
        if !valid_id(&manifest.id) || manifest.id != directory_name {
            return Err(GadgetError::new(
                ErrorKind::InvalidGadget,
                "gadget id must be 2-64 lowercase ASCII letters/digits/hyphens and match its directory",
            ));
        }
        if manifest.version.is_empty()
            || manifest.version.len() > 64
            || manifest.version.chars().any(char::is_control)
            || manifest.description.len() > 16 * 1024
        {
            return Err(GadgetError::new(
                ErrorKind::InvalidGadget,
                "invalid version or oversized description",
            ));
        }
        manifest.capabilities.validate()?;
        if !crate::extensions::valid_surfaces(&manifest.commands, &manifest.agents)
            || manifest.migration.as_ref().is_some_and(|m| {
                !crate::extensions::valid_name(&m.plugin_name) || m.order >= MAX_GADGETS_PER_ROOT
            })
        {
            return Err(GadgetError::new(
                ErrorKind::InvalidGadget,
                "invalid declarative commands, agents or migration metadata",
            ));
        }
        Ok(manifest)
    }

    pub(crate) fn into_extension(self) -> crate::extensions::Extension {
        crate::extensions::Extension {
            name: self.migration.map(|m| m.plugin_name).unwrap_or(self.id),
            kind: crate::extensions::Kind::Gadget,
            prompt: self.prompt,
            commands: self.commands,
            agents: self.agents,
        }
    }
}

pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    (2..=64).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.last().is_some_and(u8::is_ascii_alphanumeric)
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}
