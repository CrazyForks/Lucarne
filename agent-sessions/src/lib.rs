#[cfg(feature = "memory-profiling")]
#[macro_export]
macro_rules! memory_profile_snapshot {
    ($label:literal) => {{
        $crate::emit_memory_profile_snapshot($label);
    }};
}

#[cfg(not(feature = "memory-profiling"))]
#[macro_export]
macro_rules! memory_profile_snapshot {
    ($label:literal) => {{}};
}

#[cfg(feature = "memory-profiling")]
fn emit_memory_profile_snapshot(label: &str) {
    static PAUSE_MS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

    let ts_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    eprintln!(
        "LUCARNE_MEMORY_SNAPSHOT pid={} label={} ts_ms={}",
        std::process::id(),
        label,
        ts_ms
    );

    let pause_ms = *PAUSE_MS.get_or_init(|| {
        std::env::var("LUCARNE_MEMORY_PROFILE_PAUSE_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0)
    });
    if pause_ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(pause_ms));
    }
}

mod agent;
#[cfg(any(feature = "codex", feature = "claude", feature = "gemini"))]
pub mod bash;
mod error;
mod input;
mod parse_selection;
#[cfg(all(
    feature = "discovery",
    any(
        feature = "claude",
        feature = "codex",
        feature = "copilot",
        feature = "cursor",
        feature = "gemini",
        feature = "pi"
    )
))]
mod paths;
pub mod providers;
pub mod reader;
pub mod util;
#[cfg(feature = "watch")]
mod watch;

#[cfg(feature = "agent_session")]
pub mod agent_session;

pub use error::{Error, Result};
pub use input::InputMetadata;
pub use parse_selection::ParseSelection;
#[cfg(feature = "watch")]
pub use watch::{
    SessionWatcher, WatchAssistantMessage, WatchAttachment, WatchChange, WatchConfig, WatchError,
    WatchEvent, WatchEventMeta, WatchMessage, WatchOther, WatchProvider, WatchSnapshot, WatchState,
    WatchToolCall, WatchToolResult, WatchTurnCompleted, WatchTurnFailed, WatchUnknown, WatchUpdate,
    WatchUsage,
};

#[cfg(feature = "claude")]
pub use providers::claude::{self, Claude};
#[cfg(feature = "codex")]
pub use providers::codex::{self, Codex};
#[cfg(feature = "copilot")]
pub use providers::copilot::{self, Copilot};
#[cfg(feature = "cursor")]
pub use providers::cursor::{self, Cursor};
#[cfg(feature = "gemini")]
pub use providers::gemini::{self, Gemini};
#[cfg(feature = "pi")]
pub use providers::pi::{self, Pi};
#[cfg(all(feature = "discovery", feature = "agent_session"))]
pub use providers::{
    AgentProviderDescriptor, AgentProviderSource, SessionFileFormat, agent_provider,
    agent_providers,
};

#[cfg(feature = "discovery")]
pub(crate) use agent::DiscoverableProvider;
