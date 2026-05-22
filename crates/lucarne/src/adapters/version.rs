//! Binary version probe + minimum-version gate.
//!
//! Port of `lucarne/pkg/adapter/version.go` — each adapter we care about has
//! a published minimum CLI version, and [`probe_version`] runs
//! `<binary> --version` to enforce it before a session is started. A
//! friendly "please upgrade" message beats a mystery JSON-RPC handshake
//! failure.
//!
//! Usage from an adapter factory:
//!
//! ```text
//! ProtocolOptions {
//!     probe: Some(Arc::new({
//!         let bin = binary.clone();
//!         move || probe_version("codex", &bin, Some("0.100.0"))
//!     })),
//!     ..
//! }
//! ```
//!
//! Returns a populated [`ProbeResult`] with `available = false` when the
//! binary can't be run or is below the minimum.

use crate::{
    adapter::ProbeResult,
    adapters::{merged_env_map, resolve_command_for_launch},
};
use std::{collections::BTreeMap, process::Command};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Semver {
    major: u32,
    minor: u32,
    patch: u32,
}

fn parse_semver(raw: &str) -> Result<Semver, String> {
    let bytes = raw.as_bytes();
    for start in 0..bytes.len() {
        let mut cursor = start;
        if bytes[cursor] == b'v' {
            cursor += 1;
        }
        let Some((major, after_major)) = parse_version_component(bytes, cursor) else {
            continue;
        };
        if bytes.get(after_major) != Some(&b'.') {
            continue;
        }
        let Some((minor, after_minor)) = parse_version_component(bytes, after_major + 1) else {
            continue;
        };
        if bytes.get(after_minor) != Some(&b'.') {
            continue;
        }
        let Some((patch, _)) = parse_version_component(bytes, after_minor + 1) else {
            continue;
        };
        return Ok(Semver {
            major,
            minor,
            patch,
        });
    }
    Err(format!("cannot parse version {:?}", raw))
}

fn parse_version_component(bytes: &[u8], start: usize) -> Option<(u32, usize)> {
    let mut cursor = start;
    let mut value = 0u32;
    let mut overflowed = false;
    while let Some(byte) = bytes.get(cursor).copied().filter(u8::is_ascii_digit) {
        if !overflowed {
            let digit = u32::from(byte - b'0');
            if let Some(next) = value
                .checked_mul(10)
                .and_then(|value| value.checked_add(digit))
            {
                value = next;
            } else {
                overflowed = true;
            }
        }
        cursor += 1;
    }
    (cursor > start).then_some((if overflowed { 0 } else { value }, cursor))
}

fn less_than(a: Semver, b: Semver) -> bool {
    (a.major, a.minor, a.patch) < (b.major, b.minor, b.patch)
}

/// Returns `Err(msg)` when the detected version falls below the provider's
/// declared minimum; returns `Ok(())` if the provider has no minimum or
/// the version is new enough.
fn check_min_version(
    provider_id: &str,
    detected: &str,
    min_version: Option<&str>,
) -> Result<(), String> {
    let Some(min_raw) = min_version else {
        return Ok(());
    };
    let min = parse_semver(min_raw).map_err(|e| {
        format!(
            "invalid minimum version {:?} for {}: {}",
            min_raw, provider_id, e
        )
    })?;
    let got = parse_semver(detected).map_err(|e| {
        format!(
            "cannot parse detected {} version {:?}: {}",
            provider_id, detected, e
        )
    })?;
    if less_than(got, min) {
        return Err(format!(
            "{} version {} is below minimum required {} — please upgrade",
            provider_id, detected, min_raw
        ));
    }
    Ok(())
}

