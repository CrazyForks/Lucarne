use async_trait::async_trait;

use crate::agent_runtime::{AgentCommandResult, KnownAgentProvider};

use super::{
    CloseWorkspaceRequest, CoreEventReceiver, HistoryPage, InterruptTurnRequest,
    InvokeCommandRequest, LiveWorkspace, OpenWorkspaceRequest, ProviderCatalogEntry,
    ResolvePermissionRequest, ResumeWorkspaceRequest, RunDueScheduledTasksRequest,
    RunScheduledTasksReport, SubmitTurnRequest, SubmittedTurn, UpsertScheduledTaskRequest,
    WorkspaceSummary,
};
use crate::control_plane::ScheduledTaskRecord;

#[async_trait]
pub trait DaemonApi: Send + Sync {
    fn providers(&self) -> Vec<KnownAgentProvider>;
    fn provider_catalog(&self) -> Vec<ProviderCatalogEntry>;
    fn list_sessions(&self) -> Vec<WorkspaceSummary>;
    fn list_history(&self, offset: usize, limit: usize) -> HistoryPage;
    fn watch_events(&self) -> CoreEventReceiver;

    async fn open(&self, req: OpenWorkspaceRequest) -> Result<LiveWorkspace, super::CoreError>;
    async fn resume(&self, req: ResumeWorkspaceRequest) -> Result<LiveWorkspace, super::CoreError>;
    async fn submit(&self, req: SubmitTurnRequest) -> Result<SubmittedTurn, super::CoreError>;
    async fn upsert_scheduled_task(
        &self,
        req: UpsertScheduledTaskRequest,
    ) -> Result<ScheduledTaskRecord, super::CoreError>;
    async fn run_due_scheduled_tasks(
        &self,
        req: RunDueScheduledTasksRequest,
    ) -> Result<RunScheduledTasksReport, super::CoreError>;
    async fn interrupt(&self, req: InterruptTurnRequest) -> Result<(), super::CoreError>;
    async fn resolve(&self, req: ResolvePermissionRequest) -> Result<(), super::CoreError>;
    async fn invoke_command(
        &self,
        req: InvokeCommandRequest,
    ) -> Result<AgentCommandResult, super::CoreError>;
    async fn close(&self, req: CloseWorkspaceRequest) -> Result<(), super::CoreError>;
}
