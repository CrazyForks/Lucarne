use serde::{Deserialize, Serialize};

use crate::agent_runtime::{
    AgentCommandCatalog, AgentForkResult, AgentForkTargetCatalog, AgentModelCatalog,
    AgentModelSelection, AgentPermissionCatalog, AgentPermissionSelection, AgentSkillCatalog,
    AgentStatus,
};
use smol_str::SmolStr;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandResultPayload {
    pub command: String,
    pub result: CommandResultData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum CommandResultData {
    Models(AgentModelCatalog),
    ModelChanged(AgentModelSelection),
    Permissions(AgentPermissionCatalog),
    PermissionsChanged(AgentPermissionSelection),
    Status(AgentStatus),
    Skills(AgentSkillCatalog),
    Forked(AgentForkResult),
    ForkTargets(AgentForkTargetCatalog),
    Commands(AgentCommandCatalog),
    Text { text: SmolStr },
}
