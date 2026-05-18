pub mod common;

use common::agent_runtime::{
    assistant_texts, collect_until_closed, open_request, recv_event, runtime, take_events,
};
use lucarne::agent_runtime::{
    AgentCommandCatalog, AgentCommandInput, AgentCommandInvocation, AgentCommandResult,
    AgentCommandResultData, AgentCommandSource, AgentEventStream, AgentForkResult,
    AgentForkSelection, AgentForkTarget, AgentForkTargetCatalog, AgentInput, AgentModelCatalog,
    AgentModelOption, AgentModelSelection, AgentPermissionSelection, AgentReasoningOption,
    AgentSession, Event, InstanceId, InterventionResponse, MessageRole, SessionId,
};
use lucarne::ProviderId;
use serde_json::Value;
use smol_str::SmolStr;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};

#[derive(Debug)]
struct TypedCommandSession {
    instance_id: InstanceId,
    session_id: SessionId,
    event_take_count: Arc<AtomicUsize>,
    selected_fork_target: Arc<Mutex<Option<SmolStr>>>,
}

impl TypedCommandSession {
    fn new() -> Self {
        Self {
            instance_id: InstanceId("typed-command-instance".into()),
            session_id: SessionId("typed-command-session".into()),
            event_take_count: Arc::new(AtomicUsize::new(0)),
            selected_fork_target: Arc::new(Mutex::new(None)),
        }
    }
}

#[async_trait::async_trait]
impl AgentSession for TypedCommandSession {
    fn id(&self) -> &SessionId {
        &self.session_id
    }

    fn instance_id(&self) -> &InstanceId {
        &self.instance_id
    }

    fn provider_id(&self) -> ProviderId {
        ProviderId::from_static("typed-test")
    }

    async fn submit(&self, _input: AgentInput) -> Result<(), lucarne::agent_runtime::AgentError> {
        Ok(())
    }

    async fn list_models(&self) -> Result<AgentModelCatalog, lucarne::agent_runtime::AgentError> {
        Ok(AgentModelCatalog {
            current_model: Some("gpt-5.5".into()),
            current_reasoning: Some("xhigh".into()),
            models: vec![AgentModelOption {
                id: "gpt-5.5".into(),
                display_name: Some("GPT-5.5".into()),
                description: None,
                supported_reasoning: ["low", "medium", "high", "xhigh"]
                    .into_iter()
                    .map(|value| AgentReasoningOption {
                        value: value.into(),
                        description: None,
                        is_default: None,
                    })
                    .collect(),
            }],
        })
    }

    async fn list_fork_targets(
        &self,
    ) -> Result<AgentForkTargetCatalog, lucarne::agent_runtime::AgentError> {
        Ok(AgentForkTargetCatalog {
            targets: vec![
                AgentForkTarget {
                    id: "user-1".into(),
                    label: Some("First prompt".into()),
                    description: Some("turn 1".into()),
                },
                AgentForkTarget {
                    id: "user-2".into(),
                    label: Some("Second prompt".into()),
                    description: Some("turn 2".into()),
                },
            ],
        })
    }

    async fn fork(
        &self,
        selection: AgentForkSelection,
    ) -> Result<AgentForkResult, lucarne::agent_runtime::AgentError> {
        *self
            .selected_fork_target
            .lock()
            .expect("selected fork target lock") = Some(selection.target_id.clone());
        Ok(AgentForkResult {
            session_ref: Some(lucarne::agent_runtime::SessionRef("forked-session".into())),
            source_session_ref: Some(lucarne::agent_runtime::SessionRef(selection.target_id)),
        })
    }

    async fn invoke_command(
        &self,
        command: AgentCommandInvocation,
    ) -> Result<AgentCommandResult, lucarne::agent_runtime::AgentError> {
        Ok(AgentCommandResult {
            name: command.name,
            source: AgentCommandSource::ProviderNative,
            data: AgentCommandResultData::Json(serde_json::json!({
                "ok": true,
                "assistant_output": null
            })),
        })
    }

    async fn interrupt(&self) -> Result<(), lucarne::agent_runtime::AgentError> {
        Ok(())
    }

    async fn resolve(
        &self,
        _req_id: &str,
        _response: InterventionResponse,
    ) -> Result<(), lucarne::agent_runtime::AgentError> {
        Ok(())
    }

