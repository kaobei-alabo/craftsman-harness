//! Platform skill sync engine.
//!
//! Pulls skill bundles from the platform's published skills REST endpoints
//! (`GET /apps/skills/api/skill/` + `GET /apps/skills/api/skill/{id}/download`)
//! and manages them under the shared `~/.craftsman/skills/<id>/` dir with a
//! per-skill `.craftsman-skill.json` marker (the legacy layout, unchanged, so
//! existing installs stay visible).
//! The install/scan/status core takes an explicit root so it's unit-testable
//! without touching the real `~/.craftsman` or the network.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

pub const MARKER_NAME: &str = ".craftsman-skill.json";

fn validate_skill_id(skill_id: &str) -> anyhow::Result<&str> {
    anyhow::ensure!(!skill_id.is_empty(), "skill id cannot be empty");
    anyhow::ensure!(
        skill_id != "." && skill_id != ".." && !skill_id.contains('/') && !skill_id.contains('\\'),
        "skill id must be a single directory name"
    );
    Ok(skill_id)
}

/// One row of the platform catalog (`list_skill`).
#[derive(Debug, Clone)]
pub struct PlatformSkill {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub source: String,
}

/// The `.craftsman-skill.json` marker written next to an installed bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledMarker {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default)]
    pub installed_at: String,
}

fn default_source() -> String {
    "platform".into()
}

/// Install state of one skill id, from the installed ⟕⟖ platform outer join.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillState {
    UpToDate,
    UpdateAvailable,
    NotInstalled,
    LocalOnly,
}

#[derive(Debug, Clone)]
pub struct SkillStatus {
    pub id: String,
    pub name: String,
    pub description: String,
    pub installed_version: Option<String>,
    pub platform_version: Option<String>,
    pub state: SkillState,
    pub source: String,
}

/// Scan `root` for our markers. Malformed/partial markers and dot-dirs
/// (staging areas) are skipped — never raises.
pub fn scan_markers(root: &Path) -> BTreeMap<String, InstalledMarker> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for child in entries.filter_map(|e| e.ok()) {
        let name = child.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue; // staging/.<id>.incoming etc.
        }
        let marker_path = child.path().join(MARKER_NAME);
        let Ok(text) = std::fs::read_to_string(&marker_path) else {
            continue;
        };
        let Ok(mut marker) = serde_json::from_str::<InstalledMarker>(&text) else {
            continue;
        };
        if marker.name.is_empty() {
            marker.name = marker.id.clone();
        }
        out.insert(marker.id.clone(), marker);
    }
    out
}

/// Installed platform skills in the shared `~/.craftsman/skills` dir.
pub fn installed_platform_skills() -> BTreeMap<String, InstalledMarker> {
    scan_markers(&crate::config::skills_dir())
}

