//! Grok Build adapter — spawns `grok agent stdio` (ACP JSON-RPC).

use crate::{
    adapter::{
        ArgProfile, Capabilities, ConfigSchema, Field, Protocol, ProtocolAdapter, ProtocolOptions,
        Spec,
    },
    adapters::{filter_extra_args, probe_version, BlockedArgMode},
    agent_registry::{AgentDescriptor, ALL_AGENT_DESCRIPTORS},
    dialects::grok_acp::GrokAcp,
    error::Result,
    ProviderId,
};
use linkme::distributed_slice;
use std::{path::Path, sync::Arc};
use tracing::info;

pub struct Options {
    pub binary: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            binary: default_grok_binary(),
        }
    }
}

fn default_adapter() -> Arc<ProtocolAdapter> {
    new(Options::default())
}

#[distributed_slice(ALL_AGENT_DESCRIPTORS)]
static DESCRIPTOR: AgentDescriptor = AgentDescriptor {
    id: ProviderId::from_static("grok"),
    order: 50,
    adapter_factory: Some(default_adapter),
};

pub fn new(opts: Options) -> Arc<ProtocolAdapter> {
    let binary = select_grok_binary(opts.binary, std::env::var("LUCARNE_GROK_BIN").ok());

    let spec = Spec {
        id: DESCRIPTOR.id,
        label: "Grok Build".into(),
        protocol: Protocol::StdioJsonrpc,
        capabilities: Capabilities {
            resume: true,
            multi_turn: true,
            thinking: true,
            tool_stream: true,
            usage: true,
            structured_intervention: true,
            command_catalog: true,
            permission_intercept: true,
        },
        arg_profile: ArgProfile {
            system_prompt: true,
            resume_session_key: "session_id".into(),
            resume_session_id_hint: true,
            ..Default::default()
        },
        config_schema: ConfigSchema {
            fields: vec![
                Field {
                    key: "model".into(),
                    ty: "string".into(),
                    label: "Model".into(),
                    required: false,
                    ..Default::default()
                },
                Field {
                    key: "binary".into(),
                    ty: "path".into(),
                    label: "Grok CLI binary".into(),
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
        ("-p", BlockedArgMode::Standalone),
        ("--single", BlockedArgMode::WithValue),
        ("--prompt-file", BlockedArgMode::WithValue),
        ("--prompt-json", BlockedArgMode::WithValue),
        ("--output-format", BlockedArgMode::WithValue),
        ("-r", BlockedArgMode::WithValue),
        ("--resume", BlockedArgMode::WithValue),
        ("-c", BlockedArgMode::Standalone),
        ("--continue", BlockedArgMode::Standalone),
        ("-s", BlockedArgMode::WithValue),
        ("--session-id", BlockedArgMode::WithValue),
        ("-m", BlockedArgMode::WithValue),
        ("--model", BlockedArgMode::WithValue),
        ("--always-approve", BlockedArgMode::Standalone),
        ("--yolo", BlockedArgMode::Standalone),
    ]
    .iter()
    .map(|(n, m)| (n.to_string(), *m))
    .collect();

    let cli_version = short_cli_version(&probe_version(DESCRIPTOR.id.as_str(), &binary, None).version);
    info!(
        target: "lucarne::adapters::grok",
        binary = binary.as_str(),
        cli_version = ?cli_version,
        "grok acp adapter configured"
    );

    Arc::new(ProtocolAdapter::new(ProtocolOptions {
        spec,
        binary: binary.clone(),
        launcher: None,
        framer: None,
        dialect_factory: Arc::new(|| Box::new(GrokAcp::new())),
        build_args: Some(Arc::new(move |req, _files| -> Result<Vec<String>> {
            let mut args: Vec<String> = vec!["agent".into()];
            if !req.model.is_empty() {
                args.push("--model".into());
                args.push(req.model.clone());
            }
            // Only auto-approve when Lucarne permission mode asks for it.
            // Default/Write keep session/request_permission so live
            // approval/reject flows match Codex-style intervention tests.
            use crate::dialect::PermissionMode;
            if matches!(
                req.permission_mode,
                PermissionMode::Bypass | PermissionMode::Full | PermissionMode::Auto
            ) {
                args.push("--always-approve".into());
            }
            args.push("stdio".into());

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
            move || probe_version(DESCRIPTOR.id.as_str(), &bin, None)
        })),
    }))
}

fn default_grok_binary() -> String {
    let override_bin = std::env::var("LUCARNE_GROK_BIN").ok();
    let home = crate::host::paths::home_dir().map(|p| p.to_string_lossy().into_owned());
    default_grok_binary_from(override_bin.as_deref(), home.as_deref(), |path| {
        Path::new(path).exists()
    })
}

fn default_grok_binary_from<F>(
    env_override: Option<&str>,
    home: Option<&str>,
    exists: F,
) -> String
where
    F: Fn(&str) -> bool,
{
    if let Some(bin) = env_override.map(str::trim).filter(|b| !b.is_empty()) {
        return bin.to_string();
    }
    if let Some(home) = home.map(str::trim).filter(|h| !h.is_empty()) {
        let candidate = Path::new(home).join(".grok/bin/grok");
        let s = candidate.to_string_lossy().into_owned();
        if exists(&s) {
            return s;
        }
    }
    "grok".into()
}

fn select_grok_binary(configured: String, env_override: Option<String>) -> String {
    let configured = configured.trim();
    if !configured.is_empty() && configured != "grok" {
        return configured.to_string();
    }
    if let Some(env) = env_override {
        let env = env.trim();
        if !env.is_empty() {
            return env.to_string();
        }
    }
    default_grok_binary()
}

fn short_cli_version(raw: &str) -> Option<String> {
    raw.split_whitespace()
        .map(|part| part.trim_start_matches('v'))
        .find(|part| part.chars().next().is_some_and(|ch| ch.is_ascii_digit()))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_lucarne_grok_bin_override_path() {
        assert_eq!(
            select_grok_binary("grok".into(), Some("/tmp/fake-grok".into())),
            "/tmp/fake-grok"
        );
    }

    #[test]
    fn prefers_home_bin_when_present() {
        assert_eq!(
            default_grok_binary_from(None, Some("/home/user"), |p| p.ends_with(".grok/bin/grok")),
            "/home/user/.grok/bin/grok"
        );
    }
}