    async fn take_events(&self) -> Result<AgentEventStream, lucarne::agent_runtime::AgentError> {
        self.event_take_count.fetch_add(1, Ordering::Relaxed);
        let (_tx, rx) = mpsc::channel(1);
        Ok(rx)
    }

    async fn close(&self) -> Result<(), lucarne::agent_runtime::AgentError> {
        Ok(())
    }
}

fn command_names(commands: &[lucarne::agent_runtime::AgentCommand]) -> Vec<String> {
    commands
        .iter()
        .map(|command| command.name.to_string())
        .collect()
}

fn command_source(
    catalog: &lucarne::agent_runtime::AgentCommandCatalog,
    name: &str,
) -> Option<AgentCommandSource> {
    catalog
        .commands
        .iter()
        .find(|command| command.name.as_str() == name)
        .map(|command| command.source)
}

fn assert_provider_native(catalog: &lucarne::agent_runtime::AgentCommandCatalog, name: &str) {
    assert_eq!(
        command_source(catalog, name),
        Some(AgentCommandSource::ProviderNative),
        "expected /{name} to come from the provider catalog"
    );
}

fn command<'a>(
    catalog: &'a lucarne::agent_runtime::AgentCommandCatalog,
    name: &str,
) -> &'a lucarne::agent_runtime::AgentCommand {
    catalog
        .commands
        .iter()
        .find(|command| command.name.as_str() == name)
        .unwrap_or_else(|| panic!("missing command {name} in {catalog:?}"))
}

async fn invoke_and_recv_assistant_text(
    session: &dyn AgentSession,
    events: &mut AgentEventStream,
    command: AgentCommandInvocation,
) -> String {
    session
        .invoke_command(command)
        .await
        .expect("invoke agent command");
    let mut text = None;
    loop {
        match recv_event(events).await {
            Event::Message(message)
                if message.role == MessageRole::Assistant && !message.text.trim().is_empty() =>
            {
                text = Some(message.text.to_string());
            }
            Event::TurnCompleted(_) if text.is_some() => return text.expect("assistant text"),
            Event::TurnFailed(failed) => panic!("command failed: {failed:?}"),
            _ => {}
        }
    }
}

async fn list_complete_commands(session: &dyn AgentSession, context: &str) -> AgentCommandCatalog {
    for _ in 0..20 {
        let catalog = session
            .list_commands()
            .await
            .unwrap_or_else(|err| panic!("{context}: list commands failed: {err}"));
        if catalog.complete {
            return catalog;
        }
        sleep(Duration::from_millis(50)).await;
    }
    panic!("{context}: command catalog did not become complete");
}

