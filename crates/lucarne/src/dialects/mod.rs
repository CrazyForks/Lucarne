//! Module declarations for per-vendor dialects.

#[cfg(feature = "claude")]
pub mod claude;
#[cfg(feature = "claude")]
pub mod claude_transcript;
#[cfg(feature = "codex")]
pub mod codex;
#[cfg(feature = "copilot")]
pub mod copilot;
#[cfg(feature = "gemini")]
pub mod gemini;
#[cfg(feature = "pi")]
pub mod pi_rpc;
