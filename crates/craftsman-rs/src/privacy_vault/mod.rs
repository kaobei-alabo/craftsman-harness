//! Private, reversible host-data pseudonyms and a separate encrypted local vault.
//!
//! No normal session, model, tool, hook, configuration or keystore selects this
//! module. Supplied host keys/namespaces are an integration boundary, not proof
//! of actual user, normal-session or OS-keystore authority. The app binary's
//! Rust reachability does not expose a CLI or a framework/library interface.

mod crypto;
mod format;
mod storage;

#[cfg(test)]
mod tests;

use sha2::{Digest, Sha256};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use zeroize::Zeroizing;

pub use crypto::HostKey;
pub use storage::Vault;

pub const VAULT_FILENAME: &str = "privacy-vault.db";
pub const FORMAT_VERSION: u32 = 1;

/// Diagnostics are fixed, without SQL/OS/provider errors, paths, hashes or data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PrivacyError {
    #[error("The private privacy operation is closed.")]
    Closed,
    #[error("The private host key is unavailable or invalid.")]
    Key,
    #[error("The private vault or authenticated mapping is invalid.")]
    Integrity,
    #[error("The complete private privacy operation exceeds its bounds.")]
    Bounds,
    #[error("The private privacy input is invalid or ambiguous.")]
    Input,
    #[error("The declared private privacy format is unsupported.")]
    Unsupported,
    #[error("The exact alias is unavailable in this private scope.")]
    UnknownAlias,
    #[error("The private privacy data belongs to a different host scope.")]
    ForeignScope,
    #[error("A private privacy alias collides with admitted input or mappings.")]
    Collision,
    #[error("The private vault is busy.")]
    Busy,
    #[error("The private vault IO did not complete successfully.")]
    Io,
    #[error("The private vault worker did not return successfully.")]
    Worker,
    #[error("The private privacy operation exceeded its elapsed-time bound.")]
    Deadline,
}

pub type Result<T> = std::result::Result<T, PrivacyError>;

/// Only a host adapter implements this. There is no credential/config fallback,
/// persisted master key, unlock flag, or model-supplied authority Boolean.
pub trait HostKeySource: Send + Sync {
    fn acquire_privacy_key(&self) -> Result<HostKey>;
}

/// Host budgets may tighten these ceilings; they cannot expand them. Elapsed
/// time is checked around actual IO. A started blocking syscall cannot be
/// interrupted by this API and is not claimed to obey an absolute OS deadline.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub text_bytes: usize,
    pub spans: usize,
    pub scope_mappings: usize,
    pub vault_mappings: usize,
    pub ciphertext_bytes: usize,
    pub work_bytes: usize,
    pub io_calls: usize,
    pub busy_timeout: Duration,
    pub elapsed: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            text_bytes: 256 * 1024,
            spans: 256,
            scope_mappings: 512,
            vault_mappings: 4096,
            ciphertext_bytes: 8 * 1024 * 1024,
            work_bytes: 64 * 1024 * 1024,
            io_calls: 4096,
            busy_timeout: Duration::from_millis(200),
            elapsed: Duration::from_secs(5),
        }
    }
}

impl Limits {
    fn validate(self) -> Result<Self> {
        let max = Self::default();
        let pairs = [
            (self.text_bytes, max.text_bytes),
            (self.spans, max.spans),
            (self.scope_mappings, max.scope_mappings),
            (self.vault_mappings, max.vault_mappings),
            (self.ciphertext_bytes, max.ciphertext_bytes),
            (self.work_bytes, max.work_bytes),
            (self.io_calls, max.io_calls),
        ];
        if pairs.iter().any(|(n, ceiling)| *n == 0 || n > ceiling)
            || self.scope_mappings > self.vault_mappings
            || self.busy_timeout > max.busy_timeout
            || self.elapsed.is_zero()
            || self.elapsed > max.elapsed
        {
            return Err(PrivacyError::Bounds);
        }
        Ok(self)
    }
}

