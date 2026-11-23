//! Trusted user-source policy only. Enabled privacy is unavailable until a
//! separate reviewed activation; project/merged extra values mint no authority.
use crate::privacy_key::{ConfigRoot, root::FileLease};
use serde::{
    Deserialize,
    de::{self, MapAccess, Visitor},
};
use std::fmt;
use zeroize::Zeroizing;
mod file_data;
use file_data::{CapturedFileRule, RawFileRule};
#[cfg(test)]
mod file_data_tests;

const POLICY_BYTES: usize = 32 * 1024;
const USER_CONFIG_BYTES: usize = crate::privacy_key::root::CONFIG_LIMIT;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    #[error("The user privacy policy is invalid or exceeds its bounds.")]
    Invalid,
    #[error("The user privacy policy source is unavailable or changed.")]
    Source,
    #[error("Enabled privacy is not yet available in this version.")]
    NotYetAvailable,
}
type Result<T> = std::result::Result<T, PolicyError>;

/// Fixed bounded source data, not source/capability approval or a hook role.
pub struct DetectorPolicy {
    gadget: String,
    function: String,
    component: [u8; 32],
    manifest: [u8; 32],
    literals: Vec<Zeroizing<String>>,
    file_rules: Vec<CapturedFileRule>,
}
impl fmt::Debug for DetectorPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DetectorPolicy([private])")
    }
}
impl DetectorPolicy {
    pub fn gadget(&self) -> &str {
        &self.gadget
    }
    pub fn function(&self) -> &str {
        &self.function
    }
    pub fn component_pin(&self) -> &[u8; 32] {
        &self.component
    }
    pub fn manifest_pin(&self) -> &[u8; 32] {
        &self.manifest
    }
    pub fn literals(&self) -> impl Iterator<Item = &str> {
        self.literals.iter().map(|s| s.as_str())
    }
    /// Test-only. A format read from two paths grants nothing: production
    /// asks `supported_data` and `retained_data_effect` for an actual request.
    #[cfg(test)]
    pub(crate) fn file_data_format(
        &self,
        workspace: &std::path::Path,
        destination: &std::path::Path,
    ) -> Option<crate::privacy_restore::FileDataFormat> {
        self.file_rules
            .iter()
            .find_map(|rule| rule.format_for(workspace, destination))
    }
    fn precheck_data_target(
        &self,
        workspace: &std::path::Path,
        request: &crate::privacy_session::FileTargetRequest,
    ) -> Option<crate::privacy_session::FileTargetPrecheck> {
        self.file_rules
            .iter()
            .find_map(|rule| rule.precheck(workspace, request))
    }
    fn supported_data(
        &self,
        workspace: &std::path::Path,
        request: &crate::privacy_session::FileDataRequest,
    ) -> Option<crate::privacy_session::SupportedFileData> {
        self.file_rules
            .iter()
            .find_map(|rule| rule.supported(workspace, request))
    }
    fn retained_data_effect(
        &self,
        workspace: &std::path::Path,
        request: &crate::privacy_session::FileEffectRequest,
    ) -> Option<crate::privacy_session::RetainedFileEffect> {
        self.file_rules
            .iter()
            .find_map(|rule| rule.retained(workspace, request))
    }
}

enum Policy {
    Absent,
    Disabled,
    Proposed(DetectorPolicy),
}
/// No from-Config/flag/string/status constructor and no Serialize/Clone. The
/// source descriptors are retained independently of later project config merges.
pub struct CapturedPolicy {
    policy: Policy,
    root: ConfigRoot,
    source: Option<FileLease>,
}

pub(crate) enum UserConfigurationCaptureError {
    BeforeSource(crate::privacy_key::KeyError),
    Invalid,
    Refused(PolicyError),
}

