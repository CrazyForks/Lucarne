//! Shared adapter launch-preparation hooks.
//!
//! Port of `lucarne/pkg/adapter/codex_prep.go`.
//!
//! The entry point is [`prepare_local_cli_start`] (exposed via
//! [`super::prepare_local_cli_start`]). It normalizes the
//! launch request so subprocess-backed adapters can rely on:
//!
//! 1. An absolute, existing CWD (created with `0755` if missing).
//! 2. A merged env map (process env + `extra_env`, with Windows
//!    `PATH`/`Path` aliases normalized).
//! 3. A resolved, executable binary path — either absolute or looked
//!    up via the merged `PATH`. This mirrors Go's
//!    `resolveCommandForLaunch` and gives friendly errors when the CLI
//!    isn't on disk.
//!
//! [`prepare_codex_start`] intentionally avoids rewriting `CODEX_HOME`.
//! When the caller passes an explicit `CODEX_HOME` in `extra_env`, that
//! directory is validated and otherwise left untouched.

use crate::{
    adapter::SessionParams,
    error::{LucarneError, Result},
};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};
use tracing::debug;

/// Normalize the launch request and resolve the binary to an absolute,
/// executable path. Pure port of Go's `prepareLocalCLIStart`.
pub fn prepare_local_cli_start(
    req: &SessionParams,
    binary: &str,
) -> Result<(SessionParams, String)> {
    let mut req = req.clone();
    let cwd = ensure_launch_cwd(&req.cwd)?;
    req.cwd = cwd.clone();
    req.extra_env = merged_env_map(&req.extra_env);
    let resolved = resolve_command_for_launch(binary, &cwd, &req.extra_env)?;
    debug!(
        target: "lucarne::adapters::launch_prep",
        binary,
        cwd = %cwd,
        resolved = %resolved,
        "local cli launch prepared"
    );
    Ok((req, resolved))
}

/// Codex extension of [`prepare_local_cli_start`]: validate an explicit
/// caller-provided `CODEX_HOME`, but do not inject or rewrite one.
pub fn prepare_codex_start(req: &SessionParams, binary: &str) -> Result<(SessionParams, String)> {
    let explicit_env = req.extra_env.clone();
    let (mut req, resolved) = prepare_local_cli_start(req, binary)?;
    let env = prepare_codex_env(req.extra_env.clone(), &explicit_env)?;
    req.extra_env = env;
    debug!(
        target: "lucarne::adapters::launch_prep",
        binary,
        resolved = %resolved,
        "codex launch prepared"
    );
    Ok((req, resolved))
}

// ——— CWD ————————————————————————————————————————————————————————————

pub fn ensure_launch_cwd(cwd: &str) -> Result<String> {
    let cwd = if cwd.trim().is_empty() {
        let here = std::env::current_dir()
            .map_err(|e| LucarneError::adapter(format!("resolve cwd: {}", e)))?;
        here
    } else {
        PathBuf::from(cwd)
    };
    let cwd = normalize_path(&cwd);
    fs::create_dir_all(&cwd)
        .map_err(|e| LucarneError::adapter(format!("prepare cwd {:?}: {}", cwd, e)))?;
    Ok(cwd.to_string_lossy().into_owned())
}

// ——— Env ————————————————————————————————————————————————————————————

/// Build a launch env map by taking the current process env and layering
/// caller-supplied `extra` on top. On Windows, propagate the `PATH`
/// override to `Path` so child processes see a consistent lookup chain.
pub fn merged_env_map(extra: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in std::env::vars() {
        env.insert(k, v);
    }
    for (k, v) in extra {
        env.insert(k.clone(), v.clone());
    }
    normalize_path_env_keys(&mut env, extra);
    env
}

fn normalize_path_env_keys(env: &mut BTreeMap<String, String>, extra: &BTreeMap<String, String>) {
    if !cfg!(target_os = "windows") {
        return;
    }
    if let Some(v) = extra.get("PATH") {
        env.insert("PATH".into(), v.clone());
        if !extra.contains_key("Path") {
            env.insert("Path".into(), v.clone());
        }
        return;
    }
    if let Some(v) = extra.get("Path") {
        env.insert("PATH".into(), v.clone());
        env.insert("Path".into(), v.clone());
    }
}

// ——— Command resolution ————————————————————————————————————————————

