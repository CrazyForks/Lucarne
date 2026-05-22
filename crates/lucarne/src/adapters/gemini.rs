//! Gemini adapter — runs `gemini --acp` as a JSON-RPC stdio server.

use crate::{
    adapter::{
        ArgProfile, Capabilities, ConfigSchema, Field, Protocol, ProtocolAdapter, ProtocolOptions,
        Spec,
    },
    adapters::{filter_extra_args, prepare_local_cli_start, probe_version, BlockedArgMode},
    agent_registry::{AgentDescriptor, ALL_AGENT_DESCRIPTORS},
    dialects::gemini::Gemini,
    error::Result,
    framer::Framer,
    ProviderId,
};
use linkme::distributed_slice;
use std::{collections::BTreeMap, sync::Arc};
use tracing::info;

pub struct Options {
    pub binary: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            binary: "gemini".into(),
        }
    }
}

fn default_adapter() -> Arc<ProtocolAdapter> {
    new(Options::default())
}

const GEMINI_CLI_TRUST_WORKSPACE_ENV: &str = "GEMINI_CLI_TRUST_WORKSPACE";

fn prepare_gemini_start(
    req: &crate::adapter::SessionParams,
    binary: &str,
) -> Result<(crate::adapter::SessionParams, String)> {
    let (mut req, resolved) = prepare_local_cli_start(req, binary)?;
    ensure_gemini_headless_trust_env(&mut req.extra_env);
    Ok((req, resolved))
}

fn ensure_gemini_headless_trust_env(env: &mut BTreeMap<String, String>) {
    env.entry(GEMINI_CLI_TRUST_WORKSPACE_ENV.into())
        .or_insert_with(|| "true".into());
}

#[distributed_slice(ALL_AGENT_DESCRIPTORS)]
static DESCRIPTOR: AgentDescriptor = AgentDescriptor {
    id: ProviderId::from_static("gemini"),
    order: 30,
    adapter_factory: Some(default_adapter),
};

pub fn new(opts: Options) -> Arc<ProtocolAdapter> {
    let binary = if opts.binary.is_empty() {
        "gemini".into()
    } else {
        opts.binary
    };
    let spec = Spec {
        id: DESCRIPTOR.id,
        label: "Google Gemini CLI".into(),
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
            resume_session_key: "session_id".into(),
            ..Default::default()
        },
        config_schema: ConfigSchema {
            fields: vec![
                Field {
                    key: "model".into(),
                    ty: "string".into(),
                    label: "Model".into(),
                    default: Some(serde_json::Value::String("gemini-2.5-pro".into())),
                    ..Default::default()
                },
                Field {
                    key: "binary".into(),
                    ty: "path".into(),
                    label: "Gemini CLI binary".into(),
                    default: Some(serde_json::Value::String(binary.clone())),
                    ..Default::default()
                },
            ],
        },
    };

    let blocked_owned: Vec<(String, BlockedArgMode)> = [
        ("--acp", BlockedArgMode::Standalone),
        ("--output-format", BlockedArgMode::WithValue),
        ("-o", BlockedArgMode::WithValue),
        ("--approval-mode", BlockedArgMode::WithValue),
        ("--yolo", BlockedArgMode::Standalone),
        ("--model", BlockedArgMode::WithValue),
        ("-m", BlockedArgMode::WithValue),
        ("--prompt", BlockedArgMode::WithValue),
        ("-p", BlockedArgMode::WithValue),
        ("--prompt-interactive", BlockedArgMode::Standalone),
    ]
    .iter()
    .map(|(n, m)| (n.to_string(), *m))
    .collect();

    info!(
        target: "lucarne::adapters::gemini",
        binary = binary.as_str(),
        "gemini adapter configured"
    );
    Arc::new(ProtocolAdapter::new(ProtocolOptions {
        spec,
        binary: binary.clone(),
        launcher: None,
        framer: Some(Framer::jsonrpc()),
        dialect_factory: Arc::new(|| Box::new(Gemini::new())),
        build_args: Some(Arc::new(move |req, _files| -> Result<Vec<String>> {
            let mut args: Vec<String> = vec!["--acp".into()];
            let blocked_slice: Vec<(&str, BlockedArgMode)> = blocked_owned
                .iter()
                .map(|(n, m)| (n.as_str(), *m))
                .collect();
            args.extend(filter_extra_args(&req.extra_args, &blocked_slice));
            Ok(args)
        })),
        build_session: None,
        prepare_start: Some(Arc::new(prepare_gemini_start)),
        probe: Some(Arc::new({
            let bin = binary.clone();
            move || probe_version(DESCRIPTOR.id.as_str(), &bin, None)
        })),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn gemini_launch_env_trusts_workspace_by_default() {
        let mut env = BTreeMap::new();

        ensure_gemini_headless_trust_env(&mut env);

        assert_eq!(
            env.get("GEMINI_CLI_TRUST_WORKSPACE").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn gemini_launch_env_respects_existing_trust_env() {
        let mut env = BTreeMap::from([(
            "GEMINI_CLI_TRUST_WORKSPACE".to_string(),
            "false".to_string(),
        )]);

        ensure_gemini_headless_trust_env(&mut env);

        assert_eq!(
            env.get("GEMINI_CLI_TRUST_WORKSPACE").map(String::as_str),
            Some("false")
        );
    }
}
