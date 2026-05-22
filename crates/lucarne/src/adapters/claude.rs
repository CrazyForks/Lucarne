//! Claude adapter — argv builder for `claude --input-format stream-json
//! --output-format stream-json --verbose`.

use crate::{
    adapter::{
        ArgProfile, Capabilities, ConfigSchema, Field, Protocol, ProtocolAdapter, ProtocolOptions,
        Spec,
    },
    adapters::{filter_extra_args, probe_version, BlockedArgMode},
    agent_registry::{AgentDescriptor, ALL_AGENT_DESCRIPTORS},
    dialect::PermissionMode,
    dialects::claude::{decode_claude_fork_ref, Claude},
    error::Result,
    ProviderId,
};
use linkme::distributed_slice;
use std::{path::Path, sync::Arc};
use tracing::info;

pub struct Options {
    pub binary: String,
}

fn default_adapter() -> Arc<ProtocolAdapter> {
    new(Options::default())
}

#[distributed_slice(ALL_AGENT_DESCRIPTORS)]
static DESCRIPTOR: AgentDescriptor = AgentDescriptor {
    id: ProviderId::from_static("claude"),
    order: 10,
    adapter_factory: Some(default_adapter),
};

impl Default for Options {
    fn default() -> Self {
        Self {
            binary: default_claude_binary(),
        }
    }
}

fn default_claude_binary() -> String {
    let override_bin = std::env::var("LUCARNE_CLAUDE_BIN").ok();
    let home = crate::host::paths::home_dir().map(|path| path.to_string_lossy().into_owned());
    default_claude_binary_from(override_bin.as_deref(), home.as_deref(), |path| {
        Path::new(path).exists()
    })
}

fn default_claude_binary_from<F>(
    env_override: Option<&str>,
    home: Option<&str>,
    exists: F,
) -> String
where
    F: Fn(&str) -> bool,
{
    if let Some(bin) = env_override.map(str::trim).filter(|bin| !bin.is_empty()) {
        return bin.to_string();
    }

    if let Some(home) = home.map(str::trim).filter(|home| !home.is_empty()) {
        let home = Path::new(home);
        for rel in ["Library/pnpm/claude", ".local/bin/claude"] {
            let candidate = home.join(rel).to_string_lossy().into_owned();
            if exists(&candidate) {
                return candidate;
            }
        }
    }

    "claude".into()
}

fn claude_permission_mode_arg(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Default => "default",
        PermissionMode::Write => "acceptEdits",
        PermissionMode::ReadOnly => "plan",
        PermissionMode::Auto => "auto",
        PermissionMode::Full | PermissionMode::Bypass => "bypassPermissions",
    }
}

fn detect_claude_cli_version(binary: &str) -> Option<String> {
    short_cli_version(&probe_version(DESCRIPTOR.id.as_str(), binary, Some("2.1.112")).version)
}

fn short_cli_version(raw: &str) -> Option<String> {
    raw.split_whitespace()
        .map(|part| part.trim_start_matches('v'))
        .find(|part| part.chars().next().is_some_and(|ch| ch.is_ascii_digit()))
        .map(str::to_string)
}

fn push_claude_resume_args(
    args: &mut Vec<String>,
    req: &crate::adapter::SessionParams,
    sid: String,
) {
    if let Some(fork_ref) = decode_claude_fork_ref(&sid) {
        args.push("--resume".into());
        args.push(fork_ref.source_session_id);
        args.push("--fork-session".into());
        args.push("--resume-session-at".into());
        args.push(fork_ref.resume_session_at);
        return;
    }

    args.push("--resume".into());
    args.push(sid);
    if !req.resume_session_at.is_empty() {
        args.push("--resume-session-at".into());
        args.push(req.resume_session_at.clone());
    }
}

