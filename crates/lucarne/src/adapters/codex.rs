//! Codex adapter — runs `codex app-server --listen stdio://` as a
//! persistent JSON-RPC server over stdin/stdout.

use crate::{
    adapter::{
        ArgProfile, Capabilities, ConfigSchema, Field, Protocol, ProtocolAdapter, ProtocolOptions,
        Spec,
    },
    adapters::{filter_extra_args, prepare_codex_start, probe_version, BlockedArgMode},
    agent_registry::{AgentDescriptor, ALL_AGENT_DESCRIPTORS},
    dialect::{PermissionMode, SessionParams},
    dialects::codex::Codex,
    error::Result,
    framer::Framer,
    ProviderId,
};
use linkme::distributed_slice;
use std::sync::Arc;
use tracing::info;

pub struct Options {
    pub binary: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            binary: "codex".into(),
        }
    }
}

fn default_adapter() -> Arc<ProtocolAdapter> {
    new(Options::default())
}

#[distributed_slice(ALL_AGENT_DESCRIPTORS)]
static DESCRIPTOR: AgentDescriptor = AgentDescriptor {
    id: ProviderId::from_static("codex"),
    order: 20,
    adapter_factory: Some(default_adapter),
};

pub fn new(opts: Options) -> Arc<ProtocolAdapter> {
    let binary = if opts.binary.is_empty() {
        "codex".into()
    } else {
        opts.binary
    };
    let spec = Spec {
        id: DESCRIPTOR.id,
        label: "OpenAI Codex CLI".into(),
        protocol: Protocol::StdioJsonrpc,
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
            resume_session_key: "thread_id".into(),
            resume_session_id_hint: true,
            ..Default::default()
        },
        config_schema: ConfigSchema {
            fields: vec![
                Field {
                    key: "model".into(),
                    ty: "string".into(),
                    label: "Model".into(),
                    default: Some(serde_json::Value::String("codex-mini-latest".into())),
                    ..Default::default()
                },
                Field {
                    key: "binary".into(),
                    ty: "path".into(),
                    label: "Codex CLI binary".into(),
                    default: Some(serde_json::Value::String(binary.clone())),
                    ..Default::default()
                },
            ],
        },
    };

    let blocked_owned = blocked_args();

    info!(
        target: "lucarne::adapters::codex",
        binary = binary.as_str(),
        "codex adapter configured"
    );
    Arc::new(ProtocolAdapter::new(ProtocolOptions {
        spec,
        binary: binary.clone(),
        launcher: None,
        framer: Some(Framer::jsonrpc()),
        dialect_factory: Arc::new(|| Box::new(Codex::new())),
        build_args: Some(Arc::new(move |req, _files| -> Result<Vec<String>> {
            Ok(codex_app_server_args(req, &blocked_owned))
        })),
        build_session: None,
        prepare_start: Some(Arc::new(prepare_codex_start)),
        probe: Some(Arc::new({
            let bin = binary.clone();
            move || probe_version(DESCRIPTOR.id.as_str(), &bin, Some("0.100.0"))
        })),
    }))
}

fn blocked_args() -> Vec<(String, BlockedArgMode)> {
    [
        ("--listen", BlockedArgMode::WithValue),
        (
            "--dangerously-bypass-approvals-and-sandbox",
            BlockedArgMode::Standalone,
        ),
        ("--sandbox", BlockedArgMode::WithValue),
        ("-s", BlockedArgMode::WithValue),
        ("--ask-for-approval", BlockedArgMode::WithValue),
        ("-a", BlockedArgMode::WithValue),
    ]
    .iter()
    .map(|(n, m)| (n.to_string(), *m))
    .collect()
}

fn codex_app_server_args(
    req: &SessionParams,
    blocked_owned: &[(String, BlockedArgMode)],
) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if req.permission_mode == PermissionMode::Bypass {
        args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
    }
    args.extend([
        "app-server".to_string(),
        "--listen".to_string(),
        "stdio://".to_string(),
    ]);
    let blocked_slice: Vec<(&str, BlockedArgMode)> = blocked_owned
        .iter()
        .map(|(n, m)| (n.as_str(), *m))
        .collect();
    let extra_args = filter_extra_args(&req.extra_args, &blocked_slice);
    if req.permission_mode == PermissionMode::Bypass {
        args.extend(filter_permission_config_extra_args(&extra_args));
    } else {
        args.extend(extra_args);
    }
    args
}

fn filter_permission_config_extra_args(extra_args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(extra_args.len());
    let mut iter = extra_args.iter().peekable();
    while let Some(arg) = iter.next() {
        if matches!(arg.as_str(), "-c" | "--config") {
            let Some(value) = iter.next() else {
                out.push(arg.clone());
                break;
            };
            if !codex_config_override_targets_permissions(value) {
                out.push(arg.clone());
                out.push(value.clone());
            }
            continue;
        }
        if let Some(value) = arg.strip_prefix("--config=") {
            if !codex_config_override_targets_permissions(value) {
                out.push(arg.clone());
            }
            continue;
        }
        out.push(arg.clone());
    }
    out
}

fn codex_config_override_targets_permissions(value: &str) -> bool {
    let key = value
        .split_once('=')
        .map(|(key, _)| key)
        .unwrap_or(value)
        .trim();
    matches!(
        key,
        "approval_policy"
            | "approvalPolicy"
            | "approvals_reviewer"
            | "approvalsReviewer"
            | "sandbox"
            | "sandbox_mode"
            | "sandboxMode"
            | "default_permissions"
            | "defaultPermissions"
            | "sandbox_workspace_write"
            | "sandboxWorkspaceWrite"
            | "permissions"
    ) || key.starts_with("sandbox_workspace_write.")
        || key.starts_with("sandboxWorkspaceWrite.")
        || key.starts_with("permissions.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_resume_ref_is_known_session_id_hint() {
        let adapter = new(Options::default());

        assert!(adapter.spec().arg_profile.resume_session_id_hint);
    }

    #[test]
    fn bypass_permission_mode_uses_dangerous_startup_arguments() {
        let req = crate::adapter::SessionParams {
            permission_mode: crate::dialect::PermissionMode::Bypass,
            ..Default::default()
        };

        assert_eq!(
            codex_app_server_args(&req, &blocked_args()),
            vec![
                "--dangerously-bypass-approvals-and-sandbox",
                "app-server",
                "--listen",
                "stdio://",
            ]
        );
    }

    #[test]
    fn bypass_permission_mode_filters_conflicting_config_extra_args() {
        let req = crate::adapter::SessionParams {
            permission_mode: crate::dialect::PermissionMode::Bypass,
            extra_args: vec![
                "-c".into(),
                "default_permissions=\":danger-no-sandbox\"".into(),
                "--config=sandbox_mode=\"read-only\"".into(),
                "--config".into(),
                "approval_policy=\"untrusted\"".into(),
                "-c".into(),
                "model=\"gpt-5.5\"".into(),
                "--keep".into(),
            ],
            ..Default::default()
        };

        let args = codex_app_server_args(&req, &blocked_args());

        assert!(!args.iter().any(|arg| arg.contains("default_permissions")));
        assert!(!args
            .iter()
            .any(|arg| arg.contains("sandbox_mode=\"read-only\"")));
        assert!(!args
            .iter()
            .any(|arg| arg.contains("approval_policy=\"untrusted\"")));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["-c", "model=\"gpt-5.5\""]));
        assert!(args.iter().any(|arg| arg == "--keep"));
    }
}
