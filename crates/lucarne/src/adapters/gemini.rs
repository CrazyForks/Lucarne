//! Gemini adapter — runs `gemini --acp` as a JSON-RPC stdio server.

use crate::{
    adapter::{
        ArgProfile, Capabilities, ConfigSchema, Field, Protocol, ProtocolAdapter, ProtocolOptions,
        Spec,
    },
    adapters::{filter_extra_args, probe_version, BlockedArgMode},
    agent_registry::{AgentDescriptor, ALL_AGENT_DESCRIPTORS},
    dialects::gemini::Gemini,
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
            binary: "gemini".into(),
        }
    }
}

fn default_adapter() -> Arc<ProtocolAdapter> {
    new(Options::default())
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
        prepare_start: None,
        probe: Some(Arc::new({
            let bin = binary.clone();
            move || probe_version(DESCRIPTOR.id.as_str(), &bin, None)
        })),
    }))
}
