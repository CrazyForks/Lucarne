//! Vendor adapters — thin factories that build the argv/env for a
//! specific agent CLI and wire the matching `Dialect` in.
//!
//! Each adapter returns a fully-assembled [`crate::adapter::ProtocolAdapter`]
//! trait object via [`crate::adapter::ProtocolAdapter`] (or a bespoke
//! implementation for adapters that need extra launch machinery).

#[cfg(feature = "copilot")]
pub mod copilot;

#[cfg(feature = "pi")]
pub mod pi;

#[cfg(feature = "gemini")]
pub mod gemini;

#[cfg(feature = "claude")]
pub mod claude;

#[cfg(feature = "codex")]
pub mod codex;

pub mod codex_prep;
pub mod version;

pub use codex_prep::{
    ensure_launch_cwd, merged_env_map, prepare_codex_start, prepare_local_cli_start,
    resolve_command_for_launch,
};
pub use version::probe_version;

pub fn default_adapters() -> Vec<std::sync::Arc<crate::adapter::ProtocolAdapter>> {
    crate::agent_registry::adapter_descriptors()
        .into_iter()
        .filter_map(|descriptor| descriptor.adapter_factory.map(|factory| factory()))
        .collect()
}

pub fn default_adapter_provider_ids() -> Vec<&'static str> {
    crate::agent_registry::adapter_descriptors()
        .into_iter()
        .map(|descriptor| descriptor.id.as_str())
        .collect()
}

pub fn default_adapters_for_provider_ids(
    enabled_ids: &[String],
) -> Vec<std::sync::Arc<crate::adapter::ProtocolAdapter>> {
    use std::collections::HashSet;

    let requested = enabled_ids
        .iter()
        .map(|id| id.as_str())
        .collect::<HashSet<_>>();
    crate::agent_registry::adapter_descriptors()
        .into_iter()
        .filter(|descriptor| requested.contains(descriptor.id.as_str()))
        .filter_map(|descriptor| descriptor.adapter_factory.map(|factory| factory()))
        .collect()
}

// ——— shared helpers used by multiple adapters ———

/// Modes for blocking a forbidden CLI flag when forwarding `--extra-args`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockedArgMode {
    /// Flag consumes a single value token (e.g. `--model gpt-5`).
    WithValue,
    /// Flag is standalone (e.g. `--yolo`).
    Standalone,
}

/// Drop any occurrences of flags listed in `blocked` from `extra_args`,
/// preserving order and honoring whether they consume a value.
///
/// Matches Go's `filterExtraArgs`: for every arg, split on `=` to get
/// the bare flag. If the flag is blocked in ANY mode, drop it. If the
/// mode is `WithValue` and there is no inline `=`, skip the *next* arg
/// too (it is the value).
pub fn filter_extra_args(extra: &[String], blocked: &[(&str, BlockedArgMode)]) -> Vec<String> {
    let mut out = Vec::with_capacity(extra.len());
    let mut skip_next = false;
    for arg in extra {
        if skip_next {
            skip_next = false;
            continue;
        }
        let (flag, has_inline_value) = match arg.find('=') {
            Some(idx) if idx > 0 => (&arg[..idx], true),
            _ => (arg.as_str(), false),
        };
        if let Some((_, mode)) = blocked.iter().find(|(n, _)| *n == flag) {
            if *mode == BlockedArgMode::WithValue && !has_inline_value {
                skip_next = true;
            }
            continue;
        }
        out.push(arg.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_strips_blocked_flags() {
        let extra = vec![
            "--model".into(),
            "gpt-5".into(),
            "--keep".into(),
            "--yolo".into(),
            "--other=value".into(),
            "--resume=abc".into(),
        ];
        let blocked = [
            ("--model", BlockedArgMode::WithValue),
            ("--yolo", BlockedArgMode::Standalone),
            ("--resume", BlockedArgMode::WithValue),
        ];
        let out = filter_extra_args(&extra, &blocked);
        assert_eq!(out, vec!["--keep".to_string(), "--other=value".to_string()]);
    }

    #[test]
    fn filter_strips_standalone_with_inline_eq() {
        // Go blocks `--yolo=anything` too when --yolo is Standalone.
        let extra = vec!["--yolo=true".into(), "--keep".into()];
        let blocked = [("--yolo", BlockedArgMode::Standalone)];
        let out = filter_extra_args(&extra, &blocked);
        assert_eq!(out, vec!["--keep".to_string()]);
    }
}
