#![cfg(target_os = "linux")]

use std::{ffi::OsString, path::PathBuf};

pub(crate) fn resolve_command(name: &str, fallbacks: &[&str]) -> PathBuf {
    resolve_command_with_path(name, std::env::var_os("PATH"), fallbacks)
}

fn resolve_command_with_path(name: &str, path: Option<OsString>, fallbacks: &[&str]) -> PathBuf {
    if let Some(path) = path {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return candidate;
            }
        }
    }

    for fallback in fallbacks {
        let candidate = PathBuf::from(fallback);
        if candidate.is_file() {
            return candidate;
        }
    }

    fallbacks
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_command_prefers_path_candidate() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let tool = bin.join("ps");
        std::fs::write(&tool, "").unwrap();
        let path = std::env::join_paths([bin]).unwrap();

        let resolved = resolve_command_with_path("ps", Some(path), &["/usr/bin/ps", "/bin/ps"]);

        assert_eq!(resolved, tool);
    }

    #[test]
    fn resolve_command_uses_first_fallback_when_no_candidate_exists() {
        let resolved = resolve_command_with_path(
            "definitely-not-a-lucarne-tool",
            None,
            &["/definitely/not/present/tool", "/also/not/present/tool"],
        );

        assert_eq!(resolved, PathBuf::from("/definitely/not/present/tool"));
    }
}