/// Issued only by reading a retained actual ConfigRoot (or its cfg(test)
/// fixture equivalent). No caller Config/JSON/pin/flag constructor exists.
/// Rule-directory capture waits until the actual frontend controls pass.
pub(crate) struct CapturedUserConfiguration {
    configuration: Option<crate::config::Config>,
    policy: Policy,
    root: ConfigRoot,
    source: Option<FileLease>,
    bytes: Option<Zeroizing<Vec<u8>>>,
}

/// Opaque startup-only original bytes and SAME held root/file source. It must
/// stay beside the returned CapturedPolicy; ordinary policy owners are unchanged.
pub struct RetainedUserConfigurationSource {
    root: ConfigRoot,
    source: Option<FileLease>,
    bytes: Option<Zeroizing<Vec<u8>>>,
}
impl fmt::Debug for RetainedUserConfigurationSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RetainedUserConfigurationSource([private])")
    }
}
impl RetainedUserConfigurationSource {
    pub fn recheck(&self) -> Result<()> {
        self.root.recheck().map_err(|_| PolicyError::Source)?;
        match (&self.source, &self.bytes) {
            (Some(source), Some(bytes)) => source
                .verify_configuration_bytes(bytes)
                .map_err(|_| PolicyError::Source)?,
            (None, None) => self
                .root
                .verify_config_absent()
                .map_err(|_| PolicyError::Source)?,
            _ => return Err(PolicyError::Source),
        }
        self.root.recheck().map_err(|_| PolicyError::Source)
    }
}

impl CapturedUserConfiguration {
    pub(crate) fn capture(
        root: ConfigRoot,
    ) -> std::result::Result<Self, UserConfigurationCaptureError> {
        let read = root
            .read_user_config()
            .map_err(UserConfigurationCaptureError::BeforeSource)?;
        let (configuration, policy, source, bytes) = match read {
            Some((bytes, source)) => {
                let configuration: crate::config::Config = serde_json::from_slice(&bytes)
                    .map_err(|_| UserConfigurationCaptureError::Invalid)?;
                let policy = if configuration.extra.contains_key("privacy") {
                    parse_user_source(&bytes).map_err(UserConfigurationCaptureError::Refused)?
                } else {
                    Policy::Absent
                };
                (Some(configuration), policy, Some(source), Some(bytes))
            }
            None => (None, Policy::Absent, None, None),
        };
        let captured = Self {
            configuration,
            policy,
            root,
            source,
            bytes,
        };
        captured
            .recheck()
            .map_err(UserConfigurationCaptureError::Refused)?;
        Ok(captured)
    }

    fn recheck(&self) -> Result<()> {
        self.root.recheck().map_err(|_| PolicyError::Source)?;
        match (&self.source, &self.bytes) {
            (Some(source), Some(bytes)) => source
                .verify_configuration_bytes(bytes)
                .map_err(|_| PolicyError::Source)?,
            (None, None) => self
                .root
                .verify_config_absent()
                .map_err(|_| PolicyError::Source)?,
            _ => return Err(PolicyError::Source),
        }
        self.root.recheck().map_err(|_| PolicyError::Source)
    }

    pub(crate) fn is_enabled(&self) -> bool {
        matches!(self.policy, Policy::Proposed(_))
    }

    #[cfg(feature = "gadgets")]
    pub(crate) fn has_identifiers(&self) -> bool {
        matches!(&self.policy, Policy::Proposed(policy) if !policy.literals.is_empty())
    }

    pub(crate) fn into_configuration_and_policy(
        self,
    ) -> Result<(
        Option<crate::config::Config>,
        CapturedPolicy,
        RetainedUserConfigurationSource,
    )> {
        self.recheck()?;
        let retained_source = RetainedUserConfigurationSource {
            root: self.root.duplicate_owned().map_err(|_| PolicyError::Source)?,
            source: self
                .source
                .as_ref()
                .map(FileLease::duplicate_owned)
                .transpose()
                .map_err(|_| PolicyError::Source)?,
            bytes: self.bytes,
        };
        let mut policy = CapturedPolicy {
            policy: self.policy,
            root: self.root,
            source: self.source,
        };
        if let Policy::Proposed(proposed) = &mut policy.policy {
            for rule in &mut proposed.file_rules {
                rule.capture()?;
            }
        }
        policy.recheck()?;
        retained_source.recheck()?;
        Ok((self.configuration, policy, retained_source))
    }

