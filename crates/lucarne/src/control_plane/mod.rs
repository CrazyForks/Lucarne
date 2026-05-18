//! Core control-plane state for durable agent session coordination.

mod activation;
mod commands;
mod fork;
mod history_replay;
mod interventions;
mod state;
mod status;
mod store;
mod subagents;
mod turn_queue;
mod types;

pub use activation::{ActivationCheck, ActivationPlan, ActivationRequest};
pub use commands::{
    command_from_catalog, command_help_requested, command_usage, plan_command_invocation,
    CommandCallbackRecord, CommandCallbackToken, CommandInvocationPlan, CommandPlanError,
};
pub use fork::{
    resolve_fork_session_refs, ForkSessionResolution, ForkSessionResolutionError,
    ForkWorkspaceResult, ForkWorkspaceSession,
};
pub use history_replay::{
    HistoryOlderCallbackRecord, HistoryOlderCallbackToken, HistoryReplayRecord,
    HistoryReplayTurnRecord,
};
pub use interventions::{InterventionCallbackRecord, InterventionCallbackToken};
pub use state::{ControlPlaneError, ControlPlanePersistenceEntity, ControlPlaneState};
pub use status::build_status_snapshot;
pub use store::{ControlPlaneSqliteStore, ControlPlaneStoreError};
pub use subagents::{
    SubAgentActionRecord, SubAgentCallbackRecord, SubAgentCallbackToken, SubAgentLinkRecord,
    SubAgentState,
};
pub use turn_queue::{QueuedTurn, TurnAdmission, TurnPermit, TurnScheduler};
pub use types::*;