pub fn new(opts: Options) -> Arc<ProtocolAdapter> {
    let binary = if opts.binary.is_empty() {
        default_claude_binary()
    } else {
        opts.binary
    };
    let spec = Spec {
        id: DESCRIPTOR.id,
        label: "Claude Code".into(),
        protocol: Protocol::StdioNewlineJson,
        capabilities: Capabilities {
            resume: true,
            multi_turn: true,
            thinking: true,
            tool_stream: true,
            usage: true,
            structured_intervention: true,
            command_catalog: true,
            permission_intercept: false,
        },
        arg_profile: ArgProfile {
            system_prompt: true,
            resume_session_key: "session_id".into(),
            resume_session_id_hint: true,
        },
        config_schema: ConfigSchema {
            fields: vec![
                Field {
                    key: "model".into(),
                    ty: "enum".into(),
                    label: "Model".into(),
                    default: Some(serde_json::Value::String("claude-sonnet-4".into())),
                    r#enum: vec![
                        "claude-sonnet-4".into(),
                        "claude-opus-4".into(),
                        "claude-haiku-4".into(),
                    ],
                    ..Default::default()
                },
                Field {
                    key: "binary".into(),
                    ty: "path".into(),
                    label: "Claude CLI binary".into(),
                    default: Some(serde_json::Value::String(binary.clone())),
                    ..Default::default()
                },
                Field {
                    key: "system_prompt".into(),
                    ty: "string".into(),
                    label: "System prompt (optional)".into(),
                    ..Default::default()
                },
            ],
        },
    };

    let blocked_owned: Vec<(String, BlockedArgMode)> = [
        ("--output-format", BlockedArgMode::WithValue),
        ("--input-format", BlockedArgMode::WithValue),
        ("--permission-mode", BlockedArgMode::WithValue),
        ("--fork-session", BlockedArgMode::Standalone),
        ("--resume-session-at", BlockedArgMode::WithValue),
        ("--replay-user-messages", BlockedArgMode::Standalone),
        ("--print", BlockedArgMode::Standalone),
        (
            "--allow-dangerously-skip-permissions",
            BlockedArgMode::Standalone,
        ),
    ]
    .iter()
    .map(|(n, m)| (n.to_string(), *m))
    .collect();

    let cli_version = detect_claude_cli_version(&binary);
    info!(
        target: "lucarne::adapters::claude",
        binary = binary.as_str(),
        cli_version = ?cli_version,
        "claude adapter configured"
    );
    Arc::new(ProtocolAdapter::new(ProtocolOptions {
        spec,
        binary: binary.clone(),
        launcher: None,
        framer: None,
        dialect_factory: Arc::new(move || Box::new(Claude::with_cli_version(cli_version.clone()))),
        build_args: Some(Arc::new(move |req, _files| -> Result<Vec<String>> {
            let mut args: Vec<String> = vec![
                "--input-format".into(),
                "stream-json".into(),
                "--output-format".into(),
                "stream-json".into(),
                "--verbose".into(),
                "--replay-user-messages".into(),
            ];
            if !req.model.is_empty() {
                args.push("--model".into());
                args.push(req.model.clone());
            }
            if matches!(
                req.permission_mode,
                PermissionMode::Full | PermissionMode::Bypass
            ) {
                args.push("--allow-dangerously-skip-permissions".into());
            }
            if req.permission_mode != PermissionMode::Default {
                args.push("--permission-mode".into());
                args.push(claude_permission_mode_arg(req.permission_mode).into());
            }
            let sid = claude_resume_session_id(&req.resume, &req.cwd);
            if !sid.is_empty() {
                push_claude_resume_args(&mut args, req, sid);
            } else if !req.system_prompt.is_empty() {
                args.push("--append-system-prompt".into());
                args.push(req.system_prompt.clone());
            }
            let blocked_slice: Vec<(&str, BlockedArgMode)> = blocked_owned
                .iter()
                .map(|(n, m)| (n.as_str(), *m))
                .collect();
            args.extend(filter_extra_args(&req.extra_args, &blocked_slice));
            Ok(args)
        })),
        build_session: None,
        prepare_start: None,
        probe: Some(Arc::new({
            let bin = binary.clone();
            move || probe_version(DESCRIPTOR.id.as_str(), &bin, Some("2.1.112"))
        })),
    }))
}

