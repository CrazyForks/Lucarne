use super::state::{ControlPlaneError, ControlPlaneState};
use super::types::{
    LiveInstanceId, LiveInstanceRecord, ProviderSessionId, ProviderSessionRecord, WorkspaceBinding,
    WorkspaceId,
};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForkSessionResolution {
    Resumable {
        source_ref: String,
        fork_ref: String,
    },
    LiveOnly {
        source_ref: String,
    },
}

impl ForkSessionResolution {
    pub fn source_ref(&self) -> &str {
        match self {
            Self::Resumable { source_ref, .. } | Self::LiveOnly { source_ref } => source_ref,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForkSessionResolutionError {
    MissingSourceRef,
    MissingForkRef,
    ReusedSourceSession,
}

pub fn resolve_fork_session_refs(
    result_source_ref: Option<String>,
    result_fork_ref: Option<String>,
    provider_ref_before: Option<String>,
    session_resume_ref: Option<String>,
    allow_live_only_fork: bool,
) -> Result<ForkSessionResolution, ForkSessionResolutionError> {
    let source_ref = result_source_ref
        .or(provider_ref_before)
        .or(session_resume_ref)
        .ok_or(ForkSessionResolutionError::MissingSourceRef)?;
    let Some(fork_ref) = result_fork_ref else {
        return if allow_live_only_fork {
            Ok(ForkSessionResolution::LiveOnly { source_ref })
        } else {
            Err(ForkSessionResolutionError::MissingForkRef)
        };
    };
    if source_ref == fork_ref {
        return Err(ForkSessionResolutionError::ReusedSourceSession);
    }
    Ok(ForkSessionResolution::Resumable {
        source_ref,
        fork_ref,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForkWorkspaceSession {
    pub source_workspace_id: WorkspaceId,
    pub fork_workspace_id: WorkspaceId,
    pub title: SmolStr,
    pub provider_session_id: Option<ProviderSessionId>,
    pub native_resume_ref: Option<SmolStr>,
    pub live_instance_id: Option<LiveInstanceId>,
    pub pid_or_handle: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForkWorkspaceResult {
    pub source_workspace: WorkspaceBinding,
    pub fork_workspace: WorkspaceBinding,
    pub provider_session: Option<ProviderSessionRecord>,
    pub live_instance: Option<LiveInstanceRecord>,
}

impl ControlPlaneState {
    pub fn fork_workspace_session(
        &mut self,
        request: ForkWorkspaceSession,
    ) -> Result<ForkWorkspaceResult, ControlPlaneError> {
        let source_workspace = self
            .get_workspace(&request.source_workspace_id)
            .cloned()
            .ok_or_else(|| {
                ControlPlaneError::MissingWorkspace(request.source_workspace_id.clone())
            })?;
        let (provider_session_id, native_resume_ref) = match (
            request.provider_session_id.clone(),
            request.native_resume_ref.clone(),
        ) {
            (Some(provider_session_id), Some(native_resume_ref)) => {
                (provider_session_id, native_resume_ref)
            }
            _ => {
                return Err(ControlPlaneError::NonResumableFork {
                    fork_workspace_id: request.fork_workspace_id,
                });
            }
        };

        let mut fork_workspace = WorkspaceBinding::new(
            request.fork_workspace_id.clone(),
            request.title,
            source_workspace.provider_id.clone(),
            source_workspace.project_path.clone(),
        );
        fork_workspace.worktree_ref = source_workspace.worktree_ref.clone();
        self.upsert_workspace(fork_workspace);

        let provider_session = self.upsert_provider_session(ProviderSessionRecord::new(
            provider_session_id.clone(),
            source_workspace.provider_id.clone(),
            native_resume_ref,
        ));
        self.activate_provider_session(request.fork_workspace_id.clone(), provider_session_id)?;

        let live_instance = match (request.live_instance_id, request.provider_session_id) {
            (Some(live_instance_id), Some(provider_session_id)) => {
                let live = self.attach_live_instance(
                    request.fork_workspace_id.clone(),
                    LiveInstanceRecord::new(
                        live_instance_id,
                        source_workspace.provider_id.clone(),
                        provider_session_id,
                        request.pid_or_handle,
                    ),
                )?;
                Some(live)
            }
            _ => None,
        };

        let fork_workspace = self
            .get_workspace(&request.fork_workspace_id)
            .cloned()
            .ok_or_else(|| ControlPlaneError::MissingWorkspace(request.fork_workspace_id))?;

        Ok(ForkWorkspaceResult {
            source_workspace,
            fork_workspace,
            provider_session: Some(provider_session),
            live_instance,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fork_resolution_preserves_source_and_rejects_reused_session() {
        let err = resolve_fork_session_refs(
            Some("source".into()),
            Some("source".into()),
            None,
            None,
            false,
        )
        .expect_err("same fork/source ref must be rejected");
        assert_eq!(err, ForkSessionResolutionError::ReusedSourceSession);
    }

    #[test]
    fn fork_resolution_allows_live_only_when_provider_has_no_ref_yet() {
        let resolved = resolve_fork_session_refs(None, None, Some("source".into()), None, true)
            .expect("live-only fork");
        assert_eq!(
            resolved,
            ForkSessionResolution::LiveOnly {
                source_ref: "source".into()
            }
        );
    }
}