/// Outer-join installed markers with the platform catalog. Version comparison
/// is exact string inequality (not semver) — a bumped platform version always
/// reads as an update. Sorted by id.
pub fn compute_status(
    installed: &BTreeMap<String, InstalledMarker>,
    platform: &[PlatformSkill],
) -> Vec<SkillStatus> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for s in platform {
        seen.insert(s.id.clone());
        let m = installed.get(&s.id);
        let state = match m {
            None => SkillState::NotInstalled,
            Some(m) if m.version == s.version => SkillState::UpToDate,
            Some(_) => SkillState::UpdateAvailable,
        };
        out.push(SkillStatus {
            id: s.id.clone(),
            name: s.name.clone(),
            description: s.description.clone(),
            installed_version: m.map(|m| m.version.clone()),
            platform_version: Some(s.version.clone()),
            state,
            source: s.source.clone(),
        });
    }
    for (sid, m) in installed {
        if seen.contains(sid) {
            continue;
        }
        out.push(SkillStatus {
            id: sid.clone(),
            name: m.name.clone(),
            description: String::new(),
            installed_version: Some(m.version.clone()),
            platform_version: None,
            state: SkillState::LocalOnly,
            source: "local".into(),
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Async adapter over the platform skills REST endpoints
/// (`docs/platform-token-refactor.md` §4.3 — the token-era transport).
pub struct SkillsClient {
    rest: crate::platform_rest::PlatformRest,
}

impl SkillsClient {
    /// Connect to the platform skills catalog. Errors when the platform
    /// block isn't configured.
    pub async fn connect(cfg: &crate::config::Config) -> anyhow::Result<SkillsClient> {
        anyhow::ensure!(
            cfg.platform_configured(),
            "configure platform.base_url + platform.api_token to reach platform skills"
        );
        Ok(SkillsClient {
            rest: crate::platform_rest::PlatformRest::from_config(cfg)
                .expect("platform_configured guarantees a client"),
        })
    }

    /// The platform catalog (`list_skill`).
    pub async fn list(&self) -> anyhow::Result<Vec<PlatformSkill>> {
        let resp = self.rest.list_skills().await?;
        let rows = match resp.get("items").and_then(|v| v.as_array()) {
            Some(items) => items.clone(),
            None => resp.as_array().cloned().unwrap_or_default(),
        };
        Ok(rows
            .iter()
            .filter_map(|r| {
                let id = r.get("id")?.as_str()?.to_string();
                Some(PlatformSkill {
                    name: r["name"].as_str().unwrap_or(&id).to_string(),
                    version: r["version"].as_str().unwrap_or_default().to_string(),
                    description: r["description"].as_str().unwrap_or_default().to_string(),
                    source: r["source"].as_str().unwrap_or_default().to_string(),
                    id,
                })
            })
            .collect())
    }

    /// Bundle download info (`skill_download`): `{url, sha256, version, name}`.
    pub async fn download_info(&self, skill_id: &str) -> anyhow::Result<Value> {
        self.rest.skill_download(skill_id).await
    }
}

/// Pull + (re)install a skill by id into the shared skills dir. Used for both
/// first install and update — extraction overwrites atomically.
pub async fn install_skill(
    client: &SkillsClient,
    skill_id: &str,
) -> anyhow::Result<InstalledMarker> {
    let info = client.download_info(skill_id).await?;
    let url = info["url"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("skill_download response missing url"))?;
    let blob = reqwest::Client::new()
        .get(url)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let root = crate::config::skills_dir();
    std::fs::create_dir_all(&root)?;
    install_from_bytes(&root, &info, skill_id, &blob)
}

// Update is install with overwrite semantics — same code path.
pub use install_skill as update_skill;

/// Verify sha256 and atomically install the tar.gz `blob` under
/// `root/<skill_id>/`. The bundle is extracted into a dot-prefixed staging dir
/// and verified BEFORE the live dir is touched, so a bad download never
/// clobbers a good install.
pub fn install_from_bytes(
    root: &Path,
    info: &Value,
    skill_id: &str,
    blob: &[u8],
) -> anyhow::Result<InstalledMarker> {
    validate_skill_id(skill_id)?;
    let expected = info["sha256"].as_str().unwrap_or("");
    let actual = hex_digest(blob);
    if !expected.is_empty() && actual != expected {
        anyhow::bail!("sha256 mismatch for {skill_id}: expected {expected}, got {actual}");
    }

    let staging = root.join(format!(".{skill_id}.incoming"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;
    // `unpack` refuses entries that would escape the destination — the same
    // guarantee tarfile's `filter="data"` gives.
    let tar = flate2::read::GzDecoder::new(std::io::Cursor::new(blob));
    tar::Archive::new(tar).unpack(&staging)?;

    // The archive root is usually a single `<skill_id>/` dir; fall back to
    // the staging root itself for a flat bundle.
    let dirs: Vec<std::path::PathBuf> = std::fs::read_dir(&staging)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    let source = if dirs.len() == 1 {
        dirs[0].clone()
    } else {
        staging.clone()
    };

    let marker = InstalledMarker {
        id: skill_id.to_string(),
        name: info["name"].as_str().unwrap_or(skill_id).to_string(),
        version: info["version"].as_str().unwrap_or_default().to_string(),
        sha256: actual,
        source: "platform".into(),
        installed_at: now_utc_stamp(),
    };
    std::fs::write(
        source.join(MARKER_NAME),
        serde_json::to_string_pretty(&marker)? + "\n",
    )?;

    // Swap into place last (same filesystem: both under `root`).
    let dest = root.join(skill_id);
    if dest.exists() {
        std::fs::remove_dir_all(&dest)?;
    }
    std::fs::rename(&source, &dest)?;
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    Ok(marker)
}

/// Remove a platform-installed skill from `root`. Refuses to delete a dir
/// that isn't ours (no marker / `source != "platform"`). Returns whether it
/// was removed.
pub fn remove_skill_in(root: &Path, skill_id: &str) -> bool {
    if validate_skill_id(skill_id).is_err() {
        return false;
    }
    let dest = root.join(skill_id);
    let Ok(text) = std::fs::read_to_string(dest.join(MARKER_NAME)) else {
        return false;
    };
    let Ok(marker) = serde_json::from_str::<InstalledMarker>(&text) else {
        return false;
    };
    if marker.source != "platform" {
        return false;
    }
    std::fs::remove_dir_all(&dest).is_ok()
}

/// [`remove_skill_in`] against the shared `~/.craftsman/skills` dir.
pub fn remove_skill(skill_id: &str) -> bool {
    remove_skill_in(&crate::config::skills_dir(), skill_id)
}

fn hex_digest(blob: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(blob);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `2026-07-20T12:34:56Z` without a chrono dep: seconds since epoch through a
/// civil-date conversion (days_from_civil inverse, Howard Hinnant's algorithm).
fn now_utc_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // civil_from_days for the Unix epoch (era-based, valid for all our dates).
    let z = days as i64 + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn platform(id: &str, version: &str) -> PlatformSkill {
        PlatformSkill {
            id: id.into(),
            name: id.into(),
            version: version.into(),
            description: String::new(),
            source: "platform".into(),
        }
    }

    fn marker(id: &str, version: &str) -> InstalledMarker {
        InstalledMarker {
            id: id.into(),
            name: id.into(),
            version: version.into(),
            sha256: String::new(),
            source: "platform".into(),
            installed_at: String::new(),
        }
    }

    /// A gzipped tar with one `<id>/SKILL.md` entry.
    fn bundle(id: &str) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        let body = b"---\nname: test\n---\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, format!("{id}/SKILL.md"), &body[..])
            .unwrap();
        let raw = tar.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&raw).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn compute_status_outer_joins_installed_and_platform() {
        let mut installed = BTreeMap::new();
        installed.insert("a".to_string(), marker("a", "1"));
        installed.insert("b".to_string(), marker("b", "1"));
        installed.insert("z-local".to_string(), marker("z-local", "0"));
        let platform = vec![platform("a", "1"), platform("b", "2"), platform("c", "1")];
        let statuses = compute_status(&installed, &platform);
        let by_id: BTreeMap<_, _> = statuses.iter().map(|s| (s.id.as_str(), s.state)).collect();
        assert_eq!(by_id["a"], SkillState::UpToDate);
        assert_eq!(by_id["b"], SkillState::UpdateAvailable);
        assert_eq!(by_id["c"], SkillState::NotInstalled);
        assert_eq!(by_id["z-local"], SkillState::LocalOnly);
        // Sorted by id.
        let ids: Vec<_> = statuses.iter().map(|s| s.id.clone()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn install_verifies_sha_extracts_and_swaps() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let blob = bundle("web-search");
        let sha = hex_digest(&blob);
        let info = json!({"url": "x", "sha256": sha, "version": "1.0", "name": "Web Search"});
        let m = install_from_bytes(root, &info, "web-search", &blob).unwrap();
        assert_eq!(m.version, "1.0");
        assert!(root.join("web-search").join("SKILL.md").is_file());
        // The marker landed and the scanner sees it.
        let scanned = scan_markers(root);
        assert_eq!(scanned["web-search"].name, "Web Search");
        assert!(
            !root.join(".web-search.incoming").exists(),
            "staging cleaned"
        );

        // Update (reinstall) overwrites in place.
        let info2 = json!({"url": "x", "sha256": hex_digest(&blob), "version": "2.0"});
        install_from_bytes(root, &info2, "web-search", &blob).unwrap();
        assert_eq!(scan_markers(root)["web-search"].version, "2.0");
    }

    #[test]
    fn install_rejects_bad_sha_without_touching_live_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let blob = bundle("s");
        let info = json!({"url": "x", "sha256": "deadbeef", "version": "1"});
        assert!(install_from_bytes(root, &info, "s", &blob).is_err());
        assert!(!root.join("s").exists());
    }

    #[test]
    fn install_and_remove_reject_path_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let blob = bundle("safe");
        let info = json!({"url": "x", "sha256": hex_digest(&blob), "version": "1"});
        assert!(install_from_bytes(root, &info, "../escape", &blob).is_err());
        assert!(!remove_skill_in(root, "../escape"));
        assert!(!tmp.path().parent().unwrap().join("escape").exists());
    }

    #[test]
    fn remove_refuses_foreign_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // No marker: refuse.
        std::fs::create_dir_all(root.join("mine")).unwrap();
        assert!(!remove_skill_in(root, "mine"));
        assert!(root.join("mine").exists());
        // Non-platform source: refuse.
        std::fs::write(
            root.join("mine").join(MARKER_NAME),
            r#"{"id":"mine","version":"1","source":"local"}"#,
        )
        .unwrap();
        assert!(!remove_skill_in(root, "mine"));
        // Platform-installed: removed.
        let blob = bundle("ours");
        let info = json!({"url": "x", "sha256": hex_digest(&blob), "version": "1"});
        install_from_bytes(root, &info, "ours", &blob).unwrap();
        assert!(remove_skill_in(root, "ours"));
        assert!(!root.join("ours").exists());
    }

    #[test]
    fn scanner_skips_dot_dirs_and_malformed_markers() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".x.incoming")).unwrap();
        std::fs::write(root.join(".x.incoming").join(MARKER_NAME), "{}").unwrap();
        std::fs::create_dir_all(root.join("bad")).unwrap();
        std::fs::write(root.join("bad").join(MARKER_NAME), "not json").unwrap();
        std::fs::create_dir_all(root.join("nomarker")).unwrap();
        assert!(scan_markers(root).is_empty());
    }

    #[test]
    fn utc_stamp_shape() {
        let s = now_utc_stamp();
        assert_eq!(s.len(), 20, "{s}");
        assert!(s.ends_with('Z') && s.contains('T'));
        assert!(s.starts_with("20"), "sane century: {s}");
    }
}
