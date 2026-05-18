//! Adapter — top-level factory for sessions of a particular agent
//! family. Holds registry + the shared `protocol adapter` base that
//! wires `Dialect + argv-builder` into the standard runtime stack.

pub use crate::dialect::SessionParams;
use crate::{
    dialect::Dialect,
    error::{LucarneError, Result},
    framer::Framer,
    launcher::{LaunchSpec, Launcher, LocalLauncher, TempFile},
    runtime::{self, Session},
    ProviderId,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info};

#[derive(Debug, Clone, Default, Serialize)]
pub struct Spec {
    pub id: ProviderId,
    pub label: String,
    pub protocol: Protocol,
    pub capabilities: Capabilities,
    #[serde(default)]
    pub arg_profile: ArgProfile,
    #[serde(default)]
    pub config_schema: ConfigSchema,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    #[default]
    StdioNewlineJson,
    StdioJsonrpc,
    Pty,
    Sdk,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Capabilities {
    pub resume: bool,
    pub multi_turn: bool,
    pub thinking: bool,
    pub tool_stream: bool,
    pub usage: bool,
    pub structured_intervention: bool,
    pub command_catalog: bool,
    pub permission_intercept: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArgProfile {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub system_prompt: bool,
    #[serde(default = "default_resume_session_key")]
    pub resume_session_key: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resume_session_id_hint: bool,
}

impl Default for ArgProfile {
    fn default() -> Self {
        Self {
            system_prompt: false,
            resume_session_key: default_resume_session_key(),
            resume_session_id_hint: false,
        }
    }
}

fn default_resume_session_key() -> String {
    "session_id".into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigSchema {
    #[serde(default)]
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Field {
    pub key: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub r#enum: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProbeResult {
    pub available: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

// ——— ProtocolAdapter — the shared subprocess-stdio base ———

pub type BuildArgsFn =
    Arc<dyn Fn(&SessionParams, &mut Vec<TempFile>) -> Result<Vec<String>> + Send + Sync>;
pub type DialectFactoryFn = Arc<dyn Fn() -> Box<dyn Dialect> + Send + Sync>;
pub type PrepareStartFn =
    Arc<dyn Fn(&SessionParams, &str) -> Result<(SessionParams, String)> + Send + Sync>;
pub type ProbeFn = Arc<dyn Fn() -> ProbeResult + Send + Sync>;
pub type BuildSessionFn = Arc<
    dyn Fn(&SessionParams, &mut Vec<TempFile>, Arc<dyn Launcher>) -> Result<ProtocolSessionParts>
        + Send
        + Sync,
>;

pub struct ProtocolSessionParts {
    pub launcher: Arc<dyn Launcher>,
    pub args: Vec<String>,
    pub dialect: Box<dyn Dialect>,
}

pub struct ProtocolAdapter {
    spec: Spec,
    binary: String,
    launcher: Arc<dyn Launcher>,
    framer: Framer,
    dialect_factory: DialectFactoryFn,
    build_args: Option<BuildArgsFn>,
    build_session: Option<BuildSessionFn>,
    prepare_start: Option<PrepareStartFn>,
    probe: Option<ProbeFn>,
}

pub struct ProtocolOptions {
    pub spec: Spec,
    pub binary: String,
    pub launcher: Option<Arc<dyn Launcher>>,
    pub framer: Option<Framer>,
    pub dialect_factory: DialectFactoryFn,
    pub build_args: Option<BuildArgsFn>,
    pub build_session: Option<BuildSessionFn>,
    pub prepare_start: Option<PrepareStartFn>,
    pub probe: Option<ProbeFn>,
}

impl ProtocolAdapter {
    pub fn new(opts: ProtocolOptions) -> Self {
        Self {
            spec: opts.spec,
            binary: opts.binary,
            launcher: opts
                .launcher
                .unwrap_or_else(|| Arc::new(LocalLauncher::new())),
            framer: opts.framer.unwrap_or_else(Framer::newline_json),
            dialect_factory: opts.dialect_factory,
            build_args: opts.build_args,
            build_session: opts.build_session,
            prepare_start: opts.prepare_start,
            probe: opts.probe,
        }
    }

    pub fn spec(&self) -> Spec {
        self.spec.clone()
    }

    pub async fn probe(&self) -> ProbeResult {
        debug!(
            target: "lucarne::adapter",
            provider_id = %self.spec.id,
            binary = self.binary.as_str(),
            "probing protocol adapter"
        );
        if let Some(p) = &self.probe {
            p()
        } else {
            ProbeResult {
                available: true,
                ..Default::default()
            }
        }
    }

    pub async fn start(&self, mut req: SessionParams) -> Result<Session> {
        info!(
            target: "lucarne::adapter",
            provider_id = %self.spec.id,
            binary = self.binary.as_str(),
            model = req.model.as_str(),
            cwd = req.cwd.as_str(),
            resume = req.resume.is_some(),
            resume_session_at = req.resume_session_at.as_str(),
            extra_args = req.extra_args.len(),
            extra_env = req.extra_env.len(),
            "starting protocol adapter session"
        );
        let mut binary = self.binary.clone();
        if let Some(prep) = &self.prepare_start {
            let (r, b) = prep(&req, &binary)?;
            req = r;
            binary = b;
        } else {
            let (r, b) = crate::adapters::prepare_local_cli_start(&req, &binary)?;
            req = r;
            binary = b;
        }
        let mut files: Vec<TempFile> = Vec::new();
        let session_parts = if let Some(build_session) = &self.build_session {
            build_session(&req, &mut files, Arc::clone(&self.launcher))?
        } else {
            let build_args = self.build_args.as_ref().ok_or_else(|| {
                LucarneError::adapter("protocol adapter missing build_args/build_session")
            })?;
            ProtocolSessionParts {
                launcher: Arc::clone(&self.launcher),
                args: build_args(&req, &mut files)?,
                dialect: (self.dialect_factory)(),
            }
        };
        debug!(
            target: "lucarne::adapter",
            provider_id = %self.spec.id,
            binary = binary.as_str(),
            args = session_parts.args.len(),
            temp_files = files.len(),
            "protocol adapter prepared launch spec"
        );
        let spec = LaunchSpec {
            bin: binary,
            args: session_parts.args,
            cwd: req.cwd.clone(),
            env: req.extra_env.clone(),
            files,
            use_pty: false,
        };
        let cfg = runtime::Config::new(
            session_parts.launcher,
            spec,
            self.framer,
            session_parts.dialect,
            req,
        );
        let session = runtime::start(cfg).await?;
        info!(
            target: "lucarne::adapter",
            provider_id = %self.spec.id,
            runtime_session_id = session.id(),
            epoch = session.epoch(),
            "protocol adapter session started"
        );
        Ok(session)
    }
}
