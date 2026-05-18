use super::state::ControlPlaneState;
use super::types::{CommandCompletionPolicy, ProviderSessionId, Revision, WorkspaceId};
use crate::agent_runtime::{
    AgentCommand, AgentCommandCatalog, AgentCommandCompletion, AgentCommandInput,
    AgentCommandInvocation, AgentCommandSource,
};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CommandCallbackToken(SmolStr);

impl CommandCallbackToken {
    pub fn new(value: impl Into<SmolStr>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandCallbackRecord {
    pub token: CommandCallbackToken,
    pub workspace_id: WorkspaceId,
    pub workspace_revision: Revision,
    pub provider_session_id: Option<ProviderSessionId>,
    pub catalog_revision: Revision,
    pub command_name: SmolStr,
    pub args: Option<SmolStr>,
    pub values: serde_json::Value,
    pub created_at: SystemTime,
}

impl CommandCallbackRecord {
    pub fn callback_payload(&self) -> String {
        format!("agentcmd:c:{}", self.token.as_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandInvocationPlan {
    pub name: SmolStr,
    pub args: Option<SmolStr>,
    pub values: serde_json::Value,
    pub source: AgentCommandSource,
    pub catalog_revision: Revision,
    pub completion_policy: CommandCompletionPolicy,
}

impl CommandInvocationPlan {
    pub fn invocation(&self) -> AgentCommandInvocation {
        AgentCommandInvocation {
            name: self.name.clone(),
            args: self.args.clone(),
            values: self.values.clone(),
            source: self.source,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandPlanError {
    Unsupported { name: SmolStr },
    MissingRequiredArgs { name: SmolStr, label: SmolStr },
}

pub fn plan_command_invocation(
    catalog: &AgentCommandCatalog,
    name: &str,
    args: &str,
    values: serde_json::Value,
) -> Result<CommandInvocationPlan, CommandPlanError> {
    let normalized_name = normalize_command_name(name);
    let Some(command) = catalog_command(catalog, normalized_name.as_str()) else {
        return Err(CommandPlanError::Unsupported {
            name: normalized_name,
        });
    };
    let args = args.trim();
    if args.is_empty() {
        if let Some(label) = required_command_args(command) {
            return Err(CommandPlanError::MissingRequiredArgs {
                name: normalized_name,
                label: label.into(),
            });
        }
    }
    Ok(CommandInvocationPlan {
        name: normalized_name,
        args: (!args.is_empty()).then(|| args.to_string().into()),
        values,
        source: command.source,
        catalog_revision: Revision::new(catalog.revision),
        completion_policy: command_completion_policy(command.completion),
    })
}

pub fn command_help_requested(args: &str) -> bool {
    args.trim().eq_ignore_ascii_case("help")
}

pub fn command_from_catalog<'a>(
    catalog: &'a AgentCommandCatalog,
    name: &str,
) -> Option<&'a AgentCommand> {
    catalog_command(catalog, normalize_command_name(name).as_str())
}

pub fn command_usage(command: &AgentCommand) -> String {
    match &command.input {
        AgentCommandInput::None => format!("/{}", command.name),
        AgentCommandInput::Text { label, required } => {
            let label = label.trim();
            if label.is_empty() {
                format!("/{}", command.name)
            } else if *required {
                format!("/{} <{}>", command.name, label)
            } else {
                format!("/{} [{}]", command.name, label)
            }
        }
        AgentCommandInput::JsonSchema { .. } => format!("/{} <options>", command.name),
    }
}

fn normalize_command_name(name: &str) -> SmolStr {
    name.trim()
        .trim_start_matches('/')
        .to_ascii_lowercase()
        .into()
}

fn catalog_command<'a>(catalog: &'a AgentCommandCatalog, name: &str) -> Option<&'a AgentCommand> {
    catalog
        .commands
        .iter()
        .find(|command| command_supports_name(command, name))
}

fn command_supports_name(command: &AgentCommand, name: &str) -> bool {
    command.name.as_str() == name || command.aliases.iter().any(|alias| alias.as_str() == name)
}

fn required_command_args(command: &AgentCommand) -> Option<&str> {
    match &command.input {
        AgentCommandInput::Text { label, required } if *required => Some(label.as_ref()),
        _ => None,
    }
}

fn command_completion_policy(completion: AgentCommandCompletion) -> CommandCompletionPolicy {
    match completion {
        AgentCommandCompletion::CommandResult => CommandCompletionPolicy::CommandResult,
        AgentCommandCompletion::TurnCompleted => CommandCompletionPolicy::TurnCompleted,
        AgentCommandCompletion::NoOutputAck => CommandCompletionPolicy::NoOutputAck,
        AgentCommandCompletion::ProviderIdle => CommandCompletionPolicy::ProviderIdle,
    }
}

impl ControlPlaneState {
    pub fn register_command_callback(
        &mut self,
        workspace_id: WorkspaceId,
        catalog_revision: Revision,
        command_name: impl Into<SmolStr>,
        args: Option<SmolStr>,
        values: serde_json::Value,
    ) -> Option<CommandCallbackRecord> {
        let workspace = self.workspaces.get(&workspace_id)?;
        self.next_command_callback += 1;
        let record = CommandCallbackRecord {
            token: CommandCallbackToken::new(format!("t{}", self.next_command_callback)),
            workspace_id,
            workspace_revision: workspace.revision,
            provider_session_id: workspace.active_provider_session_id.clone(),
            catalog_revision,
            command_name: command_name.into(),
            args,
            values,
            created_at: SystemTime::now(),
        };
        self.command_callbacks
            .insert(record.token.clone(), record.clone());
        Some(record)
    }

    pub fn resolve_command_callback(
        &self,
        token: &CommandCallbackToken,
    ) -> Option<CommandCallbackRecord> {
        self.command_callbacks.get(token).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_usage_renders_required_text_args() {
        let command = AgentCommand {
            name: "report".into(),
            description: Some("Show provider report.".into()),
            aliases: vec!["r".into()],
            source: AgentCommandSource::ProviderNative,
            input: AgentCommandInput::Text {
                label: "scope".into(),
                required: true,
            },
            completion: AgentCommandCompletion::ProviderIdle,
        };

        assert_eq!(command_usage(&command), "/report <scope>");
    }

    #[test]
    fn plan_command_invocation_preserves_catalog_source() {
        let catalog = AgentCommandCatalog {
            commands: vec![AgentCommand {
                name: "fork".into(),
                description: None,
                aliases: Vec::new(),
                source: AgentCommandSource::ProviderNative,
                input: AgentCommandInput::None,
                completion: AgentCommandCompletion::CommandResult,
            }],
            complete: true,
            revision: 7,
        };

        let plan = plan_command_invocation(&catalog, "/fork", "", serde_json::Value::Null).unwrap();

        assert_eq!(plan.source, AgentCommandSource::ProviderNative);
        assert_eq!(plan.catalog_revision, Revision::new(7));
        assert_eq!(
            plan.completion_policy,
            CommandCompletionPolicy::CommandResult
        );
    }
}
