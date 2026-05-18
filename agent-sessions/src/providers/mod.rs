#[cfg(feature = "claude")]
pub mod claude;
#[cfg(feature = "codex")]
pub mod codex;
#[cfg(feature = "copilot")]
pub mod copilot;
#[cfg(feature = "cursor")]
pub mod cursor;
#[cfg(all(feature = "discovery", feature = "agent_session"))]
mod descriptor;
#[cfg(feature = "gemini")]
pub mod gemini;
#[cfg(feature = "pi")]
pub mod pi;

#[cfg(all(feature = "discovery", feature = "agent_session"))]
pub use descriptor::{
    AgentProviderDescriptor, AgentProviderSource, SessionFileFormat, agent_provider,
    agent_providers,
};
