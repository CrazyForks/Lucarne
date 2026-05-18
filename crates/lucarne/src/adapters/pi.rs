//! Pi RPC adapter — spawns `pi --mode rpc` once and keeps it alive.
//!
//! No more `--list-models` shell-out, no `PiRetryLauncher`, no session
//! file resolution. Model validation happens via RPC `get_available_models`
//! after the session starts. The adapter builds argv for the persistent
//! RPC-mode process and wires the [`PiRpc`](crate::dialects::pi_rpc::PiRpc)
//! dialect.

use crate::{
    adapter::{
        ArgProfile, Capabilities, ConfigSchema, Field, Protocol, ProtocolAdapter, ProtocolOptions,
        Spec,
    },
    adapters::{filter_extra_args, probe_version, BlockedArgMode},
    agent_registry::{AgentDescriptor, ALL_AGENT_DESCRIPTORS},
    dialects::pi_rpc::PiRpc,
    error::Result,
    ProviderId,
};
use linkme::distributed_slice;
use std::sync::Arc;
use tracing::info;

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

pub struct Options {
    pub binary: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            binary: "pi".into(),
        }
    }
}

fn default_adapter() -> Arc<ProtocolAdapter> {
    new(Options::default())
}

#[distributed_slice(ALL_AGENT_DESCRIPTORS)]
static DESCRIPTOR: AgentDescriptor = AgentDescriptor {
    id: ProviderId::from_static("pi"),
    order: 40,
    adapter_factory: Some(default_adapter),
};

// ---------------------------------------------------------------------------
// Adapter factory
// ---------------------------------------------------------------------------

pub fn new(opts: Options) -> Arc<ProtocolAdapter> {
    let binary = select_pi_binary(opts.binary, std::env::var("LUCARNE_PI_BIN").ok());

    let spec = Spec {
        id: DESCRIPTOR.id,
        label: "Pi CLI (RPC)".into(),
        protocol: Protocol::StdioNewlineJson,
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
            resume_session_key: "session_path".into(),
            resume_session_id_hint: true,
            ..Default::default()
        },
        config_schema: ConfigSchema {
            fields: vec![
                Field {
                    key: "model".into(),
                    ty: "string".into(),
                    label: "Model".into(),
                    required: true,
                    ..Default::default()
                },
                Field {
                    key: "binary".into(),
                    ty: "path".into(),
                    label: "Pi CLI binary".into(),
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
        ("--print", BlockedArgMode::Standalone),
        ("--mode", BlockedArgMode::WithValue),
        ("--session", BlockedArgMode::WithValue),
        ("--provider", BlockedArgMode::WithValue),
        ("--model", BlockedArgMode::WithValue),
        ("--tools", BlockedArgMode::WithValue),
        ("--append-system-prompt", BlockedArgMode::WithValue),
        ("--fork", BlockedArgMode::WithValue),
        ("--continue", BlockedArgMode::Standalone),
        ("--resume", BlockedArgMode::Standalone),
        ("--no-session", BlockedArgMode::Standalone),
        ("--thinking", BlockedArgMode::WithValue),
        ("--system-prompt", BlockedArgMode::WithValue),
        ("--api-key", BlockedArgMode::WithValue),
        ("--export", BlockedArgMode::WithValue),
        ("--list-models", BlockedArgMode::Standalone),
    ]
    .iter()
    .map(|(n, m)| (n.to_string(), *m))
    .collect();

    let cli_version = detect_pi_cli_version(&binary);
    info!(
        target: "lucarne::adapters::pi",
        binary = binary.as_str(),
        cli_version = ?cli_version,
        "pi rpc adapter configured"
    );

    Arc::new(ProtocolAdapter::new(ProtocolOptions {
        spec,
        binary: binary.clone(),
        launcher: None,
        framer: None,
        dialect_factory: Arc::new(move || Box::new(PiRpc::with_cli_version(cli_version.clone()))),
        build_args: Some(Arc::new(move |req, _files| -> Result<Vec<String>> {
            let mut args: Vec<String> = vec!["--mode".into(), "rpc".into()];

            // --provider / --model / --thinking (split "provider/model:<level>")
            let (provider, model) = split_pi_model(&req.model);
            let (model, thinking) = split_pi_model_thinking(&model);
            if !provider.is_empty() {
                args.push("--provider".into());
                args.push(provider);
            }
            if !model.is_empty() {
                args.push("--model".into());
                args.push(model);
            }
            if let Some(level) = thinking {
                args.push("--thinking".into());
                args.push(level);
            }

            // --session (resume path from req.resume or req.resume_session_at)
            if !req.resume_session_at.is_empty() {
                args.push("--session".into());
                args.push(req.resume_session_at.clone());
            } else if let Some(h) = &req.resume {
                if let Some(serde_json::Value::String(path)) = h.data.get("session_path") {
                    if !path.is_empty() {
                        args.push("--session".into());
                        args.push(path.clone());
                    }
                }
            }

            // --system-prompt
            if !req.system_prompt.is_empty() {
                args.push("--system-prompt".into());
                args.push(req.system_prompt.clone());
            }

            // Filter extra_args through blocked set
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
            move || probe_version(DESCRIPTOR.id.as_str(), &bin, Some("0.74.0"))
        })),
    }))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Split `"provider/model"` into `("provider", "model")`.
/// If no `/` is present, the entire string is treated as the model and
/// provider is empty.
fn split_pi_model(s: &str) -> (String, String) {
    let s = s.trim();
    if s.is_empty() {
        return (String::new(), String::new());
    }
    match s.split_once('/') {
        Some((provider, model)) => (provider.trim().to_string(), model.trim().to_string()),
        None => (String::new(), s.to_string()),
    }
}

fn split_pi_model_thinking(s: &str) -> (String, Option<String>) {
    let s = s.trim();
    let Some((model, thinking)) = s.rsplit_once(':') else {
        return (s.to_string(), None);
    };
    let model = model.trim();
    let thinking = thinking.trim();
    if model.is_empty() || thinking.is_empty() {
        return (s.to_string(), None);
    }
    (model.to_string(), Some(thinking.to_string()))
}

fn select_pi_binary(configured: String, env_override: Option<String>) -> String {
    let configured = configured.trim();
    let chosen = if configured.is_empty() || configured == "pi" {
        env_override.as_deref().unwrap_or(configured)
    } else {
        configured
    };
    match chosen.trim() {
        "" => "pi".into(),
        path => path.to_string(),
    }
}

fn detect_pi_cli_version(binary: &str) -> Option<String> {
    short_cli_version(&probe_version(DESCRIPTOR.id.as_str(), binary, Some("0.74.0")).version)
}

fn short_cli_version(raw: &str) -> Option<String> {
    raw.split_whitespace()
        .map(|part| part.trim_start_matches('v'))
        .find(|part| part.chars().next().is_some_and(|ch| ch.is_ascii_digit()))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::select_pi_binary;

    #[test]
    fn explicit_pi_binary_overrides_env_binary() {
        assert_eq!(
            select_pi_binary("/tmp/fake-pi".into(), Some("/tmp/env-pi".into())),
            "/tmp/fake-pi"
        );
    }

    #[test]
    fn default_pi_binary_uses_env_binary() {
        assert_eq!(
            select_pi_binary("pi".into(), Some("/tmp/env-pi".into())),
            "/tmp/env-pi"
        );
    }
}