    pub(crate) fn into_legacy_configuration(self) -> Result<Option<crate::config::Config>> {
        self.recheck()?;
        if self.is_enabled() {
            return Err(PolicyError::NotYetAvailable);
        }
        Ok(self.configuration)
    }
}
impl fmt::Debug for CapturedPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CapturedPrivacyPolicy([private])")
    }
}
impl CapturedPolicy {
    pub fn capture_actual() -> Result<Self> {
        Self::capture(ConfigRoot::capture_actual().map_err(|_| PolicyError::Source)?)
    }
    pub(crate) fn capture_from_root(root: ConfigRoot) -> Result<Self> {
        Self::capture(root)
    }
    pub(crate) fn root(&self) -> &ConfigRoot {
        &self.root
    }
    fn capture(root: ConfigRoot) -> Result<Self> {
        let read = root.read_user_config().map_err(|_| PolicyError::Source)?;
        let (policy, source) = match read {
            Some((bytes, source)) => (parse_user_source(&bytes)?, Some(source)),
            None => (Policy::Absent, None),
        };
        let mut captured = Self {
            policy,
            root,
            source,
        };
        if let Policy::Proposed(policy) = &mut captured.policy {
            for rule in &mut policy.file_rules {
                rule.capture()?;
            }
        }
        captured.recheck()?;
        Ok(captured)
    }
    pub fn recheck(&self) -> Result<()> {
        self.root.recheck().map_err(|_| PolicyError::Source)?;
        if let Some(file) = &self.source {
            file.recheck().map_err(|_| PolicyError::Source)?;
        } else {
            self.root
                .verify_config_absent()
                .map_err(|_| PolicyError::Source)?;
        }
        if let Policy::Proposed(policy) = &self.policy {
            for rule in &policy.file_rules {
                rule.recheck()?;
                // The directories the rule holds, not the way it spells
                // them: a rule that leads into the held root is no data rule.
                if rule.reaches_root(&self.root) {
                    return Err(PolicyError::Invalid);
                }
            }
        }
        Ok(())
    }
    /// Data is exposed for the upcoming private session interface only. It
    /// grants no describe/compile/capability approval and no activation.
    pub fn proposed_detector(&self) -> Result<Option<&DetectorPolicy>> {
        self.recheck()?;
        Ok(match &self.policy {
            Policy::Proposed(policy) => Some(policy),
            _ => None,
        })
    }
    pub fn require_available(&self) -> Result<()> {
        self.recheck()?;
        match self.policy {
            Policy::Proposed(_) => Err(PolicyError::NotYetAvailable),
            _ => Ok(()),
        }
    }
    /// The ACTUAL configured user root is a protected role, regardless of its
    /// pathname or a model's declared purpose. User config/key/vault/detector/
    /// grants/agent instructions and other sources under it cannot become data.
    /// The rule that names a destination is judged by the directories it
    /// retains: if one of them is the root's own held directory, by device and
    /// inode, the destination is refused under any spelling. The pathname and
    /// protected-role refusals remain as further checks. For a destination that
    /// no rule names there is no retained directory to compare: only the name
    /// checks apply to it here, and it is no data for want of a rule.
    fn permits_data_destination(&self, destination: &std::path::Path) -> bool {
        self.root
            .with_captured_path(|root| Ok(!destination.starts_with(root)))
            == Ok(true)
            && !file_data::protected_role(destination)
            && !self.rule_reaches_root(destination)
    }
    fn rule_reaches_root(&self, destination: &std::path::Path) -> bool {
        match &self.policy {
            Policy::Proposed(policy) => policy
                .file_rules
                .iter()
                .any(|rule| rule.names(destination) && rule.reaches_root(&self.root)),
            _ => false,
        }
    }
    pub(crate) fn precheck_data_target(
        &self,
        workspace: &std::path::Path,
        request: &crate::privacy_session::FileTargetRequest,
    ) -> Option<crate::privacy_session::FileTargetPrecheck> {
        self.recheck().ok()?;
        if !self.permits_data_destination(request.destination()) {
            return None;
        }
        let precheck = self
            .proposed_detector()
            .ok()??
            .precheck_data_target(workspace, request)?;
        self.recheck().ok()?;
        Some(precheck)
    }
    pub(crate) fn supported_data(
        &self,
        workspace: &std::path::Path,
        request: &crate::privacy_session::FileDataRequest,
    ) -> Option<crate::privacy_session::SupportedFileData> {
        self.recheck().ok()?;
        if !self.permits_data_destination(request.destination()) {
            return None;
        }
        let supported = self
            .proposed_detector()
            .ok()??
            .supported_data(workspace, request)?;
        self.recheck().ok()?;
        Some(supported)
    }
    pub(crate) fn retained_data_effect(
        &self,
        workspace: &std::path::Path,
        request: &crate::privacy_session::FileEffectRequest,
    ) -> Option<crate::privacy_session::RetainedFileEffect> {
        self.recheck().ok()?;
        if !self.permits_data_destination(request.destination()) {
            return None;
        }
        let retained = self
            .proposed_detector()
            .ok()??
            .retained_data_effect(workspace, request)?;
        self.recheck().ok()?;
        Some(retained)
    }
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct RawPolicy {
    enabled: bool,
    #[serde(default)]
    detector: Option<RawDetector>,
    #[serde(default)]
    identifiers: Vec<String>,
    #[serde(default)]
    file_data: Vec<RawFileRule>,
}
#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct RawDetector {
    gadget: String,
    function: String,
    component_sha256: String,
    manifest_sha256: String,
}