#[tokio::test]
async fn list_model_returns_typed_catalog_not_stream_side_effect() {
    let session = TypedCommandSession::new();

    let catalog = session.list_models().await.expect("list typed models");

    assert_eq!(catalog.current_model.as_deref(), Some("gpt-5.5"));
    assert_eq!(catalog.current_reasoning.as_deref(), Some("xhigh"));
    assert_eq!(catalog.models.len(), 1);
    assert_eq!(catalog.models[0].id.as_str(), "gpt-5.5");
    assert_eq!(catalog.models[0].display_name.as_deref(), Some("GPT-5.5"));
    assert_eq!(
        catalog.models[0]
            .supported_reasoning
            .iter()
            .map(|option| option.value.as_str())
            .collect::<Vec<_>>(),
        vec!["low", "medium", "high", "xhigh"]
    );
    assert_eq!(session.event_take_count.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn list_fork_targets_returns_typed_targets_before_fork() {
    let session = TypedCommandSession::new();

    let catalog = session
        .list_fork_targets()
        .await
        .expect("list typed fork targets");
    assert_eq!(
        catalog
            .targets
            .iter()
            .map(|target| target.id.as_str())
            .collect::<Vec<_>>(),
        vec!["user-1", "user-2"]
    );
    assert_eq!(
        *session
            .selected_fork_target
            .lock()
            .expect("selected fork target lock"),
        None
    );

    let result = session
        .fork(AgentForkSelection {
            target_id: catalog.targets[1].id.clone(),
        })
        .await
        .expect("fork selected typed target");

    assert_eq!(
        result
            .session_ref
            .as_ref()
            .map(|session_ref| session_ref.0.as_str()),
        Some("forked-session")
    );
    assert_eq!(
        session
            .selected_fork_target
            .lock()
            .expect("selected fork target lock")
            .as_deref(),
        Some("user-2")
    );
}

#[tokio::test]
async fn invoke_command_returns_structured_result_for_no_assistant_output() {
    let session = TypedCommandSession::new();

    let result = session
        .invoke_command(AgentCommandInvocation {
            name: "structured".into(),
            args: None,
            values: Value::Null,
            source: AgentCommandSource::ProviderNative,
        })
        .await
        .expect("invoke typed command");

    assert_eq!(result.name.as_str(), "structured");
    assert_eq!(result.source, AgentCommandSource::ProviderNative);
    assert_eq!(
        result.data,
        AgentCommandResultData::Json(serde_json::json!({
            "ok": true,
            "assistant_output": null
        }))
    );
    assert_eq!(session.event_take_count.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn agent_runtime_commands_claude_lists_and_invokes_native_commands() {
    let runtime = runtime();
    let session = runtime
        .open("claude", open_request("claude", "commands.fixture", None))
        .await
        .expect("open claude command session");

    let catalog = list_complete_commands(session.as_ref(), "claude command catalog").await;
    assert!(catalog.complete);
    assert_eq!(
        command_names(&catalog.commands),
        vec!["context", "review", "usage"]
    );
    assert_provider_native(&catalog, "context");
    assert_eq!(command(&catalog, "context").input, AgentCommandInput::None);
    assert_eq!(
        catalog
            .commands
            .iter()
            .find(|command| command.name.as_str() == "usage")
            .and_then(|command| command.description.as_deref()),
        Some("Show the total cost and duration of the current session")
    );
    assert_eq!(
        command(&catalog, "usage")
            .aliases
            .iter()
            .map(|alias| alias.as_str())
            .collect::<Vec<_>>(),
        ["cost", "stats"]
    );

    let mut events = take_events(session.as_ref()).await;
    let mut texts = Vec::new();
    for command in [
        AgentCommandInvocation {
            name: "context".into(),
            args: None,
            values: Value::Null,
            source: AgentCommandSource::ProviderNative,
        },
        AgentCommandInvocation {
            name: "review".into(),
            args: Some("focus on runtime".into()),
            values: Value::Null,
            source: AgentCommandSource::ProviderNative,
        },
        AgentCommandInvocation {
            name: "usage".into(),
            args: None,
            values: Value::Null,
            source: AgentCommandSource::ProviderNative,
        },
        AgentCommandInvocation {
            name: "cost".into(),
            args: None,
            values: Value::Null,
            source: AgentCommandSource::ProviderNative,
        },
    ] {
        texts.push(invoke_and_recv_assistant_text(session.as_ref(), &mut events, command).await);
    }

    assert_eq!(texts, vec!["CTX_OK", "REVIEW_OK", "USAGE_OK", "COST_OK"]);

    let status = session.status().await.expect("claude status");
    assert_eq!(status.version.as_deref(), Some("2.1.119"));
    assert_eq!(status.model.as_deref(), Some("default"));
    assert_eq!(status.reasoning.as_deref(), Some("medium"));
    assert_eq!(status.directory.as_deref(), Some("/tmp/project"));
    assert_eq!(status.permissions.as_deref(), Some("default"));
    assert_eq!(status.session_id.as_deref(), Some("sess-commands"));
    let tokens = status.tokens.as_ref().expect("claude status tokens");
    assert_eq!(tokens.input_tokens, Some(1900));
    assert_eq!(tokens.output_tokens, Some(387));
    let context = status.context.as_ref().expect("claude status context");
    assert_eq!(context.used_tokens, Some(14000));
    assert_eq!(context.max_tokens, Some(205000));
    assert_eq!(context.percent_used, Some(7));
    assert_eq!(status.compactions, Some(0));

    let models = session.list_models().await.expect("claude models");
    assert_eq!(models.current_model.as_deref(), Some("default"));
    assert_eq!(
        models
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["default", "haiku"]
    );

    let skills = session.list_skills().await.expect("claude skills");
    assert_eq!(
        skills
            .skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        ["update-config", "frontend-design"]
    );

    let fork_targets = session
        .list_fork_targets()
        .await
        .expect("claude fork targets");
    assert_eq!(
        fork_targets
            .targets
            .iter()
            .map(|target| target.id.as_str())
            .collect::<Vec<_>>(),
        ["user_command", "user_review", "user_usage", "user_cost"]
    );

    session.new().await.expect("claude new");

    let err = session
        .invoke_command(AgentCommandInvocation {
            name: "fork".into(),
            args: Some("user_command".into()),
            values: Value::Null,
            source: AgentCommandSource::ProviderNative,
        })
        .await
        .expect_err("fixed fork should not be exposed through native /commands");
    assert!(
        err.message.contains("unsupported command \"fork\""),
        "{err:?}"
    );
}

#[tokio::test]
async fn agent_runtime_commands_claude_uses_reload_plugins_commands_when_initialize_has_none() {
    let runtime = runtime();
    let session = runtime
        .open(
            "claude",
            open_request("claude", "commands_reload_plugins.fixture", None),
        )
        .await
        .expect("open claude reload_plugins command session");

    let catalog = list_complete_commands(session.as_ref(), "claude reload_plugins catalog").await;
    assert!(catalog.complete);
    assert_eq!(command_names(&catalog.commands), vec!["context", "review"]);
    assert_eq!(
        command_source(&catalog, "review"),
        Some(AgentCommandSource::ProviderNative)
    );
}

#[tokio::test]
async fn agent_runtime_commands_claude_invokes_native_name_even_when_it_matches_fixed_command() {
    let runtime = runtime();
    let session = runtime
        .open(
            "claude",
            open_request("claude", "commands_native_status.fixture", None),
        )
        .await
        .expect("open claude native status command session");

    let catalog = list_complete_commands(session.as_ref(), "claude native status catalog").await;
    assert_eq!(command_names(&catalog.commands), vec!["status"]);
    assert_provider_native(&catalog, "status");

    let mut events = take_events(session.as_ref()).await;
    let text = invoke_and_recv_assistant_text(
        session.as_ref(),
        &mut events,
        AgentCommandInvocation {
            name: "status".into(),
            args: None,
            values: Value::Null,
            source: AgentCommandSource::ProviderNative,
        },
    )
    .await;

    assert_eq!(text, "NATIVE_STATUS_OK");
}

#[tokio::test]
async fn agent_runtime_commands_gemini_lists_and_invokes_acp_commands() {
    let runtime = runtime();
    let session = runtime
        .open("gemini", open_request("gemini", "commands.fixture", None))
        .await
        .expect("open gemini command session");

    let catalog = list_complete_commands(session.as_ref(), "gemini command catalog").await;
    assert_eq!(command_names(&catalog.commands), vec!["help", "about"]);
    assert_eq!(
        catalog.commands[0].description.as_deref(),
        Some("Show available commands")
    );
    assert_provider_native(&catalog, "help");

    let mut events = take_events(session.as_ref()).await;
    let help = invoke_and_recv_assistant_text(
        session.as_ref(),
        &mut events,
        AgentCommandInvocation {
            name: "help".into(),
            args: None,
            values: Value::Null,
            source: AgentCommandSource::ProviderNative,
        },
    )
    .await;
    assert_eq!(help, "GEMINI_HELP");

    let models = session.list_models().await.expect("gemini models");
    assert_eq!(models.current_model.as_deref(), Some("gemini-2.5-flash"));
    assert_eq!(
        models
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["gemini-2.5-flash", "gemini-2.5-pro"]
    );
    let status = session
        .set_model(AgentModelSelection {
            model: "gemini-2.5-pro".into(),
            reasoning: None,
        })
        .await
        .expect("set gemini model");
    assert_eq!(status.model.as_deref(), Some("gemini-2.5-pro"));

    let permissions = session
        .list_permissions()
        .await
        .expect("gemini permissions");
    assert_eq!(permissions.current_mode.as_deref(), Some("default"));
    assert_eq!(
        permissions
            .modes
            .iter()
            .map(|mode| mode.id.as_str())
            .collect::<Vec<_>>(),
        ["default", "autoEdit"]
    );
    let status = session
        .set_permissions(AgentPermissionSelection {
            mode: "autoEdit".into(),
        })
        .await
        .expect("set gemini permissions");
    assert_eq!(status.permissions.as_deref(), Some("autoEdit"));
}

#[tokio::test]
async fn agent_runtime_commands_gemini_invokes_native_name_even_when_it_matches_fixed_command() {
    let runtime = runtime();
    let session = runtime
        .open(
            "gemini",
            open_request("gemini", "commands_native_model.fixture", None),
        )
        .await
        .expect("open gemini native model command session");

    let catalog = list_complete_commands(session.as_ref(), "gemini native model catalog").await;
    assert_eq!(command_names(&catalog.commands), vec!["model"]);
    assert_provider_native(&catalog, "model");

    let mut events = take_events(session.as_ref()).await;
    let text = invoke_and_recv_assistant_text(
        session.as_ref(),
        &mut events,
        AgentCommandInvocation {
            name: "model".into(),
            args: None,
            values: Value::Null,
            source: AgentCommandSource::ProviderNative,
        },
    )
    .await;

    assert_eq!(text, "NATIVE_MODEL_OK");
}

#[tokio::test]
async fn agent_runtime_commands_codex_maps_review_to_app_server_method() {
    let runtime = runtime();
    let session = runtime
        .open("codex", open_request("codex", "commands.fixture", None))
        .await
        .expect("open codex command session");

    let catalog = session.list_commands().await.expect("list codex commands");
    assert!(catalog.complete);
    assert_provider_native(&catalog, "review");

    let mut events = take_events(session.as_ref()).await;
    session
        .invoke_command(AgentCommandInvocation {
            name: "review".into(),
            args: None,
            values: Value::Null,
            source: AgentCommandSource::ProviderNative,
        })
        .await
        .expect("invoke codex review command");

    let events = collect_until_closed(&mut events).await;
    assert!(events.iter().any(|event| matches!(
        event,
        Event::Message(message)
            if message.role == MessageRole::Assistant && message.text.as_str() == "REVIEW_OK"
    )));
}

#[tokio::test]
async fn agent_runtime_commands_codex_lists_only_provider_backed_commands() {
    let runtime = runtime();
    let session = runtime
        .open("codex", open_request("codex", "commands.fixture", None))
        .await
        .expect("open codex command session");

    let catalog = session.list_commands().await.expect("list codex commands");
    assert!(catalog.complete);
    let names = command_names(&catalog.commands);
    assert_eq!(
        names,
        vec![
            "fast", "hooks", "review", "rename", "compact", "goal", "mcp", "apps", "plugins",
            "stop"
        ]
    );
    for native in &names {
        assert_provider_native(&catalog, native);
    }
    for fixed in [
        "model",
        "permissions",
        "skills",
        "status",
        "new",
        "fork",
        "quit",
    ] {
        assert!(
            !names.iter().any(|name| name == fixed),
            "session-trait command /{fixed} must not appear in provider /commands: {names:?}"
        );
    }
    for not_direct in ["side", "plan", "init", "resume", "copy", "raw", "diff"] {
        assert!(
            !names.iter().any(|name| name == not_direct),
            "non-direct command /{not_direct} must not appear in provider /commands: {names:?}"
        );
    }
    assert_eq!(
        command(&catalog, "goal").input,
        AgentCommandInput::Text {
            label: "objective|pause|resume|clear".into(),
            required: false,
        }
    );
    assert_eq!(
        command(&catalog, "rename").input,
        AgentCommandInput::Text {
            label: "name".into(),
            required: true,
        }
    );
    assert_eq!(
        command(&catalog, "stop")
            .aliases
            .iter()
            .map(|alias| alias.as_str())
            .collect::<Vec<_>>(),
        ["clean"]
    );
    assert_eq!(command(&catalog, "stop").input, AgentCommandInput::None);
}

#[tokio::test]
async fn journey_55_command_source_dispatches_system_commands_without_marker() {
    let runtime = runtime();
    let session = runtime
        .open(
            "codex",
            open_request("codex", "journey_55_model_system_command.fixture", None),
        )
        .await
        .expect("open codex command session");

    let mut _events = take_events(session.as_ref()).await;

    let model_status = session
        .set_model(AgentModelSelection {
            model: "gpt-5.4".into(),
            reasoning: None,
        })
        .await
        .expect("set model through typed system command path");
    assert_eq!(model_status.model.as_deref(), Some("gpt-5.4"));
}

#[test]
fn dialect_command_boundary_has_no_adapter_marker_system() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dialect_source =
        std::fs::read_to_string(manifest_dir.join("src/dialect.rs")).expect("read dialect source");
    let runtime_source =
        std::fs::read_to_string(manifest_dir.join("src/runtime.rs")).expect("read runtime source");
    let agent_session_source =
        std::fs::read_to_string(manifest_dir.join("src/agent_runtime/session.rs"))
            .expect("read agent runtime session source");

    for forbidden in [
        "AgentAdapterCommands",
        "__lucarne_adapter_command",
        "mark_adapter_command",
        "is_adapter_command_invocation",
        "dispatch_adapter_command",
        "encode_command_invocation",
    ] {
        assert!(
            !dialect_source.contains(forbidden),
            "dialect boundary must not contain {forbidden}"
        );
        assert!(
            !runtime_source.contains(forbidden),
            "runtime command dispatch must not contain {forbidden}"
        );
        assert!(
            !agent_session_source.contains(forbidden),
            "agent runtime facade must not contain {forbidden}"
        );
    }

    assert!(
        dialect_source.contains("fn handle_system_command"),
        "Dialect must expose explicit system-command dispatch"
    );
    assert!(
        dialect_source.contains("fn handle_native_command"),
        "Dialect must expose explicit native-command dispatch"
    );
}

#[test]
fn out_frame_boundary_has_no_rpc_specific_stdin_variants() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dialect_source =
        std::fs::read_to_string(manifest_dir.join("src/dialect.rs")).expect("read dialect source");
    let runtime_source =
        std::fs::read_to_string(manifest_dir.join("src/runtime.rs")).expect("read runtime source");

    for forbidden in ["RpcRequest", "RpcResponse"] {
        assert!(
            !dialect_source.contains(forbidden),
            "OutFrame must not carry RPC-specific stdin variants: {forbidden}"
        );
        assert!(
            !runtime_source.contains(forbidden),
            "runtime stdin dispatch must not branch on {forbidden}"
        );
    }
    assert!(
        dialect_source.contains("Stdin(Vec<u8>)"),
        "OutFrame should keep a single byte-carrying stdin variant"
    );
}

#[test]
fn session_config_boundary_has_single_session_params_carrier() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let adapter_source =
        std::fs::read_to_string(manifest_dir.join("src/adapter.rs")).expect("read adapter source");
    let dialect_source =
        std::fs::read_to_string(manifest_dir.join("src/dialect.rs")).expect("read dialect source");
    let runtime_source =
        std::fs::read_to_string(manifest_dir.join("src/runtime.rs")).expect("read runtime source");

    assert!(
        dialect_source.contains("pub struct SessionParams"),
        "dialect boundary should expose SessionParams"
    );
    for forbidden in ["pub struct StartRequest", "pub struct SessionCfg"] {
        assert!(
            !adapter_source.contains(forbidden),
            "adapter boundary must not expose {forbidden}"
        );
        assert!(
            !dialect_source.contains(forbidden),
            "dialect boundary must not expose {forbidden}"
        );
    }
    assert!(
        !runtime_source.contains("session_cfg"),
        "runtime config should carry SessionParams without a SessionCfg remap"
    );
}

#[test]
fn agent_registry_boundary_has_single_descriptor_source() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let registry_source = std::fs::read_to_string(manifest_dir.join("src/agent_registry.rs"))
        .expect("read agent registry source");
    let adapters_source = std::fs::read_to_string(manifest_dir.join("src/adapters/mod.rs"))
        .expect("read adapters source");
    let catalog_source = std::fs::read_to_string(manifest_dir.join("src/agent_runtime/catalog.rs"))
        .expect("read catalog source");
    let history_source = std::fs::read_to_string(manifest_dir.join("src/history/mod.rs"))
        .expect("read history source");

    assert!(
        registry_source.contains("#[distributed_slice]"),
        "agent registry must declare a linkme distributed slice"
    );
    assert!(
        registry_source.contains("ALL_AGENT_DESCRIPTORS"),
        "agent registry must expose the single descriptor source"
    );
    for forbidden in [
        "DEFAULT_AGENT_PROVIDERS",
        "SUPPORTED_HISTORY_PROVIDERS",
        "adapters.push(",
        "match provider.id",
    ] {
        assert!(
            !adapters_source.contains(forbidden),
            "adapters registry must not contain duplicated registry pattern {forbidden}"
        );
        assert!(
            !catalog_source.contains(forbidden),
            "runtime catalog must not contain duplicated registry pattern {forbidden}"
        );
        assert!(
            !history_source.contains(forbidden),
            "history must not contain duplicated registry pattern {forbidden}"
        );
    }
}

