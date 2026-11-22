//! Trusted user-source policy only. Enabled privacy is unavailable until a
//! separate reviewed activation; project/merged extra values mint no authority.
use crate::privacy_key::{ConfigRoot, root::FileLease};
use serde::{
    Deserialize,
    de::{self, MapAccess, Visitor},
};
use std::fmt;
use zeroize::Zeroizing;

const POLICY_BYTES: usize = 32 * 1024;
const USER_CONFIG_BYTES: usize = 1024 * 1024;

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
impl fmt::Debug for CapturedPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CapturedPrivacyPolicy([private])")
    }
}
impl CapturedPolicy {
    pub fn capture_actual() -> Result<Self> {
        Self::capture(ConfigRoot::capture_actual().map_err(|_| PolicyError::Source)?)
    }
    fn capture(root: ConfigRoot) -> Result<Self> {
        let read = root.read_user_config().map_err(|_| PolicyError::Source)?;
        let (policy, source) = match read {
            Some((bytes, source)) => (parse_user_source(&bytes)?, Some(source)),
            None => (Policy::Absent, None),
        };
        let captured = Self {
            policy,
            root,
            source,
        };
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
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct RawPolicy {
    enabled: bool,
    #[serde(default)]
    detector: Option<RawDetector>,
    #[serde(default)]
    identifiers: Vec<String>,
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
            })
        }
        None => {
            if raw.enabled || !raw.identifiers.is_empty() {
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