fn claude_resume_session_id(resume: &Option<crate::event::ResumeHandle>, cwd: &str) -> String {
    let Some(resume) = resume else {
        return String::new();
    };
    let value = match resume.data.get("session_id") {
        Some(serde_json::Value::String(value)) if !value.is_empty() => value.clone(),
        _ => return String::new(),
    };
    let resume_cwd = resume
        .data
        .get("cwd")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if !same_claude_resume_cwd(cwd, resume_cwd) {
        return String::new();
    }
    value
}

fn same_claude_resume_cwd(cwd: &str, resume_cwd: &str) -> bool {
    if cwd.is_empty() || resume_cwd.is_empty() {
        return true;
    }
    normalize_claude_resume_cwd(cwd) == normalize_claude_resume_cwd(resume_cwd)
}

fn normalize_claude_resume_cwd(cwd: &str) -> String {
    let path = std::path::Path::new(cwd);
    if path.is_absolute() {
        return clean_claude_resume_path(path);
    }
    match std::env::current_dir() {
        Ok(current) => clean_claude_resume_path(&current.join(path)),
        Err(_) => clean_claude_resume_path(path),
    }
}

fn clean_claude_resume_path(path: &std::path::Path) -> String {
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::{
        claude_permission_mode_arg, claude_resume_session_id, default_claude_binary_from,
        push_claude_resume_args, short_cli_version,
    };
    use crate::{dialect::PermissionMode, event::ResumeHandle};
    use serde_json::Value;
    use std::collections::BTreeMap;

    #[test]
    fn claude_fork_resume_ref_expands_to_resume_at_with_fork_session() {
        let mut data = BTreeMap::new();
        data.insert(
            "session_id".into(),
            Value::String("claude-fork:v1:c2Vzc2lvbi1zb3VyY2U:dHVybi11dWlk".into()),
        );
        data.insert("cwd".into(), Value::String("/work".into()));
        let req = crate::adapter::SessionParams {
            cwd: "/work".into(),
            resume: Some(ResumeHandle { version: 1, data }),
            ..Default::default()
        };

        let sid = claude_resume_session_id(&req.resume, &req.cwd);
        let mut args = Vec::new();
        push_claude_resume_args(&mut args, &req, sid);

        assert_eq!(
            args,
            [
                "--resume",
                "session-source",
                "--fork-session",
                "--resume-session-at",
                "turn-uuid"
            ]
        );
    }

    #[test]
    fn maps_canonical_permission_modes_to_claude_cli_values() {
        assert_eq!(
            claude_permission_mode_arg(PermissionMode::Default),
            "default"
        );
        assert_eq!(
            claude_permission_mode_arg(PermissionMode::Write),
            "acceptEdits"
        );
        assert_eq!(claude_permission_mode_arg(PermissionMode::ReadOnly), "plan");
        assert_eq!(claude_permission_mode_arg(PermissionMode::Auto), "auto");
        assert_eq!(
            claude_permission_mode_arg(PermissionMode::Full),
            "bypassPermissions"
        );
    }

    #[test]
    fn default_binary_prefers_env_override() {
        let selected = default_claude_binary_from(Some(" /custom/claude "), None, |_| true);

        assert_eq!(selected, "/custom/claude");
    }

    #[test]
    fn default_binary_prefers_current_pnpm_install_before_path_lookup() {
        let expected = std::path::Path::new("/Users/era")
            .join("Library/pnpm/claude")
            .to_string_lossy()
            .into_owned();
        let selected =
            default_claude_binary_from(None, Some("/Users/era"), |path| path == expected);

        assert_eq!(selected, expected);
    }

    #[test]
    fn default_binary_falls_back_to_path_lookup() {
        let selected = default_claude_binary_from(None, Some("/Users/era"), |_| false);

        assert_eq!(selected, "claude");
    }

    #[test]
    fn short_cli_version_extracts_claude_version_output() {
        assert_eq!(
            short_cli_version("2.1.112 (Claude Code)\n").as_deref(),
            Some("2.1.112")
        );
        assert_eq!(
            short_cli_version("claude 2.1.119\n").as_deref(),
            Some("2.1.119")
        );
    }
}
