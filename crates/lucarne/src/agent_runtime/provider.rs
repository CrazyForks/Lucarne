use std::sync::Arc;

use async_trait::async_trait;
use smol_str::SmolStr;
use tracing::{debug, info, warn};

use crate::{
    adapter::{self, ArgProfile, ProtocolAdapter},
    ProviderId,
};

use super::{
    provider_args, AgentCapabilities, AgentError, AgentErrorKind, AgentSession, AgentSessionFacade,
    OpenSession, ProbeResult, ResumeSession,
};

#[async_trait]
pub trait AgentProvider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn label(&self) -> &str {
        self.id().as_str()
    }
    fn binary(&self) -> &str {
        self.id().as_str()
    }
    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities::default()
    }

    async fn probe(&self) -> Result<ProbeResult, AgentError>;
    async fn open(&self, req: OpenSession) -> Result<Box<dyn AgentSession>, AgentError>;
    async fn resume(&self, req: ResumeSession) -> Result<Box<dyn AgentSession>, AgentError>;
}

pub struct ProtocolProvider {
    provider_id: ProviderId,
    label: SmolStr,
    binary: SmolStr,
    capabilities: AgentCapabilities,
    arg_profile: ArgProfile,
    adapter: Arc<ProtocolAdapter>,
}

impl ProtocolProvider {
    pub fn new(adapter: Arc<ProtocolAdapter>) -> Result<Self, AgentError> {
        let spec = adapter.spec();
        let provider_id = spec.id;
        if provider_id.as_str().trim().is_empty() {
            return Err(unsupported(
                "protocol adapter provider id must not be empty",
            ));
        }
        Ok(Self {
            provider_id,
            label: smol_str_from_spec(&spec.label)?,
            binary: binary_from_spec(&spec)?,
            capabilities: runtime_capabilities_from_spec(&spec),
            arg_profile: spec.arg_profile,
            adapter,
        })
    }
}

#[async_trait]
impl AgentProvider for ProtocolProvider {
    fn id(&self) -> ProviderId {
        self.provider_id
    }

    fn label(&self) -> &str {
        self.label.as_str()
    }

    fn binary(&self) -> &str {
        self.binary.as_str()
    }

    fn capabilities(&self) -> AgentCapabilities {
        self.capabilities
    }

    async fn probe(&self) -> Result<ProbeResult, AgentError> {
        debug!(
            target: "lucarne::agent_runtime::provider",
            provider_id = %self.provider_id,
            "probing provider"
        );
        let probe = self.adapter.probe().await;
        if !probe.available {
            warn!(
                target: "lucarne::agent_runtime::provider",
                provider_id = %self.provider_id,
                error = probe.error.as_str(),
                path = probe.path.as_str(),
                "provider unavailable"
            );
            return Err(unsupported(if probe.error.is_empty() {
                format!("provider {:?} is unavailable", self.provider_id.as_str())
            } else {
                probe.error
            }));
        }

        info!(
            target: "lucarne::agent_runtime::provider",
            provider_id = %self.provider_id,
            provider_version = probe.version.as_str(),
            path = probe.path.as_str(),
            "provider probe ok"
        );
        Ok(ProbeResult {
            provider_id: self.provider_id,
            provider_version: non_empty_smol(probe.version),
            capabilities: self.capabilities,
        })
    }

    async fn open(&self, req: OpenSession) -> Result<Box<dyn AgentSession>, AgentError> {
        info!(
            target: "lucarne::agent_runtime::provider",
            provider_id = %self.provider_id,
            model = req.model.as_deref().unwrap_or("-"),
            cwd = req.cwd.as_deref().unwrap_or("-"),
            has_initial_input = req.initial_input.is_some(),
            "provider open requested"
        );
        let (start, initial_input, session_options) =
            provider_args::decode_open(self.provider_id, &self.arg_profile, req)?;
        let session = self.adapter.start(start).await.map_err(map_lucarne_error)?;
        let session = AgentSessionFacade::attach_with_options_and_capabilities(
            self.provider_id,
            fresh_instance_id(),
            session,
            initial_input,
            None,
            session_options,
            self.capabilities,
        )
        .await?;
        info!(
            target: "lucarne::agent_runtime::provider",
            provider_id = %self.provider_id,
            instance_id = %session.instance_id().0,
            session_id = %session.id().0,
            "provider open completed"
        );
        Ok(Box::new(session))
    }