struct UserSource(Option<RawPolicy>);
impl<'de> Deserialize<'de> for UserSource {
    fn deserialize<D: serde::Deserializer<'de>>(input: D) -> std::result::Result<Self, D::Error> {
        struct UserVisitor;
        impl<'de> Visitor<'de> for UserVisitor {
            type Value = UserSource;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("user configuration object")
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                let mut privacy = None;
                let mut seen = false;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "privacy" {
                        if seen {
                            return Err(de::Error::custom("duplicate privacy policy"));
                        }
                        seen = true;
                        privacy = Some(map.next_value::<RawPolicy>()?);
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(UserSource(privacy))
            }
        }
        input.deserialize_map(UserVisitor)
    }
}

fn pin(value: &str) -> Result<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(PolicyError::Invalid);
    }
    let mut bytes = [0; 32];
    for (index, part) in value.as_bytes().chunks_exact(2).enumerate() {
        let digit = |b| if b <= b'9' { b - b'0' } else { b - b'a' + 10 };
        bytes[index] = digit(part[0]) * 16 + digit(part[1]);
    }
    Ok(bytes)
}
fn parse_user_source(bytes: &[u8]) -> Result<Policy> {
    if bytes.len() > USER_CONFIG_BYTES {
        return Err(PolicyError::Invalid);
    }
    let UserSource(raw) = serde_json::from_slice(bytes).map_err(|_| PolicyError::Invalid)?;
    let Some(raw) = raw else {
        return Ok(Policy::Absent);
    };
    if serde_json::to_vec(&raw)
        .map_err(|_| PolicyError::Invalid)?
        .len()
        > POLICY_BYTES
    {
        return Err(PolicyError::Invalid);
    }
    let encoded_size = raw
        .identifiers
        .iter()
        .try_fold(0usize, |n, s| n.checked_add(s.len()))
        .ok_or(PolicyError::Invalid)?;
    if raw.identifiers.len() > 64
        || encoded_size > 16 * 1024
        || raw
            .identifiers
            .iter()
            .any(|s| s.is_empty() || s.len() > 256 || s.chars().any(char::is_control))
        || raw
            .identifiers
            .iter()
            .enumerate()
            .any(|(i, s)| raw.identifiers[..i].contains(s))
    {
        return Err(PolicyError::Invalid);
    }
    let detector = match raw.detector {
        Some(detector) => {
            let id = detector.gadget.as_bytes();
            let function = detector.function.as_bytes();
            if id.is_empty()
                || id.len() > 64
                || !id[0].is_ascii_alphanumeric()
                || !id
                    .iter()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(*b, b'_' | b'-'))
                || function.is_empty()
                || function.len() > 64
                || !(function[0].is_ascii_alphabetic() || function[0] == b'_')
                || !function
                    .iter()
                    .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
            {
                return Err(PolicyError::Invalid);
            }
            Some(DetectorPolicy {
                component: pin(&detector.component_sha256)?,
                manifest: pin(&detector.manifest_sha256)?,
                gadget: detector.gadget,
                function: detector.function,
                literals: raw.identifiers.into_iter().map(Zeroizing::new).collect(),
                file_rules: file_data::validate_rules(raw.file_data)?,
            })
        }
        None => {
            if raw.enabled || !raw.identifiers.is_empty() || !raw.file_data.is_empty() {
                return Err(PolicyError::Invalid);
            }
            None
        }
    };
    if raw.enabled {
        Ok(Policy::Proposed(detector.ok_or(PolicyError::Invalid)?))
    } else {
        Ok(Policy::Disabled)
    }
}

