//! Codex adapter — runs `codex app-server --listen stdio://` as a
//! persistent JSON-RPC server over stdin/stdout.

use crate::{
    adapter::{
        ArgProfile, Capabilities, ConfigSchema, Field, Protocol, ProtocolAdapter, ProtocolOptions,
        Spec,
    },
    adapters::{filter_extra_args, prepare_codex_start, probe_version, BlockedArgMode},
    agent_registry::{AgentDescriptor, ALL_AGENT_DESCRIPTORS},
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

    let blocked_owned: Vec<(String, BlockedArgMode)> = [("--listen", BlockedArgMode::WithValue)]
        .iter()
        .map(|(n, m)| (n.to_string(), *m))
        .collect();

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
            let mut args: Vec<String> =
                vec!["app-server".into(), "--listen".into(), "stdio://".into()];
            let blocked_slice: Vec<(&str, BlockedArgMode)> = blocked_owned
                .iter()
                .map(|(n, m)| (n.as_str(), *m))
                .collect();
            args.extend(filter_extra_args(&req.extra_args, &blocked_slice));
            Ok(args)
        })),
        build_session: None,
        prepare_start: Some(Arc::new(prepare_codex_start)),
        probe: Some(Arc::new({
            let bin = binary.clone();
            move || probe_version(DESCRIPTOR.id.as_str(), &bin, Some("0.100.0"))
        })),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_resume_ref_is_known_session_id_hint() {
        let adapter = new(Options::default());

        assert!(adapter.spec().arg_profile.resume_session_id_hint);
    }
}