/// An explicit absolute host config root. Open captures/checks this root in a
/// worker; it never calls Config::load/config_dir, creates directories, changes
/// user config, or opens threads.db. The production proposal is
/// config::config_dir()/privacy-vault.db, selected by a subsequent host adapter.
pub struct VaultLocation {
    root: PathBuf,
    opening: OpeningMode,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum OpeningMode {
    Legacy,
    ExistingOnly,
    CreateOnly,
}

impl VaultLocation {
    pub fn under_host_config_root(root: &Path) -> Result<Self> {
        if !root.is_absolute()
            || root.as_os_str().len() > 16 * 1024
            || root.components().count() > 128
            || root.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
            || root.to_str().is_none()
        {
            return Err(PrivacyError::Input);
        }
        Ok(Self {
            root: root.into(),
            opening: OpeningMode::Legacy,
        })
    }
}

/// All five names are host data, captured immutably and keyed before SQL. A
/// project/thread alone cannot reuse another host, profile, context or version.
pub struct HostNamespace {
    parts: [Zeroizing<String>; 5],
}

impl HostNamespace {
    pub fn supplied_by_host(
        host: &str,
        project: &str,
        thread: &str,
        profile: &str,
        context: &str,
    ) -> Result<Self> {
        let values = [host, project, thread, profile, context];
        if values
            .iter()
            .any(|s| s.is_empty() || s.len() > 256 || s.chars().any(char::is_control))
            || values.iter().map(|s| s.len()).sum::<usize>() > 1024
        {
            return Err(PrivacyError::Bounds);
        }
        Ok(Self {
            parts: values.map(|s| Zeroizing::new(s.to_owned())),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentifierKind {
    Customer,
    Account,
    Project,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    /// Narrow ASCII dot-atom email with a dotted ASCII DNS name.
    Email,
    /// Explicitly selected ASCII layout: 7–15 digits, optional leading '+',
    /// conservative spaces/hyphens/dots and at most one parenthesized group.
    /// Exact digits and separators define identity; no number normalization.
    Telephone,
    /// Exact host-declared literal identifiers, never automatic secret recovery.
    Identifier(IdentifierKind),
}

impl Category {
    fn code(self) -> i64 {
        match self {
            Self::Email => 1,
            Self::Telephone => 2,
            Self::Identifier(IdentifierKind::Customer) => 3,
            Self::Identifier(IdentifierKind::Account) => 4,
            Self::Identifier(IdentifierKind::Project) => 5,
        }
    }

    fn from_code(code: i64) -> Result<Self> {
        match code {
            1 => Ok(Self::Email),
            2 => Ok(Self::Telephone),
            3 => Ok(Self::Identifier(IdentifierKind::Customer)),
            4 => Ok(Self::Identifier(IdentifierKind::Account)),
            5 => Ok(Self::Identifier(IdentifierKind::Project)),
            _ => Err(PrivacyError::Integrity),
        }
    }
}

/// Values come from explicit host declarations. No names/addresses/medical/ID
/// detector claim, password/API-key/private-key restoration, or model authority
/// follows. Unused declarations are not persisted. Duplicates/ambiguity refuse.
pub struct DeclaredIdentifiers {
    values: Vec<(IdentifierKind, Zeroizing<String>)>,
}

impl DeclaredIdentifiers {
    pub fn supplied_by_host(values: &[(IdentifierKind, &str)]) -> Result<Self> {
        if values.len() > 128 || values.iter().map(|(_, s)| s.len()).sum::<usize>() > 32 * 1024 {
            return Err(PrivacyError::Bounds);
        }
        for (index, (_, value)) in values.iter().enumerate() {
            format::validate_identifier(value)?;
            if values[..index].iter().any(|(_, prior)| prior == value) {
                return Err(PrivacyError::Input);
            }
        }
        Ok(Self {
            values: values
                .iter()
                .map(|(kind, value)| (*kind, Zeroizing::new((*value).to_owned())))
                .collect(),
        })
    }

    pub fn none() -> Self {
        Self { values: Vec::new() }
    }

    fn owns(&self, kind: IdentifierKind, value: &str) -> bool {
        self.values
            .iter()
            .any(|(k, v)| *k == kind && v.as_str() == value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DetectorSpan {
    pub start: usize,
    pub end: usize,
    pub category: Category,
}

/// This report is untrusted data. Whole hash/range/UTF-8/overlap/category and
/// declaration checks occur before any replacement or mapping admission.
pub struct DetectorReport {
    pub input_sha256: [u8; 32],
    pub spans: Vec<DetectorSpan>,
}

/// Original bytes for a host-only detector, bound to this particular session.
/// The unkeyed digest is a span-input comparison, never an authority or model map.
pub struct CapturedText {
    text: Zeroizing<String>,
    digest: [u8; 32],
    live: Arc<SessionLife>,
}

impl CapturedText {
    pub fn input_sha256_for_host_detector(&self) -> Result<[u8; 32]> {
        self.live.check()?;
        Ok(self.digest)
    }

    pub fn with_host_detector_text<T>(&self, read: impl FnOnce(&str) -> T) -> Result<T> {
        self.live.check()?;
        let result = read(&self.text);
        self.live.check()?;
        Ok(result)
    }
}

struct SessionLife {
    open: AtomicBool,
    vault: Arc<storage::Inner>,
    scope: [u8; 32],
}

impl SessionLife {
    fn check(&self) -> Result<()> {
        if !self.open.load(Ordering::Acquire) {
            return Err(PrivacyError::Closed);
        }
        self.vault.check()
    }

    fn close(&self) {
        self.open.store(false, Ordering::Release);
    }
}

/// Immutable host scope and append-only identity policy. It cannot be rebound,
/// reset, granted by a model, or used to look through a foreign namespace.
pub struct MappingSession {
    live: Arc<SessionLife>,
}

impl MappingSession {
    pub fn capture_text(&self, text: &str) -> Result<CapturedText> {
        self.live.check()?;
        if text.len() > self.live.vault.limits.text_bytes {
            self.live.close();
            return Err(PrivacyError::Bounds);
        }
        Ok(CapturedText {
            text: Zeroizing::new(text.to_owned()),
            digest: Sha256::digest(text.as_bytes()).into(),
            live: self.live.clone(),
        })
    }

    /// Whole input/output and mappings, or a fixed error. Only an awaited actual
    /// commit produces MaskedText; there is no read/write/decrypt/raw fallback.
    pub async fn mask(
        &self,
        input: CapturedText,
        report: DetectorReport,
        declared: DeclaredIdentifiers,
    ) -> Result<MaskedText> {
        storage::mask(self.live.clone(), input, report, declared).await
    }

    /// Exact fields only, without scanning/guessing/normalizing arbitrary text.
    /// Unknown, altered, malformed or foreign aliases refuse the whole request.
    pub async fn resolve_exact_aliases(&self, aliases: &[&str]) -> Result<ResolutionRecords> {
        storage::resolve(self.live.clone(), aliases).await
    }

    pub fn close(&self) {
        self.live.close();
    }
}

/// Mapping values are private, bounded, zeroized host data; there is no raw map
/// getter or serialization. Existing records are never rewritten by production.
struct Mapping {
    category: Category,
    original: Zeroizing<String>,
    alias: Zeroizing<String>,
    counter: u32,
}

/// Model-facing access contains only the complete successfully persisted view.
/// Later callers must recheck immediately before sending; this API sends nothing
/// and cannot retract a reference already handed to a trusted host consumer.
pub struct MaskedText {
    text: Zeroizing<String>,
    input_digest: [u8; 32],
    replacements: usize,
    records: ResolutionRecords,
}

impl MaskedText {
    pub fn text_for_later_model_request(&self) -> Result<&str> {
        self.records.live.check()?;
        Ok(&self.text)
    }

    pub fn replacement_count(&self) -> Result<usize> {
        self.records.live.check()?;
        Ok(self.replacements)
    }

    pub fn input_sha256_for_host_validation(&self) -> Result<[u8; 32]> {
        self.records.live.check()?;
        Ok(self.input_digest)
    }

    pub fn resolution_records_for_host_fields(&self) -> Result<&ResolutionRecords> {
        self.records.live.check()?;
        Ok(&self.records)
    }
}

/// Immutable per-request records. These are historical alias observations, not
/// read/write permissions or file-content/staleness/escaping validation.
pub struct ResolutionRecords {
    live: Arc<SessionLife>,
    mappings: Vec<Mapping>,
}

/// Borrowed only inside an explicit host-field callback; never Debug/Serialize.
pub struct ReadonlyHostField<'a> {
    mapping: &'a Mapping,
}

impl ReadonlyHostField<'_> {
    pub fn known_alias(&self) -> &str {
        &self.mapping.alias
    }

    /// Original UTF-8 bytes. The later host layer must validate the exact field,
    /// destination/content/permission and escape for that field's syntax.
    pub fn original_for_validated_host_field(&self) -> &str {
        &self.mapping.original
    }

    pub fn category(&self) -> Category {
        self.mapping.category
    }
}

impl ResolutionRecords {
    pub fn len(&self) -> Result<usize> {
        self.live.check()?;
        Ok(self.mappings.len())
    }

    pub fn is_empty(&self) -> Result<bool> {
        self.len().map(|n| n == 0)
    }

    pub fn with_host_field<T>(
        &self,
        index: usize,
        read: impl FnOnce(ReadonlyHostField<'_>) -> T,
    ) -> Result<T> {
        self.live.check()?;
        let mapping = self.mappings.get(index).ok_or(PrivacyError::Input)?;
        let result = read(ReadonlyHostField { mapping });
        self.live.check()?;
        Ok(result)
    }

    pub fn validate_session_binding(&self, session: &MappingSession) -> Result<()> {
        self.live.check()?;
        session.live.check()?;
        if !Arc::ptr_eq(&self.live, &session.live) {
            return Err(PrivacyError::ForeignScope);
        }
        Ok(())
    }

    /// Only exact known aliases selected as complete host display fields. No
    /// dialing, network, tool-argument, shell/code or arbitrary text substitution.
    pub fn resolve_exact_display_field(&self, alias: &str) -> Result<HostDisplayField> {
        self.live.check()?;
        if alias.len() > 256 {
            return Err(PrivacyError::Bounds);
        }
        let mapping = self
            .mappings
            .iter()
            .find(|m| m.alias.as_str() == alias)
            .ok_or(PrivacyError::UnknownAlias)?;
        Ok(HostDisplayField {
            original: Zeroizing::new(mapping.original.to_string()),
            live: self.live.clone(),
        })
    }
}

pub struct HostDisplayField {
    original: Zeroizing<String>,
    live: Arc<SessionLife>,
}

impl HostDisplayField {
    /// Display-only host data; the actual renderer's control escaping is still
    /// required. This function has no side effects or automatic output sink.
    pub fn with_host_display_text<T>(&self, read: impl FnOnce(&str) -> T) -> Result<T> {
        self.live.check()?;
        let result = read(&self.original);
        self.live.check()?;
        Ok(result)
    }
}

macro_rules! private_debug {
    ($($type:ty),+ $(,)?) => {$(
        impl fmt::Debug for $type {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($type), "([private])"))
            }
        }
    )+};
}

private_debug!(
    VaultLocation,
    HostNamespace,
    DeclaredIdentifiers,
    DetectorReport,
    CapturedText,
    MappingSession,
    MaskedText,
    ResolutionRecords,
    HostDisplayField,
);