/// Called inside the actual user-file loader, before any project merge. It
/// never returns enabled authority; malformed or enabled source fails fixed.
pub(crate) fn reject_unavailable_user_source(bytes: &[u8]) -> Result<()> {
    match parse_user_source(bytes)? {
        Policy::Proposed(_) => Err(PolicyError::NotYetAvailable),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn enabled() -> Vec<u8> {
        br#"{"privacy":{"enabled":true,"detector":{"gadget":"privacy-detector","function":"detect","component_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","manifest_sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},"identifiers":["owned identifier"]}}"#.to_vec()
    }
    #[test]
    fn actual_user_policy_absence_disabled_and_fixed_enabled_refusal() {
        for raw in [b"{}".as_slice(), b"{\"privacy\":{\"enabled\":false}}"] {
            assert!(reject_unavailable_user_source(raw).is_ok());
        }
        let enabled = br#"{"privacy":{"enabled":true,"detector":{"gadget":"privacy-detector","function":"detect","component_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","manifest_sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},"identifiers":["owned identifier"]}}"#;
        assert_eq!(
            reject_unavailable_user_source(enabled),
            Err(PolicyError::NotYetAvailable)
        );
        for raw in [
            br#"{"privacy":null}"#.as_slice(),
            br#"{"privacy":{}}"#,
            br#"{"privacy":{"enabled":"false"}}"#,
            br#"{"privacy":{"enabled":false,"capabilities":[]}}"#,
            br#"{"privacy":{"enabled":false,"enabled":false}}"#,
            br#"{"privacy":{"enabled":false},"privacy":{"enabled":false}}"#,
            br#"{"privacy":{"enabled":false,"identifiers":["x"]}}"#,
        ] {
            assert_eq!(
                reject_unavailable_user_source(raw),
                Err(PolicyError::Invalid)
            );
        }
    }

    #[test]
    fn detector_name_function_pins_literal_and_duplicate_bounds_are_strict() {
        let template = enabled();
        for (path, replacement) in [
            ("gadget", serde_json::Value::String("../project".into())),
            ("function", serde_json::Value::String("code()".into())),
            (
                "component_sha256",
                serde_json::Value::String("a".repeat(63)),
            ),
            ("manifest_sha256", serde_json::Value::String("G".repeat(64))),
        ] {
            let mut value: serde_json::Value = serde_json::from_slice(&template).unwrap();
            value["privacy"]["detector"][path] = replacement;
            assert!(matches!(
                parse_user_source(&serde_json::to_vec(&value).unwrap()),
                Err(PolicyError::Invalid)
            ));
        }
        for literals in [
            vec!["same".to_owned(), "same".to_owned()],
            vec!["x".repeat(257)],
            vec!["bad\nvalue".to_owned()],
            (0..65).map(|i| i.to_string()).collect(),
        ] {
            let mut value: serde_json::Value = serde_json::from_slice(&template).unwrap();
            value["privacy"]["identifiers"] = serde_json::to_value(literals).unwrap();
            assert!(matches!(
                parse_user_source(&serde_json::to_vec(&value).unwrap()),
                Err(PolicyError::Invalid)
            ));
        }
        assert!(matches!(
            parse_user_source(&vec![b' '; USER_CONFIG_BYTES + 1]),
            Err(PolicyError::Invalid)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn real_user_source_owner_survives_project_config_values_but_refuses_file_changes() {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let path = directory.path().canonicalize().unwrap();
        let root = ConfigRoot::capture_owned_test_root(&path).unwrap();
        let absent = CapturedPolicy::capture(root).unwrap();
        assert!(absent.require_available().is_ok());
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path.join("config.json"))
            .unwrap();
        file.write_all(&enabled()).unwrap();
        file.sync_all().unwrap();
        assert!(matches!(absent.recheck(), Err(PolicyError::Source)));
        let captured =
            CapturedPolicy::capture(ConfigRoot::capture_owned_test_root(&path).unwrap()).unwrap();
        assert_eq!(
            captured.require_available(),
            Err(PolicyError::NotYetAvailable)
        );
        let mut merged = crate::config::Config::default();
        merged.extra.insert(
            "privacy".into(),
            serde_json::json!({"enabled":true,"detector":{"gadget":"project-shadow"}}),
        );
        assert_eq!(
            captured.proposed_detector().unwrap().unwrap().gadget(),
            "privacy-detector"
        );
        assert_eq!(
            captured
                .proposed_detector()
                .unwrap()
                .unwrap()
                .component_pin(),
            &[0xaa; 32]
        );
        assert!(!format!("{captured:?}").contains("owned identifier"));
        file.write_all(b" ").unwrap();
        file.sync_all().unwrap();
        assert!(matches!(captured.recheck(), Err(PolicyError::Source)));
    }

    const HELPER: &str = "privacy_policy::tests::marked_actual_user_policy_load";
    #[test]
    #[ignore = "owned actual user-policy loader; exact private test selector required"]
    fn marked_actual_user_policy_load() {
        let args: Vec<_> = std::env::args_os().collect();
        if args
            .iter()
            .filter(|arg| arg.as_os_str() == std::ffi::OsStr::new(HELPER))
            .count()
            != 1
            || !args.iter().any(|arg| arg == "--exact")
            || !args.iter().any(|arg| arg == "--ignored")
        {
            return;
        }
        // Only after that selector may this helper read its owned environment.
        match crate::config::Config::load() {
            Err(error) => assert_eq!(
                error.to_string(),
                "Enabled privacy is not yet available in this version."
            ),
            Ok(_) => panic!("selected owned privacy policy was not refused"),
        }
        println!("OWNED-USER-POLICY:REFUSED-BEFORE-STARTUP");
    }
    #[test]
    fn actual_selected_user_policy_loader_refuses_enabled_before_frontend_effects() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("config.json"), enabled()).unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .env_clear()
            .env("CRAFTSMAN_CONFIG_DIR", directory.path())
            .args(["--ignored", "--exact", HELPER, "--nocapture"])
            .stdin(std::process::Stdio::null());
        #[cfg(windows)]
        if let Some(system) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system);
        }
        let output = command.output().unwrap();
        assert!(output.status.success());
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .contains("OWNED-USER-POLICY:REFUSED-BEFORE-STARTUP")
        );
        assert!(output.stderr.is_empty());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
