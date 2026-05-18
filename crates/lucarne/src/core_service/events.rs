use tokio::sync::broadcast::{
    self,
    error::{RecvError as BroadcastRecvError, TryRecvError as BroadcastTryRecvError},
};
use tracing::{debug, warn};

use crate::{
    agent_runtime::Event as AgentEvent,
    control_plane::{TurnId, WorkspaceId},
};

#[derive(Debug, Clone, PartialEq)]
pub enum CoreEvent {
    WorkspaceChanged {
        workspace_id: WorkspaceId,
    },
    TimelineEvent {
        workspace_id: WorkspaceId,
        turn_id: Option<TurnId>,
        event: AgentEvent,
    },
    TurnStarted {
        workspace_id: WorkspaceId,
    },
    TurnCompleted {
        workspace_id: WorkspaceId,
    },
    TurnFailed {
        workspace_id: WorkspaceId,
        error: String,
    },
}

pub type CoreEventReceiver = tokio::sync::broadcast::Receiver<CoreEvent>;

pub struct CoreWorkspaceEventStream {
    workspace_id: WorkspaceId,
    events: CoreWorkspaceEventSource,
}

enum CoreWorkspaceEventSource {
    Workspace(broadcast::Receiver<AgentEvent>),
    Global(CoreEventReceiver),
}

impl CoreWorkspaceEventStream {
    pub fn new(workspace_id: WorkspaceId, events: CoreEventReceiver) -> Self {
        Self::from_source(workspace_id, CoreWorkspaceEventSource::Global(events))
    }

    pub(crate) fn from_workspace_events(
        workspace_id: WorkspaceId,
        events: broadcast::Receiver<AgentEvent>,
    ) -> Self {
        Self::from_source(workspace_id, CoreWorkspaceEventSource::Workspace(events))
    }

    fn from_source(workspace_id: WorkspaceId, events: CoreWorkspaceEventSource) -> Self {
        debug!(
            target: "lucarne::core_service::events",
            workspace_id = %workspace_id.as_str(),
            "workspace event stream created"
        );
        Self {
            workspace_id,
            events,
        }
    }

    pub async fn recv(&mut self) -> Result<AgentEvent, CoreWorkspaceEventRecvError> {
        match &mut self.events {
            CoreWorkspaceEventSource::Workspace(events) => match events.recv().await {
                Ok(event) => Ok(event),
                Err(err) => self.map_recv_error(err),
            },
            CoreWorkspaceEventSource::Global(events) => loop {
                match events.recv().await {
                    Ok(CoreEvent::TimelineEvent {
                        workspace_id,
                        turn_id: _,
                        event,
                    }) if workspace_id == self.workspace_id => return Ok(event),
                    Ok(_) => {}
                    Err(err) => return self.map_recv_error(err),
                }
            },
        }
    }

    pub fn try_recv(&mut self) -> Result<AgentEvent, CoreWorkspaceEventTryRecvError> {
        match &mut self.events {
            CoreWorkspaceEventSource::Workspace(events) => match events.try_recv() {
                Ok(event) => Ok(event),
                Err(err) => self.map_try_recv_error(err),
            },
            CoreWorkspaceEventSource::Global(events) => loop {
                match events.try_recv() {
                    Ok(CoreEvent::TimelineEvent {
                        workspace_id,
                        turn_id: _,
                        event,
                    }) if workspace_id == self.workspace_id => return Ok(event),
                    Ok(_) => {}
                    Err(err) => return self.map_try_recv_error(err),
                }
            },
        }
    }

    fn map_recv_error(
        &self,
        err: BroadcastRecvError,
    ) -> Result<AgentEvent, CoreWorkspaceEventRecvError> {
        match err {
            BroadcastRecvError::Closed => {
                debug!(
                    target: "lucarne::core_service::events",
                    workspace_id = %self.workspace_id.as_str(),
                    "workspace event stream closed"
                );
                Err(CoreWorkspaceEventRecvError::Closed)
            }
            BroadcastRecvError::Lagged(skipped) => {
                warn!(
                    target: "lucarne::core_service::events",
                    workspace_id = %self.workspace_id.as_str(),
                    skipped,
                    "workspace event stream lagged"
                );
                Err(CoreWorkspaceEventRecvError::Lagged(skipped))
            }
        }
    }

    fn map_try_recv_error(
        &self,
        err: BroadcastTryRecvError,
    ) -> Result<AgentEvent, CoreWorkspaceEventTryRecvError> {
        match err {
            BroadcastTryRecvError::Empty => Err(CoreWorkspaceEventTryRecvError::Empty),
            BroadcastTryRecvError::Closed => {
                debug!(
                    target: "lucarne::core_service::events",
                    workspace_id = %self.workspace_id.as_str(),
                    "workspace event stream closed"
                );
                Err(CoreWorkspaceEventTryRecvError::Closed)
            }
            BroadcastTryRecvError::Lagged(skipped) => {
                warn!(
                    target: "lucarne::core_service::events",
                    workspace_id = %self.workspace_id.as_str(),
                    skipped,
                    "workspace event stream lagged"
                );
                Err(CoreWorkspaceEventTryRecvError::Lagged(skipped))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreWorkspaceEventRecvError {
    Closed,
    Lagged(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreWorkspaceEventTryRecvError {
    Empty,
    Closed,
    Lagged(u64),
}