/// Synchronously runs `<bin> --version`, captures combined stdout+stderr,
/// and validates the result against [`check_min_version`].
///
/// Kept sync and dependency-free because the [`ProbeFn`] type is
/// `Fn() -> ProbeResult` — adapters invoke this on their own thread / in
/// a blocking task as needed.
pub fn probe_version(provider_id: &str, bin: &str, min_version: Option<&str>) -> ProbeResult {
    let mut result = ProbeResult {
        available: false,
        version: String::new(),
        path: bin.to_string(),
        error: String::new(),
    };

    let output = run_version_command(bin);
    match output {
        Ok(combined) => {
            // Keep the raw `<bin> --version` text for diagnostics, even
            // when the min-version gate subsequently fails.
            result.version = combined.clone();
            if let Err(e) = check_min_version(provider_id, &combined, min_version) {
                result.error = e;
                return result;
            }
            result.available = true;
        }
        Err(e) => {
            result.error = e;
        }
    }
    result
}

fn run_version_command(bin: &str) -> Result<String, String> {
    let resolved = resolve_version_binary(bin)?;
    let output = Command::new(&resolved)
        .arg("--version")
        .output()
        .map_err(|e| format!("{}", e))?;

    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        return Err(combined.trim().to_string());
    }
    Ok(combined)
}

fn resolve_version_binary(bin: &str) -> Result<String, String> {
    let env = merged_env_map(&BTreeMap::new());
    let cwd = std::env::current_dir().map_err(|err| format!("cwd: {err}"))?;
    resolve_command_for_launch(bin, &cwd.to_string_lossy(), &env).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_order() {
        assert_eq!(
            parse_semver("claude 2.1.3\n").unwrap(),
            Semver {
                major: 2,
                minor: 1,
                patch: 3
            }
        );
        assert_eq!(
            parse_semver("v0.100.1").unwrap(),
            Semver {
                major: 0,
                minor: 100,
                patch: 1
            }
        );
        assert_eq!(
            parse_semver("codex-cli xv12.34.56-beta").unwrap(),
            Semver {
                major: 12,
                minor: 34,
                patch: 56
            }
        );
        assert!(parse_semver("version 1.2").is_err());
        assert!(less_than(
            parse_semver("1.0.0").unwrap(),
            parse_semver("1.0.1").unwrap()
        ));
        assert!(less_than(
            parse_semver("0.99.99").unwrap(),
            parse_semver("0.100.0").unwrap()
        ));
        assert!(!less_than(
            parse_semver("2.0.0").unwrap(),
            parse_semver("2.0.0").unwrap()
        ));
    }

    #[test]
    #[cfg(windows)]
    fn probe_version_prefers_pathext_script_over_extensionless_shim() {
        let root = tempfile::tempdir().expect("tempdir");
        let extensionless = root.path().join("mycli");
        std::fs::write(
            &extensionless,
            b"shell shim without executable extension\r\n",
        )
        .expect("write extensionless shim");
        let cmd = root.path().join("mycli.cmd");
        std::fs::write(&cmd, b"@echo off\r\necho mycli 1.2.3\r\n").expect("write cmd shim");
        let stem = root.path().join("mycli");

        let probe = probe_version("mycli", &stem.to_string_lossy(), Some("1.0.0"));

        assert!(probe.available, "probe failed: {}", probe.error);
        assert!(probe.version.contains("1.2.3"));
    }

    #[test]
    fn min_version_gate() {
        assert!(check_min_version("claude", "2.1.112", Some("2.1.112")).is_ok());
        assert!(check_min_version("claude", "2.1.37", Some("2.1.112")).is_err());
        assert!(check_min_version("codex", "0.99.5", Some("0.100.0")).is_err());
        assert!(check_min_version("codex", "0.100.0", Some("0.100.0")).is_ok());
        assert!(check_min_version("copilot", "1.0.0", Some("1.0.0")).is_ok());
        assert!(check_min_version("gemini", "garbage", None).is_ok());
    }

    #[test]
    fn unparseable_detected() {
        let e = check_min_version("claude", "nope", Some("2.1.112")).unwrap_err();
        assert!(e.contains("cannot parse detected"));
    }

    #[test]
    fn version_probe_does_not_spawn_threads() {
        let source = include_str!("version.rs");

        let spawn_pattern = ["thread", "::spawn"].concat();
        let std_thread_pattern = ["std", "::thread"].concat();

        assert!(!source.contains(&spawn_pattern));
        assert!(!source.contains(&std_thread_pattern));
    }
}
