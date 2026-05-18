//! Copilot adapter — argv builder for `copilot -p <prompt> --output-format json`.

use crate::{
    adapter::{
        ArgProfile, Capabilities, ConfigSchema, Field, Protocol, ProtocolAdapter, ProtocolOptions,
        Spec,
    },
    adapters::{filter_extra_args, probe_version, BlockedArgMode},
    agent_registry::{AgentDescriptor, ALL_AGENT_DESCRIPTORS},
    dialects::copilot::Copilot,
    error::{LucarneError, Result},
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
            binary: "copilot".into(),
        }
    }
}

#[distributed_slice(ALL_AGENT_DESCRIPTORS)]
static DESCRIPTOR: AgentDescriptor = AgentDescriptor {
    id: ProviderId::from_static("copilot"),
    order: 50,
    adapter_factory: None,
};

pub fn new(opts: Options) -> Arc<ProtocolAdapter> {
    let binary = if opts.binary.is_empty() {
        "copilot".into()
    } else {
        opts.binary
    };
    let spec = Spec {
        id: DESCRIPTOR.id,
        label: "GitHub Copilot".into(),
        protocol: Protocol::StdioNewlineJson,
        capabilities: Capabilities {
            resume: true,
            thinking: true,
            ..Default::default()
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
                    ..Default::default()
                },
                Field {
                    key: "binary".into(),
                    ty: "path".into(),
                    label: "Copilot CLI binary".into(),
                    default: Some(serde_json::Value::String(binary.clone())),
                    ..Default::default()
                },
            ],
        },
    };

    let blocked_owned: Vec<(String, BlockedArgMode)> = [
        ("-p", BlockedArgMode::WithValue),
        ("--output-format", BlockedArgMode::WithValue),
        ("--allow-all", BlockedArgMode::Standalone),
        ("--allow-all-tools", BlockedArgMode::Standalone),
        ("--allow-all-paths", BlockedArgMode::Standalone),
        ("--allow-all-urls", BlockedArgMode::Standalone),
        ("--no-ask-user", BlockedArgMode::Standalone),
        ("--resume", BlockedArgMode::WithValue),
        ("--acp", BlockedArgMode::Standalone),
        ("--yolo", BlockedArgMode::Standalone),
    ]
    .iter()
    .map(|(n, m)| (n.to_string(), *m))
    .collect();

    info!(
        target: "lucarne::adapters::copilot",
        binary = binary.as_str(),
        "copilot adapter configured"
    );
    Arc::new(ProtocolAdapter::new(ProtocolOptions {
        spec,
        binary: binary.clone(),
        launcher: None,
        framer: None,
        dialect_factory: Arc::new(|| Box::new(Copilot::new())),
        build_args: Some(Arc::new(move |req, _files| -> Result<Vec<String>> {
            let prompt = req.first_prompt.trim();
            if prompt.is_empty() {
                return Err(LucarneError::adapter(
                    "copilot: FirstPrompt must not be empty for one-shot mode",
                ));
            }
            let mut args = vec![
                "-p".into(),
                prompt.into(),
                "--output-format".into(),
                "json".into(),
                "--allow-all".into(),
                "--no-ask-user".into(),
            ];
            if !req.model.is_empty() {
                args.push("--model".into());
                args.push(req.model.clone());
            }
            let sid = copilot_resume_session_id(&req.resume, &req.cwd);
            if !sid.is_empty() {
                args.push("--resume".into());
                args.push(sid);
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
            move || probe_version(DESCRIPTOR.id.as_str(), &bin, Some("1.0.0"))
        })),
    }))
}

fn copilot_resume_session_id(resume: &Option<crate::event::ResumeHandle>, cwd: &str) -> String {
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
    if !same_copilot_resume_cwd(cwd, resume_cwd) {
        return String::new();
    }
    value
}

fn same_copilot_resume_cwd(cwd: &str, resume_cwd: &str) -> bool {
    if cwd.is_empty() || resume_cwd.is_empty() {
        return true;
    }
    normalize_copilot_resume_cwd(cwd) == normalize_copilot_resume_cwd(resume_cwd)
}

fn normalize_copilot_resume_cwd(cwd: &str) -> String {
    let path = std::path::Path::new(cwd);
    if path.is_absolute() {
        return clean_copilot_resume_path(path);
    }
    match std::env::current_dir() {
        Ok(current) => clean_copilot_resume_path(&current.join(path)),
        Err(_) => clean_copilot_resume_path(path),
    }
}

fn clean_copilot_resume_path(path: &std::path::Path) -> String {
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