#[tokio::test]
async fn agent_runtime_commands_codex_maps_first_batch_control_commands() {
    let runtime = runtime();
    let session = runtime
        .open(
            "codex",
            open_request("codex", "commands_control.fixture", None),
        )
        .await
        .expect("open codex command session");
    let mut events = take_events(session.as_ref()).await;

    let rename = invoke_and_recv_assistant_text(
        session.as_ref(),
        &mut events,
        AgentCommandInvocation {
            name: "rename".into(),
            args: Some("release notes".into()),
            values: Value::Null,
            source: AgentCommandSource::ProviderNative,
        },
    )
    .await;
    let stop = invoke_and_recv_assistant_text(
        session.as_ref(),
        &mut events,
        AgentCommandInvocation {
            name: "stop".into(),
            args: None,
            values: Value::Null,
            source: AgentCommandSource::ProviderNative,
        },
    )
    .await;

    assert_eq!(rename, "Renamed thread.");
    assert_eq!(stop, "Stopping all background terminals.");

    let status = session.status().await.expect("codex status");
    assert_eq!(status.model.as_deref(), Some("gpt-5.4"));
    assert_eq!(status.reasoning.as_deref(), Some("low"));
    assert_eq!(status.directory.as_deref(), Some("/tmp/project"));
    assert_eq!(
        status.permissions.as_deref(),
        Some("Custom (workspace-write, on-request)")
    );
    assert_eq!(
        status.agents_md.as_deref(),
        Some("/Users/era/.codex/AGENTS.md")
    );
    assert_eq!(status.session_id.as_deref(), Some("codex-command-thread"));
    let tokens = status.tokens.as_ref().expect("codex status tokens");
    assert_eq!(tokens.input_tokens, Some(10700000));
    assert_eq!(tokens.output_tokens, Some(31300));
    let context = status.context.as_ref().expect("codex status context");
    assert_eq!(context.used_tokens, Some(14000));
    assert_eq!(context.max_tokens, Some(258400));
    assert_eq!(context.percent_used, Some(5));

    let models = session.list_models().await.expect("codex models");
    assert_eq!(
        models
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["gpt-5.5", "gpt-5.4"]
    );
    assert_eq!(
        models.models[0]
            .supported_reasoning
            .iter()
            .map(|reasoning| (reasoning.value.as_str(), reasoning.is_default))
            .collect::<Vec<_>>(),
        [
            ("low", Some(false)),
            ("medium", Some(true)),
            ("high", Some(false))
        ]
    );

    let status = session
        .set_model(AgentModelSelection {
            model: "gpt5.4".into(),
            reasoning: Some("high".into()),
        })
        .await
        .expect("set codex model");
    assert_eq!(status.model.as_deref(), Some("gpt-5.4"));
    assert_eq!(status.reasoning.as_deref(), Some("high"));

    let skills = session.list_skills().await.expect("codex skills");
    assert_eq!(skills.skills.len(), 1);
    assert_eq!(skills.skills[0].name.as_str(), "browser-use:browser");
    assert_eq!(
        skills.skills[0].display_name.as_deref(),
        Some("Browser Use")
    );

    let mcp = invoke_and_recv_assistant_text(
        session.as_ref(),
        &mut events,
        AgentCommandInvocation {
            name: "mcp".into(),
            args: None,
            values: Value::Null,
            source: AgentCommandSource::ProviderNative,
        },
    )
    .await;
    let plugins = invoke_and_recv_assistant_text(
        session.as_ref(),
        &mut events,
        AgentCommandInvocation {
            name: "plugins".into(),
            args: None,
            values: Value::Null,
            source: AgentCommandSource::ProviderNative,
        },
    )
    .await;
    assert!(mcp.starts_with("MCP servers:\n"));
    assert!(plugins.starts_with("Plugins:\n"));

    let permissions = session.list_permissions().await.expect("codex permissions");
    assert_eq!(permissions.current_mode.as_deref(), Some("full-access"));
    assert_eq!(
        permissions
            .modes
            .iter()
            .map(|mode| mode.id.as_str())
            .collect::<Vec<_>>(),
        ["default", "auto-review", "full-access", "bypass"]
    );
    let status = session
        .set_permissions(AgentPermissionSelection {
            mode: "auto-review".into(),
        })
        .await
        .expect("set codex permissions");
    assert_eq!(status.permissions.as_deref(), Some("Auto-review"));

    let fork_targets = session
        .list_fork_targets()
        .await
        .expect("codex fork targets");
    assert_eq!(
        fork_targets
            .targets
            .iter()
            .map(|target| target.id.as_str())
            .collect::<Vec<_>>(),
        ["user-1", "user-2"]
    );
    let forked = session
        .fork(AgentForkSelection {
            target_id: "user-1".into(),
        })
        .await
        .expect("codex fork");
    assert_eq!(
        forked.session_ref.as_ref().map(|value| value.0.as_str()),
        Some("codex-forked-thread")
    );
    assert_eq!(
        session
            .provider_session_id()
            .await
            .expect("codex provider session id")
            .0
            .as_str(),
        "codex-forked-thread"
    );
}

