//! Explicit immutable capture of known configured credential values. Never scan
//! the whole environment, run services or echo config/lookup errors. Required
//! provider placeholders resolve once; disabled-service values are optional.

use super::{RedactionError, Redactor};
use crate::config::Config;
use crate::route::ChatRoute;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const MAX_NAMES: usize = 64;
const MAX_NAME: usize = 128;
const MAX_VALUE: usize = 16 * 1024;
const MAX_TEMPLATE: usize = 32 * 1024;
const MAX_SOURCES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Origin {
    Provider,
    Platform,
    Environment,
    Search,
    ProviderHeader,
    DisabledMcp,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum CaptureError {
    #[error("configured credential source is invalid or unavailable")]
    Credential,
    #[error("configured credential capture exceeds its resource bound")]
    Bound,
    #[error("configured credentials cannot form a safe bounded redactor")]
    Redactor,
}
impl From<RedactionError> for CaptureError {
    fn from(_: RedactionError) -> Self {
        Self::Redactor
    }
}
/// No Debug/Serialize: captures contain actual credential values. The route is
/// the SAME resolved value consumed by the private builder for every role.
pub(crate) struct Captured {
    route: ChatRoute,
    redactor: Redactor,
    origins: BTreeMap<Origin, usize>,
}
impl Captured {
    pub(crate) fn capture(cfg: &Config, names: &[String]) -> Result<Self, CaptureError> {
        Self::with_lookup(cfg, names, |name| match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => Err(CaptureError::Credential),
        })
    }
    pub(crate) fn with_lookup(
        cfg: &Config,
        configured_names: &[String],
        mut lookup: impl FnMut(&str) -> Result<Option<String>, CaptureError>,
    ) -> Result<Self, CaptureError> {
        let mut sources = vec![
            (Origin::Platform, cfg.platform.api_token.clone()),
            (
                Origin::Search,
                cfg.search_api_key().unwrap_or("").to_string(),
            ),
        ];
        for server in &cfg.mcp_servers {
            for field in ["headers", "env"] {
                if let Some(values) = server.get(field).and_then(serde_json::Value::as_object) {
                    sources.extend(
                        values
                            .values()
                            .filter_map(serde_json::Value::as_str)
                            .map(|v| (Origin::DisabledMcp, v.to_owned())),
                    );
                }
            }
        }
        let model = cfg.model.as_deref().unwrap_or("");
        let profile = cfg.model_profile(model);
        for options in [
            profile.get("default_headers"),
            cfg.extra
                .get("model_kwargs")
                .and_then(|v| v.get("default_headers")),
        ] {
            if let Some(headers) = options.and_then(serde_json::Value::as_object) {
                sources.extend(
                    headers
                        .values()
                        .filter_map(serde_json::Value::as_str)
                        .map(|v| (Origin::ProviderHeader, v.to_owned())),
                );
            }
        }
        if sources.len() > MAX_SOURCES {
            return Err(CaptureError::Bound);
        }
        if configured_names.len() > MAX_NAMES {
            return Err(CaptureError::Bound);
        }
        let mut names: BTreeSet<String> = configured_names.iter().cloned().collect();
        names.extend(template_names(&cfg.api_key)?.into_iter().map(str::to_owned));
        // Optional disabled sources can be unresolvable/malformed without making
        // an unavailable service active. Known names/value seeds remain captured.
        for (_, source) in &sources {
            if let Ok(found) = template_names(source) {
                names.extend(found.into_iter().map(str::to_owned));
            }
        }
        if names.len() > MAX_NAMES
            || names.iter().any(|n| {
                n.is_empty()
                    || n.len() > MAX_NAME
                    || !n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            })
        {
            return Err(CaptureError::Bound);
        }
        let mut snapshot = BTreeMap::new();
        for name in names {
            let value = lookup(&name)?;
            if value.as_ref().is_some_and(|s| s.len() > MAX_VALUE) {
                return Err(CaptureError::Bound);
            }
            snapshot.insert(name, value);
        }
        let key = expand(&cfg.api_key, &snapshot)?.ok_or(CaptureError::Credential)?;
        let route = crate::route::with_resolved_key(cfg, key.clone());
        let mut values = Vec::new();
        let mut origins = BTreeMap::new();
        let mut seed = |origin, value: String| -> Result<(), CaptureError> {
            if value.is_empty() {
                return Ok(());
            }
            if value.len() > MAX_VALUE || values.len() >= MAX_SOURCES {
                return Err(CaptureError::Bound);
            }
            values.push(value.into_bytes());
            *origins.entry(origin).or_insert(0) += 1;
            Ok(())
        };
        seed(Origin::Provider, key)?;
        for value in snapshot.values().flatten() {
            seed(Origin::Environment, value.clone())?;
        }
        for (origin, source) in sources {
            // Header/search values are actually passed literally by current
            // clients; capture that literal in addition to optional expansion.
            if matches!(
                origin,
                Origin::ProviderHeader | Origin::Search | Origin::DisabledMcp
            ) {
                seed(origin, source.clone())?;
            }
            match expand(&source, &snapshot) {
                Ok(Some(resolved)) => seed(origin, resolved)?,
                Ok(None) | Err(CaptureError::Credential) => {}
                Err(error) => return Err(error),
            }
        }
        if cfg.platform.api_token.trim().is_empty() {
            if let Some(Some(value)) = snapshot.get("CRAFTSMAN_PLATFORM_TOKEN") {
                seed(Origin::Platform, value.trim().to_string())?;
            }
        }
        Ok(Self {
            route,
            redactor: Redactor::from_values(values)?,
            origins,
        })
    }
    pub(crate) fn route(&self) -> &ChatRoute {
        &self.route
    }
    pub(crate) fn redactor(&self) -> &Redactor {
        &self.redactor
    }
    pub(crate) fn origin_count(&self, origin: Origin) -> usize {
        self.origins.get(&origin).copied().unwrap_or(0)
    }
}
fn template_names(raw: &str) -> Result<Vec<&str>, CaptureError> {
    if raw.len() > MAX_TEMPLATE {
        return Err(CaptureError::Bound);
    }
    let mut names = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let end = after.find('}').ok_or(CaptureError::Credential)?;
        let name = &after[..end];
        if name.is_empty()
            || name.len() > MAX_NAME
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(CaptureError::Credential);
        }
        names.push(name);
        rest = &after[end + 1..];
        if names.len() > MAX_NAMES {
            return Err(CaptureError::Bound);
        }
    }
    Ok(names)
}
fn expand(
    raw: &str,
    snapshot: &BTreeMap<String, Option<String>>,
) -> Result<Option<String>, CaptureError> {
    template_names(raw)?;
    let mut output = String::new();
    let mut rest = raw;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find('}').ok_or(CaptureError::Credential)?;
        let Some(Some(value)) = snapshot.get(&after[..end]) else {
            return Ok(None);
        };
        output.push_str(value);
        if output.len() > MAX_VALUE {
            return Err(CaptureError::Bound);
        }
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    if output.len() > MAX_VALUE {
        return Err(CaptureError::Bound);
    }
    Ok(Some(output))
}
