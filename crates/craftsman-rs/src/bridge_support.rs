//! Shared ACP-bridge proxy authentication and host disk facts.
//! These helpers retain the former server's behavior without its chat protocol.

use axum::http::HeaderMap;

pub(crate) const PROXY_HEADER: &str = "x-craftsman-workspace-proxy";
pub(crate) const PROXY_SECRET_ENV: &str = "CRAFTSMAN_WORKSPACE_PROXY_SECRET";

/// True when the inbound connection carries the platform proxy secret. With
/// `CRAFTSMAN_WORKSPACE_PROXY_SECRET` unset (local dev) auth is disabled — the
/// NetworkPolicy is then the only gate. In the pod the env is always set, so
/// a direct dial without the header is rejected.
pub(crate) fn authorized(headers: &HeaderMap) -> bool {
    authorized_from_secret(headers, std::env::var_os(PROXY_SECRET_ENV))
}

fn authorized_from_secret(headers: &HeaderMap, secret: Option<std::ffi::OsString>) -> bool {
    let Some(secret) = secret else {
        return true;
    };
    let secret = secret.to_string_lossy().into_owned();
    if secret.is_empty() {
        return true;
    }
    let Some(presented) = headers.get(PROXY_HEADER).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    constant_time_eq(presented.as_bytes(), secret.as_bytes())
}

/// Constant-time byte compare (length still leaks).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub(crate) fn home_dir() -> std::path::PathBuf {
    dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// (used, total) bytes of the filesystem containing `path` — generic
/// capacity/headroom facts (on hostPath-style k8s storage this is the NODE
/// disk, which is why `/stats` also reports the `home` walk).
pub(crate) fn disk_usage(path: &std::path::Path) -> (u64, u64) {
    crate::platform_os::disk_usage(path)
}

/// Bytes actually stored under `root`: iterative walk summing regular-file
/// sizes, never following symlinks (they contribute nothing and can't cause
/// cycles or escape `root`). Per-entry errors are skipped; a missing or
/// unreadable root yields 0.
pub(crate) fn dir_used_bytes(root: &std::path::Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            // `DirEntry::metadata` does not traverse symlinks, so a link is
            // neither a dir nor a regular file here beyond itself.
            if meta.is_dir() && !meta.is_symlink() {
                stack.push(entry.path());
            } else if meta.is_file() {
                total += meta.len();
            }
        }
    }
    total
}

#[cfg(test)]
pub(crate) fn proxy_environment_fixture_for_tests(secret_present: bool) {
    let mut headers = HeaderMap::new();
    assert_eq!(authorized(&headers), !secret_present);
    headers.insert(PROXY_HEADER, "nope".parse().unwrap());
    assert_eq!(authorized(&headers), !secret_present);
    headers.insert(PROXY_HEADER, "s3cret".parse().unwrap());
    assert!(authorized(&headers));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_matches_semantics() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secre"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn dir_walk_counts_files_and_skips_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), b"12345").unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("sub/b.txt"), b"123").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(tmp.path().join("a.txt"), tmp.path().join("link")).unwrap();
        assert_eq!(dir_used_bytes(tmp.path()), 8);
        assert_eq!(dir_used_bytes(&tmp.path().join("missing")), 0);
    }

    #[test]
    fn supplied_secret_auth_preserves_empty_header_lossy_and_constant_time_behavior() {
        let mut headers = HeaderMap::new();
        assert!(authorized_from_secret(&headers, None));
        assert!(authorized_from_secret(&headers, Some("".into())));
        assert!(!authorized_from_secret(&headers, Some("s3cret".into())));
        headers.insert(PROXY_HEADER, "wrong".parse().unwrap());
        assert!(!authorized_from_secret(&headers, Some("s3cret".into())));
        headers.insert(PROXY_HEADER, "s3cret".parse().unwrap());
        assert!(authorized_from_secret(&headers, Some("s3cret".into())));
        headers.insert(
            PROXY_HEADER,
            axum::http::HeaderValue::from_bytes(&[255]).unwrap(),
        );
        assert!(!authorized_from_secret(&headers, Some("s3cret".into())));
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            assert!(!authorized_from_secret(
                &headers,
                Some(std::ffi::OsString::from_vec(vec![255]))
            ));
        }
    }
}