/// Resolve `command` to an absolute, executable path.
///
/// - If `command` contains a path separator it is treated as a file path
///   (joined onto `cwd` when relative) and must exist and be executable.
/// - Otherwise each directory on the merged `PATH` is tried in order.
///
/// Mirrors Go's `resolveCommandForLaunch` including its empty-element
/// fallback to `"."` (POSIX legacy — an empty entry in `$PATH` means CWD).
pub fn resolve_command_for_launch(
    command: &str,
    cwd: &str,
    env: &BTreeMap<String, String>,
) -> Result<String> {
    let command = command.trim();
    if command.is_empty() {
        return Err(LucarneError::adapter("command is empty"));
    }
    // Match Go's `strings.ContainsRune(command, filepath.Separator)` —
    // only the OS-native separator flags this as a path.
    if command_has_path_separator(command) {
        let mut candidate = PathBuf::from(command);
        if !candidate.is_absolute() {
            candidate = PathBuf::from(cwd).join(&candidate);
        }
        let mut first_error = None;
        for candidate in command_candidates(candidate, env) {
            match ensure_executable(&candidate) {
                Ok(abs) => return Ok(abs),
                Err(err) => first_error.get_or_insert(err),
            };
        }
        return Err(first_error.unwrap_or_else(|| {
            LucarneError::adapter(format!("resolve command path {:?}", command))
        }));
    }
    let path = lookup_path_env(env);
    for dir in split_path(&path) {
        let dir_str = dir.to_string_lossy();
        let dir = if dir_str.trim().is_empty() {
            OsString::from(".")
        } else {
            dir
        };
        let candidate = PathBuf::from(dir).join(command);
        for candidate in command_candidates(candidate, env) {
            if let Ok(abs) = ensure_executable(&candidate) {
                return Ok(abs);
            }
        }
    }
    Err(LucarneError::adapter(format!(
        "resolve command {:?} in PATH",
        command
    )))
}

fn command_has_path_separator(command: &str) -> bool {
    #[cfg(windows)]
    {
        command.contains('\\') || command.contains('/')
    }
    #[cfg(not(windows))]
    {
        command.contains(std::path::MAIN_SEPARATOR)
    }
}

#[cfg(windows)]
fn command_candidates(command: PathBuf, env: &BTreeMap<String, String>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if command.extension().is_none() {
        for extension in windows_path_extensions(env) {
            let mut candidate = command.as_os_str().to_os_string();
            candidate.push(extension);
            candidates.push(PathBuf::from(candidate));
        }
    }
    candidates.push(command);
    candidates
}

#[cfg(not(windows))]
fn command_candidates(command: PathBuf, _env: &BTreeMap<String, String>) -> Vec<PathBuf> {
    vec![command]
}

#[cfg(windows)]
fn windows_path_extensions(env: &BTreeMap<String, String>) -> Vec<String> {
    env.get("PATHEXT")
        .or_else(|| env.get("PathExt"))
        .map(String::as_str)
        .unwrap_or(".COM;.EXE;.BAT;.CMD")
        .split(';')
        .filter_map(|extension| {
            let extension = extension.trim();
            if extension.is_empty() {
                return None;
            }
            if extension.starts_with('.') {
                Some(extension.to_string())
            } else {
                Some(format!(".{extension}"))
            }
        })
        .collect()
}

fn lookup_path_env(env: &BTreeMap<String, String>) -> String {
    if let Some(v) = env.get("PATH") {
        if !v.trim().is_empty() {
            return v.clone();
        }
    }
    if cfg!(target_os = "windows") {
        if let Some(v) = env.get("Path") {
            return v.trim().to_string();
        }
    }
    String::new()
}

fn split_path(path: &str) -> Vec<OsString> {
    if path.is_empty() {
        return Vec::new();
    }
    let sep = if cfg!(target_os = "windows") {
        ';'
    } else {
        ':'
    };
    path.split(sep).map(OsString::from).collect()
}

