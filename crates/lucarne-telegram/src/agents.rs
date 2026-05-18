//! Presentation helpers for daemon-reported agent providers.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEntry {
    pub display_name: String,
    pub provider_id: String,
    /// Provider command label shown to the user. Execution is owned by the daemon.
    pub command: String,
    /// True when the daemon reported the provider as currently available.
    pub available: bool,
}