#[tokio::test]
async fn agent_runtime_commands_codex_generic_status_uses_adapter_path() {
    let runtime = runtime();
    let session = runtime
        .open(
            "codex",
            open_request("codex", "commands_adapter_status.fixture", None),
        )
        .await
        .expect("open codex adapter status session");
    let mut events = take_events(session.as_ref()).await;

    let status_text = invoke_and_recv_assistant_text(
        session.as_ref(),
        &mut events,
        AgentCommandInvocation {
            name: "status".into(),
            args: None,
            values: Value::Null,
            source: AgentCommandSource::AdapterMapped,
        },
    )
    .await;

    assert!(status_text.contains("Status:"));
    assert!(status_text.contains("Model: `gpt-5.4` (reasoning low)"));
    assert!(status_text.contains("Session: `codex-adapter-status-thread`"));
}

#[tokio::test]
async fn agent_runtime_commands_codex_maps_review_args_to_custom_target() {
    let runtime = runtime();
    let session = runtime
        .open(
            "codex",
            open_request("codex", "commands_review_custom.fixture", None),
        )
        .await
        .expect("open codex command session");
    let mut events = take_events(session.as_ref()).await;

    session
        .invoke_command(AgentCommandInvocation {
            name: "review".into(),
            args: Some("focus on the runtime API".into()),
            values: Value::Null,
            source: AgentCommandSource::ProviderNative,
        })
        .await
        .expect("invoke codex review custom command");

    let events = collect_until_closed(&mut events).await;
    assert_eq!(assistant_texts(&events), vec!["CUSTOM_REVIEW_OK"]);
}

#[tokio::test]
async fn agent_runtime_commands_reject_unknown_native_command_before_prompt_passthrough() {
    let runtime = runtime();
    let session = runtime
        .open("claude", open_request("claude", "commands.fixture", None))
        .await
        .expect("open claude command session");

    let err = session
        .invoke_command(AgentCommandInvocation {
            name: "missing".into(),
            args: None,
            values: Value::Null,
            source: AgentCommandSource::ProviderNative,
        })
        .await
        .expect_err("unknown command should be rejected");

    assert!(
        err.to_string().contains("unsupported command"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn agent_runtime_commands_submit_still_treats_slash_text_as_plain_prompt() {
    let runtime = runtime();
    let session = runtime
        .open("codex", open_request("codex", "basic.fixture", None))
        .await
        .expect("open codex basic session");
    let mut events = take_events(session.as_ref()).await;

    session
        .submit(AgentInput {
            text: "/review".into(),
            images: Vec::new(),
        })
        .await
        .expect("submit slash-looking text");

    let events = collect_until_closed(&mut events).await;
    assert_eq!(assistant_texts(&events), vec!["Hello from Codex!"]);
}