fn ensure_executable(path: &Path) -> Result<String> {
    let meta = fs::metadata(path)
        .map_err(|e| LucarneError::adapter(format!("{}: {}", path.display(), e)))?;
    if meta.is_dir() {
        return Err(LucarneError::adapter(format!(
            "{:?} is not executable",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 == 0 {
            return Err(LucarneError::adapter(format!(
                "{:?} is not executable",
                path.display()
            )));
        }
    }
    // On Windows we don't check the executable bit — existence is enough.
    //
    // Match Go's `filepath.Abs(path)`: make the path absolute *without*
    // resolving symlinks. This matters when the user deliberately points
    // at a wrapper symlink in `$PATH`.
    let abs = if path.is_absolute() {
        clean_abs(path)
    } else {
        let cur =
            std::env::current_dir().map_err(|e| LucarneError::adapter(format!("cwd: {}", e)))?;
        clean_abs(&cur.join(path))
    };
    Ok(abs.to_string_lossy().into_owned())
}

fn clean_abs(p: &Path) -> PathBuf {
    // Lexical absolute cleanup — no I/O, no symlink resolution.
    normalize_path(p)
}

pub fn normalize_path(p: &Path) -> PathBuf {
    // Lightweight clean — no symlink resolution. Matches Go's
    // `filepath.Clean` semantics well enough for launch CWDs.
    let mut out = PathBuf::new();
    for comp in p.components() {
        use std::path::Component::*;
        match comp {
            CurDir => {}
            ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

// ——— Codex-specific managed CODEX_HOME ———————————————————————————————

fn prepare_codex_env(
    env: BTreeMap<String, String>,
    explicit: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    if explicit_codex_home(explicit) {
        if let Some(h) = env.get("CODEX_HOME") {
            prepare_explicit_codex_home(h)?;
        }
    }
    Ok(env)
}

fn explicit_codex_home(extra: &BTreeMap<String, String>) -> bool {
    matches!(extra.get("CODEX_HOME"), Some(v) if !v.trim().is_empty())
}

fn prepare_explicit_codex_home(path: &str) -> Result<()> {
    let path = path.trim();
    if path.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(path).map_err(|e| {
        LucarneError::adapter(format!("prepare explicit CODEX_HOME {:?}: {}", path, e))
    })?;
    let meta = fs::metadata(path).map_err(|e| {
        LucarneError::adapter(format!("stat explicit CODEX_HOME {:?}: {}", path, e))
    })?;
    if !meta.is_dir() {
        return Err(LucarneError::adapter(format!(
            "explicit CODEX_HOME {:?} is not a directory",
            path
        )));
    }
    // Probe writability via a uniquely-named tempfile so concurrent
    // sessions can't race on the same probe path. Matches Go's
    // `os.CreateTemp(path, ".lucarne-codex-home-*")`.
    let probe = tempfile::Builder::new()
        .prefix(".lucarne-codex-home-")
        .tempfile_in(path)
        .map_err(|e| {
            LucarneError::adapter(format!(
                "explicit CODEX_HOME {:?} is not writable: {}",
                path, e
            ))
        })?;
    let _ = probe.close();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn env_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn ensure_launch_cwd_creates_and_cleans() {
        let root = tempdir().unwrap();
        let nested = root.path().join("a").join("b");
        let got = ensure_launch_cwd(&nested.to_string_lossy()).unwrap();
        assert!(Path::new(&got).is_dir());
    }

    #[test]
    fn ensure_launch_cwd_empty_uses_cwd() {
        let got = ensure_launch_cwd("").unwrap();
        assert!(Path::new(&got).is_dir());
    }

    #[test]
    #[cfg(unix)]
    fn resolve_absolute_bin_requires_executable() {
        let root = tempdir().unwrap();
        let file = root.path().join("maybe-bin");
        fs::write(&file, b"#!/bin/sh\necho hi").unwrap();
        // Not +x — should fail.
        let env = env_map(&[("PATH", "/usr/bin")]);
        let err = resolve_command_for_launch(&file.to_string_lossy(), "/tmp", &env).unwrap_err();
        assert!(err.to_string().contains("not executable"));
        // +x now — OK.
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&file).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&file, perms).unwrap();
        let ok = resolve_command_for_launch(&file.to_string_lossy(), "/tmp", &env).unwrap();
        assert!(Path::new(&ok).is_absolute());
    }

    #[test]
    #[cfg(unix)]
    fn resolve_path_lookup_finds_bin() {
        let root = tempdir().unwrap();
        let bin = root.path().join("mycli");
        fs::write(&bin, b"#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&bin).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&bin, perms).unwrap();
        let env = env_map(&[("PATH", &root.path().to_string_lossy())]);
        let got = resolve_command_for_launch("mycli", "/tmp", &env).unwrap();
        assert!(Path::new(&got).ends_with("mycli"));
    }

    #[test]
    #[cfg(windows)]
    fn resolve_path_lookup_uses_pathext() {
        let root = tempdir().unwrap();
        let extensionless_shim = root.path().join("mycli");
        fs::write(
            &extensionless_shim,
            b"shell shim without executable extension\r\n",
        )
        .unwrap();
        let bin = root.path().join("mycli.cmd");
        fs::write(&bin, b"@echo off\r\n").unwrap();
        let env = env_map(&[
            ("PATH", &root.path().to_string_lossy()),
            ("PATHEXT", ".COM;.EXE;.BAT;.CMD"),
        ]);

        let got = resolve_command_for_launch("mycli", r"C:\", &env).unwrap();

        assert_eq!(
            Path::new(&got)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("mycli.cmd")
        );
    }

    #[test]
    #[cfg(windows)]
    fn resolve_path_command_uses_pathext() {
        let root = tempdir().unwrap();
        let tools = root.path().join("tools");
        fs::create_dir_all(&tools).unwrap();
        let bin = tools.join("mycli.cmd");
        fs::write(&bin, b"@echo off\r\n").unwrap();
        let env = env_map(&[("PATHEXT", ".CMD")]);

        let got = resolve_command_for_launch(r"tools\mycli", &root.path().to_string_lossy(), &env)
            .unwrap();

        assert_eq!(
            Path::new(&got)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("mycli.cmd")
        );
        assert!(Path::new(&got)
            .parent()
            .is_some_and(|path| path.ends_with("tools")));
    }

    #[test]
    #[cfg(windows)]
    fn resolve_forward_slash_path_command_uses_pathext() {
        let root = tempdir().unwrap();
        let tools = root.path().join("tools");
        fs::create_dir_all(&tools).unwrap();
        let bin = tools.join("mycli.cmd");
        fs::write(&bin, b"@echo off\r\n").unwrap();
        let env = env_map(&[("PATHEXT", ".CMD")]);

        let got = resolve_command_for_launch("tools/mycli", &root.path().to_string_lossy(), &env)
            .unwrap();

        assert_eq!(
            Path::new(&got)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("mycli.cmd")
        );
        assert!(Path::new(&got)
            .parent()
            .is_some_and(|path| path.ends_with("tools")));
    }

    #[test]
    fn resolve_empty_errors() {
        let env = env_map(&[]);
        assert!(resolve_command_for_launch("", "/tmp", &env).is_err());
    }

    #[test]
    fn merged_env_inherits_process_env_and_overlays_extra() {
        let extra = env_map(&[("LUCARNE_TEST_KEY", "from-extra")]);
        let merged = merged_env_map(&extra);
        assert_eq!(merged.get("LUCARNE_TEST_KEY").unwrap(), "from-extra");
        // HOME (or PATH) usually exists on CI — at least one var inherited.
        assert!(merged.len() > 1);
    }

    #[test]
    fn prepare_codex_env_respects_explicit() {
        let tmp = tempdir().unwrap();
        let explicit = env_map(&[("CODEX_HOME", &tmp.path().to_string_lossy())]);
        let merged = merged_env_map(&explicit);
        let out = prepare_codex_env(merged, &explicit).unwrap();
        assert_eq!(
            out.get("CODEX_HOME").unwrap(),
            &tmp.path().to_string_lossy().to_string()
        );
    }

    #[test]
    #[cfg(unix)]
    fn prepare_codex_env_without_explicit_home_does_not_inject_managed_home() {
        let home = tempdir().unwrap();
        let mut env = BTreeMap::new();
        env.insert("HOME".into(), home.path().to_string_lossy().into_owned());
        let out = prepare_codex_env(env, &BTreeMap::new()).unwrap();
        assert!(
            !out.contains_key("CODEX_HOME"),
            "managed CODEX_HOME injection must stay disabled: {out:?}"
        );
    }

    #[test]
    fn normalize_strips_curdir() {
        assert_eq!(
            normalize_path(Path::new("/a/./b/c/..")),
            PathBuf::from("/a/b")
        );
    }
}