    async fn resume(&self, req: ResumeSession) -> Result<Box<dyn AgentSession>, AgentError> {
        info!(
            target: "lucarne::agent_runtime::provider",
            provider_id = %self.provider_id,
            session_ref = %req.session_ref.0,
            "provider resume requested"
        );
        let (start, session_options, known_session_id) =
            provider_args::decode_resume(self.provider_id, &self.arg_profile, req)?;
        let session = self.adapter.start(start).await.map_err(map_lucarne_error)?;
        let session = AgentSessionFacade::attach_with_options_and_capabilities(
            self.provider_id,
            fresh_instance_id(),
            session,
            None,
            known_session_id,
            session_options,
            self.capabilities,
        )
        .await?;
        info!(
            target: "lucarne::agent_runtime::provider",
            provider_id = %self.provider_id,
            instance_id = %session.instance_id().0,
            session_id = %session.id().0,
            "provider resume completed"
        );
        Ok(Box::new(session))
    }
}

fn binary_from_spec(spec: &adapter::Spec) -> Result<SmolStr, AgentError> {
    spec.config_schema
        .fields
        .iter()
        .find(|field| field.key == "binary")
        .and_then(|field| field.default.as_ref())
        .and_then(|value| value.as_str())
        .map(smol_str_from_spec)
        .unwrap_or_else(|| smol_str_from_spec(spec.id.as_str()))
}

fn smol_str_from_spec(value: &str) -> Result<SmolStr, AgentError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(unsupported(
            "adapter spec must declare non-empty static strings",
        ));
    }
    Ok(SmolStr::new(value))
}

fn runtime_capabilities_from_spec(spec: &adapter::Spec) -> AgentCapabilities {
    AgentCapabilities {
        reasoning_stream: spec.capabilities.thinking,
        tool_stream: spec.capabilities.tool_stream,
        usage_reporting: spec.capabilities.usage,
        structured_intervention: spec.capabilities.structured_intervention,
        command_catalog: spec.capabilities.command_catalog,
    }
}

fn fresh_instance_id() -> super::InstanceId {
    super::InstanceId(uuid::Uuid::new_v4().to_string().into())
}

fn non_empty_smol(value: String) -> Option<SmolStr> {
    if value.is_empty() {
        None
    } else {
        Some(value.into())
    }
}

fn map_lucarne_error(err: crate::error::LucarneError) -> AgentError {
    let kind = AgentErrorKind::from(match &err {
        crate::error::LucarneError::Closed => crate::error::LucarneError::Closed,
        crate::error::LucarneError::Adapter(message) => {
            crate::error::LucarneError::Adapter(message.clone())
        }
        crate::error::LucarneError::Dialect(message) => {
            crate::error::LucarneError::Dialect(message.clone())
        }
        crate::error::LucarneError::Protocol(message) => {
            crate::error::LucarneError::Protocol(message.clone())
        }
        crate::error::LucarneError::Launcher(message) => {
            crate::error::LucarneError::Launcher(message.clone())
        }
        crate::error::LucarneError::Runtime(message) => {
            crate::error::LucarneError::Runtime(message.clone())
        }
        crate::error::LucarneError::Io(io) => {
            crate::error::LucarneError::Io(std::io::Error::new(io.kind(), io.to_string()))
        }
        crate::error::LucarneError::Json(json) => {
            crate::error::LucarneError::Other(json.to_string())
        }
        crate::error::LucarneError::Timeout => crate::error::LucarneError::Timeout,
        crate::error::LucarneError::Other(message) => {
            crate::error::LucarneError::Other(message.clone())
        }
    });

    AgentError {
        kind,
        message: err.to_string().into(),
    }
}

fn unsupported(message: impl Into<String>) -> AgentError {
    AgentError {
        kind: AgentErrorKind::Unsupported,
        message: message.into().into(),
    }
}
