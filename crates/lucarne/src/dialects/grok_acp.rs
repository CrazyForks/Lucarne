//! Grok Build ACP dialect — `grok agent stdio` JSON-RPC 2.0.
//!
//! Wire lifecycle:
//! 1. `initialize` (advertise `clientCapabilities.fs.readTextFile/writeTextFile`)
//! 2. `session/new` or `session/load` (resume UUID)
//! 3. `session/prompt` for each user turn
//! 4. `session/update` notifications stream chunks/tools
//! 5. reverse client RPCs: `fs/read_text_file`, `fs/write_text_file`,
//!    optional `session/request_permission`
//! 6. `session/cancel` for interrupt

use crate::{
    agent_runtime::{
        AgentCommand, AgentCommandCatalog, AgentCommandCompletion, AgentCommandInput,
        AgentCommandInvocation, AgentCommandSource, AgentForkResult, AgentForkTarget,
        AgentForkTargetCatalog, AgentModelCatalog, AgentModelOption, AgentPermissionCatalog,
        AgentPermissionOption, AgentReasoningOption, AgentSkillCatalog, AgentSkillSummary,
        AgentStatus, SessionRef,
    },
    dialect::{
        fork_name, model_args, normalize_agent_command_name, permission_arg, CommandDispatch,
        CommandResult, Conversation, Dialect, Input, ModelSelection, OutFrame, PermissionMode,
        PermissionSelection, SessionParams,
    },
    error::{LucarneError, Result},
    event::{
        self, Decision, Event, Payload, PermissionQuestion, PermissionQuestionOption,
        PermissionRequest, PermissionResponse, ResumeHandle, Risk, SessionClosed, SessionStarted,
        Timeline, ToolResult, TurnCompleted, TurnFailed, Usage,
    },
};
use serde_json::{json, Value};
use smol_str::SmolStr;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use tracing::{debug, warn};

/// Lucarne AdapterMapped names — never injected into ProviderNative catalog.
const ADAPTER_MAPPED_COMMANDS: &[&str] = &[
    "status",
    "model",
    "list_models",
    "permissions",
    "skills",
    "new",
    "fork",
    "list_commands",
    "quit",
    "exit",
];

pub struct GrokAcp {
    cfg: SessionParams,
    seq: u64,
    pending: HashMap<i64, PendingKind>,
    pending_out: Vec<OutFrame>,
    pending_prompts: VecDeque<Input>,
    state: SessionState,
    session_started: bool,
    session_id: Option<String>,
    supports_load: bool,
    turn_active: bool,
    turn_id: u64,
    assistant_buf: String,
    thought_buf: String,
    tool_names: HashMap<String, String>,
    permission_rpc_ids: HashMap<String, i64>,
    command_catalog: AgentCommandCatalog,
    /// Raw availableCommands entries (including skill metadata) for skills list.
    available_command_meta: Vec<Value>,
    models: AgentModelCatalog,
    current_permission_mode: String,
    agent_version: Option<String>,
    skill_names: HashSet<String>,
}

#[derive(Debug, Clone)]
enum PendingKind {
    Initialize,
    /// Bootstrap `session/new` after initialize (no fake `/new` command events).
    SessionNew,
    /// User `/new` AdapterMapped command — emits NewConversation on success.
    CommandNewSession,
    SessionLoad,
    SessionPrompt {
        turn_id: String,
        /// Optional public command completion when this prompt was a control slash.
        on_complete: PromptCommandComplete,
    },
    SessionCancel,
    /// Optional ACP `session/set_model` when supported.
    SetModel { model: String, reasoning: Option<String> },
    /// x.ai session fork RPC.
    SessionFork,
}

#[derive(Debug, Clone, Default)]
enum PromptCommandComplete {
    #[default]
    None,
    PermissionsChanged {
        mode: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionState {
    Initializing,
    Ready,
    Closed,
}

impl Default for GrokAcp {
    fn default() -> Self {
        Self::new()
    }
}

impl GrokAcp {
    pub fn new() -> Self {
        Self {
            cfg: SessionParams::default(),
            seq: 1,
            pending: HashMap::new(),
            pending_out: Vec::new(),
            pending_prompts: VecDeque::new(),
            state: SessionState::Initializing,
            session_started: false,
            session_id: None,
            supports_load: true,
            turn_active: false,
            turn_id: 0,
            assistant_buf: String::new(),
            thought_buf: String::new(),
            tool_names: HashMap::new(),
            permission_rpc_ids: HashMap::new(),
            command_catalog: AgentCommandCatalog {
                commands: Vec::new(),
                complete: true,
                revision: 0,
            },
            available_command_meta: Vec::new(),
            models: AgentModelCatalog::default(),
            current_permission_mode: permission_mode_label(PermissionMode::Default).into(),
            agent_version: None,
            skill_names: HashSet::new(),
        }
    }

    fn next_id(&mut self) -> i64 {
        let id = self.seq as i64;
        self.seq = self.seq.wrapping_add(1);
        id
    }

    fn current_turn_id(&self) -> String {
        format!("grok-turn-{}", self.turn_id)
    }

    fn enqueue_rpc(&mut self, id: i64, method: &str, params: Value, kind: PendingKind) {
        self.pending.insert(id, kind);
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut bytes = serde_json::to_vec(&msg).expect("json");
        bytes.push(b'\n');
        self.pending_out.push(OutFrame::stdin(bytes));
    }

    fn enqueue_result(&mut self, id: i64, result: Value) {
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        });
        let mut bytes = serde_json::to_vec(&msg).expect("json");
        bytes.push(b'\n');
        self.pending_out.push(OutFrame::stdin(bytes));
    }

    fn enqueue_error(&mut self, id: i64, code: i64, message: impl Into<String>) {
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": code,
                "message": message.into(),
            },
        });
        let mut bytes = serde_json::to_vec(&msg).expect("json");
        bytes.push(b'\n');
        self.pending_out.push(OutFrame::stdin(bytes));
    }

    /// Parse JSON-RPC request id (number preferred; string digits accepted).
    fn rpc_id(obj: &Value) -> Option<i64> {
        let id = obj.get("id")?;
        if let Some(n) = id.as_i64() {
            return Some(n);
        }
        if let Some(n) = id.as_u64() {
            return i64::try_from(n).ok();
        }
        if let Some(s) = id.as_str() {
            return s.parse().ok();
        }
        None
    }

    /// ACP reverse: `fs/read_text_file` → `{ content }`.
    fn handle_fs_read_text_file(&mut self, id: i64, params: &Value) {
        let path = match params.get("path").and_then(|v| v.as_str()) {
            Some(p) if !p.is_empty() => p,
            _ => {
                self.enqueue_error(id, -32602, "fs/read_text_file: missing path");
                return;
            }
        };
        let line = params
            .get("line")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);

        match std::fs::read_to_string(path) {
            Ok(full) => {
                let content = apply_line_window(&full, line, limit);
                self.enqueue_result(id, json!({ "content": content }));
            }
            Err(err) => {
                self.enqueue_error(
                    id,
                    -32000,
                    format!("fs/read_text_file: {path}: {err}"),
                );
            }
        }
    }

    /// ACP reverse: `fs/write_text_file` → empty object result.
    fn handle_fs_write_text_file(&mut self, id: i64, params: &Value) {
        let path = match params.get("path").and_then(|v| v.as_str()) {
            Some(p) if !p.is_empty() => p,
            _ => {
                self.enqueue_error(id, -32602, "fs/write_text_file: missing path");
                return;
            }
        };
        let content = match params.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => {
                self.enqueue_error(id, -32602, "fs/write_text_file: missing content");
                return;
            }
        };
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(err) = std::fs::create_dir_all(parent) {
                    self.enqueue_error(
                        id,
                        -32000,
                        format!("fs/write_text_file: create parent {parent:?}: {err}"),
                    );
                    return;
                }
            }
        }
        match std::fs::write(path, content) {
            Ok(()) => self.enqueue_result(id, json!({})),
            Err(err) => self.enqueue_error(
                id,
                -32000,
                format!("fs/write_text_file: {path}: {err}"),
            ),
        }
    }

    fn resume_uuid(&self) -> Option<String> {
        if let Some(h) = &self.cfg.resume {
            if let Some(Value::String(id)) = h.data.get("session_id") {
                if !id.is_empty() {
                    return Some(id.clone());
                }
            }
        }
        if !self.cfg.resume_session_at.is_empty() {
            return Some(self.cfg.resume_session_at.clone());
        }
        None
    }

    fn emit_session_started(&mut self) -> Option<Event> {
        if self.session_started {
            return None;
        }
        let sid = self.session_id.clone().unwrap_or_default();
        if sid.is_empty() {
            return None;
        }
        self.session_started = true;
        Some(Event::new(Payload::SessionStarted(SessionStarted {
            session_id: sid,
            model: self.cfg.model.clone(),
        })))
    }

    fn flush_pending_prompts(&mut self) {
        while let Some(input) = self.pending_prompts.pop_front() {
            if let Ok(frames) = self.encode_user_message(&input) {
                self.pending_out.extend(frames);
            }
        }
    }

    fn begin_turn(&mut self) -> String {
        self.turn_id = self.turn_id.wrapping_add(1);
        self.turn_active = true;
        self.assistant_buf.clear();
        self.thought_buf.clear();
        self.current_turn_id()
    }

    fn content_text(content: &Value) -> Option<String> {
        if let Some(t) = content.get("text").and_then(|v| v.as_str()) {
            return Some(t.to_string());
        }
        if let Some(arr) = content.as_array() {
            let mut out = String::new();
            for item in arr {
                if let Some(t) = item
                    .pointer("/content/text")
                    .and_then(|v| v.as_str())
                    .or_else(|| item.get("text").and_then(|v| v.as_str()))
                {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(t);
                }
            }
            if !out.is_empty() {
                return Some(out);
            }
        }
        None
    }

    fn handle_session_update(&mut self, params: &Value) -> Vec<Event> {
        let mut events = Vec::new();
        let session_id = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if let Some(expected) = &self.session_id {
            if !session_id.is_empty() && session_id != expected {
                // Foreign session noise — ignore (parity with codex foreign_thread).
                debug!(
                    target: "lucarne::dialects::grok_acp",
                    session_id, expected,
                    "ignoring foreign session update"
                );
                return events;
            }
        }

        let update = match params.get("update") {
            Some(u) => u,
            None => return events,
        };
        let kind = update
            .get("sessionUpdate")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let turn = self.current_turn_id();

        match kind {
            "agent_thought_chunk" => {
                if let Some(text) = update.get("content").and_then(Self::content_text) {
                    self.thought_buf.push_str(&text);
                    events.push(Event::new(Payload::Timeline(Timeline {
                        item: event::new_timeline_reasoning(&turn, &text),
                    })));
                }
            }
            "agent_message_chunk" => {
                if let Some(text) = update.get("content").and_then(Self::content_text) {
                    self.assistant_buf.push_str(&text);
                    events.push(Event::new(Payload::Timeline(Timeline {
                        item: event::new_timeline_assistant(&turn, &text, true),
                    })));
                }
            }
            "user_message_chunk" => {
                if let Some(text) = update.get("content").and_then(Self::content_text) {
                    events.push(Event::new(Payload::Timeline(Timeline {
                        item: event::new_timeline_user(&turn, &text),
                    })));
                }
            }
            "tool_call" => {
                let call_id = update
                    .get("toolCallId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool");
                let name = update
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool");
                self.tool_names
                    .insert(call_id.to_string(), name.to_string());
                let input = update.get("rawInput").cloned().unwrap_or(json!({}));
                let call = event::tool_call(name, input);
                events.push(Event::new(Payload::Timeline(Timeline {
                    item: event::new_timeline_tool_call(call_id, call),
                })));
            }
            "tool_call_update" => {
                let call_id = update
                    .get("toolCallId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool");
                // Grok often streams intermediate tool_call_update without
                // status; only emit ToolResult on a terminal status.
                let status = update.get("status").and_then(|v| v.as_str()).unwrap_or("");
                if matches!(
                    status,
                    "completed" | "failed" | "error" | "cancelled" | "rejected"
                ) {
                    let is_error =
                        matches!(status, "failed" | "error" | "cancelled" | "rejected");
                    let output = update
                        .get("content")
                        .and_then(Self::content_text)
                        .or_else(|| {
                            update.get("rawOutput").map(|v| match v {
                                Value::String(s) => s.clone(),
                                other => other.to_string(),
                            })
                        })
                        .unwrap_or_default();
                    events.push(Event::new(Payload::Timeline(Timeline {
                        item: event::new_timeline_tool_result(
                            &turn,
                            call_id,
                            ToolResult {
                                output: if is_error {
                                    String::new()
                                } else {
                                    output.clone()
                                },
                                error: if is_error { output } else { String::new() },
                                exit_code: if is_error { 1 } else { 0 },
                                ..Default::default()
                            },
                        ),
                    })));
                }
            }
            "turn_completed" => {
                if !self.turn_active {
                    // Already finalized via session/prompt result or prior notification.
                } else {
                    let final_text = self.assistant_buf.clone();
                    if !final_text.is_empty() {
                        events.push(Event::new(Payload::Timeline(Timeline {
                            item: event::new_timeline_assistant(&turn, &final_text, false),
                        })));
                    }
                    self.turn_active = false;
                    events.push(Event::new(Payload::TurnCompleted(TurnCompleted {
                        turn_id: turn,
                        usage: None,
                    })));
                }
            }
            "available_commands_update" => {
                self.ingest_available_commands(update);
            }
            _ => {}
        }
        events
    }

    fn ingest_available_commands(&mut self, update: &Value) {
        let Some(commands) = update.get("availableCommands").and_then(|v| v.as_array()) else {
            return;
        };
        self.available_command_meta = commands.clone();
        let mut out = Vec::new();
        let mut skills = HashSet::new();
        for command in commands {
            let raw_name = command.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let name = normalize_command_name(raw_name);
            if name.is_empty() || is_adapter_mapped_command(name) {
                continue;
            }
            if out
                .iter()
                .any(|cmd: &AgentCommand| cmd.name.as_str() == name)
            {
                continue;
            }
            let description = command
                .get("description")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(SmolStr::new);
            let path = command
                .pointer("/_meta/path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if path.contains("SKILL.md") || path.contains("/skills/") {
                skills.insert(name.to_string());
            }
            let required = command
                .pointer("/input/hint")
                .and_then(|v| v.as_str())
                .is_some_and(|h| h.contains("required"));
            out.push(AgentCommand {
                name: name.into(),
                description,
                aliases: Vec::new(),
                source: AgentCommandSource::ProviderNative,
                input: AgentCommandInput::Text {
                    label: "arguments".into(),
                    required,
                },
                completion: AgentCommandCompletion::ProviderIdle,
            });
        }
        self.skill_names = skills;
        self.command_catalog = AgentCommandCatalog {
            commands: out,
            complete: true,
            revision: self.command_catalog.revision.saturating_add(1),
        };
    }

    fn ingest_models(&mut self, models_val: &Value) {
        if let Some(current) = models_val
            .get("currentModelId")
            .or_else(|| models_val.get("current_model_id"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            self.models.current_model = Some(current.into());
            self.cfg.model = current.to_string();
        }
        let items = models_val
            .get("availableModels")
            .or_else(|| models_val.get("available_models"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut models = Vec::new();
        for item in items {
            let id = item
                .get("modelId")
                .or_else(|| item.get("id"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let Some(id) = id else { continue };
            let name = item
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(id);
            let description = item
                .get("description")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(SmolStr::new);
            let mut supported_reasoning = Vec::new();
            if let Some(efforts) = item
                .pointer("/_meta/reasoningEfforts")
                .or_else(|| item.pointer("/_meta/reasoning_efforts"))
                .and_then(|v| v.as_array())
            {
                for effort in efforts {
                    let value = effort
                        .get("id")
                        .or_else(|| effort.get("value"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim();
                    if value.is_empty() {
                        continue;
                    }
                    supported_reasoning.push(AgentReasoningOption {
                        value: value.into(),
                        description: effort
                            .get("description")
                            .and_then(|v| v.as_str())
                            .map(SmolStr::new),
                        is_default: effort.get("default").and_then(|v| v.as_bool()),
                    });
                }
            }
            if let Some(current_effort) = item
                .pointer("/_meta/reasoningEffort")
                .or_else(|| item.pointer("/_meta/reasoning_effort"))
                .and_then(|v| v.as_str())
            {
                if self.models.current_model.as_deref() == Some(id) {
                    self.models.current_reasoning = Some(current_effort.into());
                }
            }
            models.push(AgentModelOption {
                id: id.into(),
                display_name: Some(name.into()),
                description,
                supported_reasoning,
            });
        }
        if !models.is_empty() {
            self.models.models = models;
        }
    }

    fn list_commands(&self) -> Result<CommandDispatch> {
        Ok(CommandDispatch::ready(CommandResult::Commands(
            self.command_catalog.clone(),
        )))
    }

    fn list_models(&self) -> Result<CommandDispatch> {
        Ok(CommandDispatch::ready(CommandResult::Models(
            self.models.clone(),
        )))
    }

    fn set_model(&mut self, model: &str, reasoning: Option<&str>) -> Result<CommandDispatch> {
        let model = model.trim();
        if model.is_empty() {
            return Err(LucarneError::dialect("grok: /model requires a model id"));
        }
        if !self.models.models.is_empty()
            && !self
                .models
                .models
                .iter()
                .any(|m| m.id.as_str().eq_ignore_ascii_case(model))
        {
            debug!(
                target: "lucarne::dialects::grok_acp",
                model,
                "model not in cached catalog; sending set_model/slash anyway"
            );
        }
        let effort = reasoning.map(str::trim).filter(|s| !s.is_empty());
        let mut slash = format!("/model {model}");
        if let Some(e) = effort {
            slash.push(' ');
            slash.push_str(e);
        }
        // Prefer ACP session/set_model (include reasoningEffort when set).
        // When reasoning is present, also dual-path slash so Grok effort applies
        // even if the RPC ignores that field. Do not call encode_user_message
        // here — it takes pending_out and would drop the set_model frame.
        if let Some(session_id) = self.session_id.clone() {
            let id = self.next_id();
            let mut params = json!({
                "sessionId": session_id,
                "modelId": model,
            });
            if let Some(e) = effort {
                params["reasoningEffort"] = json!(e);
            }
            self.enqueue_rpc(
                id,
                "session/set_model",
                params,
                PendingKind::SetModel {
                    model: model.into(),
                    reasoning: effort.map(str::to_string),
                },
            );
            if effort.is_some() && self.state == SessionState::Ready {
                let turn_id = self.begin_turn();
                let pid = self.next_id();
                let sid = self.session_id.clone().unwrap_or_default();
                self.enqueue_rpc(
                    pid,
                    "session/prompt",
                    json!({
                        "sessionId": sid,
                        "prompt": [{ "type": "text", "text": slash }],
                    }),
                    PendingKind::SessionPrompt {
                        turn_id,
                        on_complete: PromptCommandComplete::None,
                    },
                );
            }
            return Ok(CommandDispatch::deferred(std::mem::take(
                &mut self.pending_out,
            )));
        }
        self.encode_user_message(&Input {
            text: slash,
            images: Vec::new(),
        })
        .map(CommandDispatch::deferred)
    }

    fn list_permissions(&self) -> Result<CommandDispatch> {
        Ok(CommandDispatch::ready(CommandResult::Permissions(
            grok_permission_catalog(&self.current_permission_mode),
        )))
    }

    fn set_permissions(&mut self, mode: &str) -> Result<CommandDispatch> {
        let mode = mode.trim();
        if mode.is_empty() {
            return Err(LucarneError::dialect(
                "grok: /permissions requires a mode id",
            ));
        }
        let normalized = normalize_permission_mode(mode).ok_or_else(|| {
            LucarneError::dialect(format!(
                "grok: unknown permission mode {mode:?}; expected default|always-approve|auto|bypass|full"
            ))
        })?;
        self.current_permission_mode = normalized.into();
        // Real agent path: documented slash toggles over session/prompt.
        // Emit PermissionsChanged when that prompt RPC completes (not before
        // discarding frames — agent must receive the slash).
        let slash = match normalized {
            "always-approve" | "bypass" | "full" => Some("/always-approve"),
            "auto" => Some("/auto"),
            // default: no Grok slash to force interactive mode over ACP; still
            // surface Lucarne-local PermissionsChanged for catalog/status.
            _ => None,
        };
        if let Some(text) = slash {
            if self.state == SessionState::Ready {
                let turn_id = self.begin_turn();
                let id = self.next_id();
                let session_id = self
                    .session_id
                    .clone()
                    .ok_or_else(|| LucarneError::dialect("grok: no session id"))?;
                self.enqueue_rpc(
                    id,
                    "session/prompt",
                    json!({
                        "sessionId": session_id,
                        "prompt": [{ "type": "text", "text": text }],
                    }),
                    PendingKind::SessionPrompt {
                        turn_id,
                        on_complete: PromptCommandComplete::PermissionsChanged {
                            mode: normalized.into(),
                        },
                    },
                );
                return Ok(CommandDispatch::deferred(std::mem::take(
                    &mut self.pending_out,
                )));
            }
        }
        Ok(CommandDispatch::ready(CommandResult::PermissionsChanged(
            PermissionSelection {
                mode: normalized.into(),
            },
        )))
    }

    fn list_skills(&self) -> Result<CommandDispatch> {
        let mut skills = Vec::new();
        for raw in &self.available_command_meta {
            let name = raw
                .get("name")
                .and_then(|v| v.as_str())
                .map(normalize_command_name)
                .filter(|n| !n.is_empty())
                .unwrap_or("");
            if name.is_empty() {
                continue;
            }
            let path = raw
                .pointer("/_meta/path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let is_skill = path.contains("SKILL.md")
                || path.contains("/skills/")
                || self.skill_names.contains(name);
            if !is_skill {
                continue;
            }
            skills.push(AgentSkillSummary {
                name: name.into(),
                display_name: Some(name.into()),
                description: raw
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(SmolStr::new),
                path: (!path.is_empty()).then(|| path.into()),
                scope: raw
                    .pointer("/_meta/scope")
                    .and_then(|v| v.as_str())
                    .map(SmolStr::new),
                source: Some("grok".into()),
                tokens: None,
                enabled: Some(true),
            });
        }
        // Fallback: provider-native catalog entries that look like skills by name
        // when meta is missing.
        if skills.is_empty() {
            for cmd in &self.command_catalog.commands {
                if self.skill_names.contains(cmd.name.as_str()) {
                    skills.push(AgentSkillSummary {
                        name: cmd.name.clone(),
                        display_name: Some(cmd.name.clone()),
                        description: cmd.description.clone(),
                        path: None,
                        scope: Some("user".into()),
                        source: Some("grok".into()),
                        tokens: None,
                        enabled: Some(true),
                    });
                }
            }
        }
        Ok(CommandDispatch::ready(CommandResult::Skills(
            AgentSkillCatalog { skills },
        )))
    }

    fn status(&self) -> Result<CommandDispatch> {
        Ok(CommandDispatch::ready(CommandResult::Status(AgentStatus {
            version: self.agent_version.as_deref().map(SmolStr::new),
            session_id: self
                .session_id
                .as_deref()
                .or(if self.cfg.resume_session_at.is_empty() {
                    None
                } else {
                    Some(self.cfg.resume_session_at.as_str())
                })
                .map(SmolStr::new),
            directory: if self.cfg.cwd.is_empty() {
                None
            } else {
                Some(self.cfg.cwd.clone().into())
            },
            model: {
                let m = self
                    .models
                    .current_model
                    .as_deref()
                    .unwrap_or(self.cfg.model.as_str());
                (!m.is_empty()).then(|| m.into())
            },
            reasoning: self.models.current_reasoning.clone(),
            permissions: Some(self.current_permission_mode.clone().into()),
            ..Default::default()
        })))
    }

    fn new_conversation_command(&mut self) -> Result<CommandDispatch> {
        self.session_started = false;
        self.session_id = None;
        self.state = SessionState::Initializing;
        let id = self.next_id();
        self.enqueue_rpc(
            id,
            "session/new",
            json!({ "cwd": self.cfg.cwd, "mcpServers": [] }),
            PendingKind::CommandNewSession,
        );
        Ok(CommandDispatch::deferred(std::mem::take(
            &mut self.pending_out,
        )))
    }

    fn list_fork(&self) -> Result<CommandDispatch> {
        // Grok `/fork` is directive-based (optional worktree), not a catalog of
        // historical entry ids like Pi. Surface a single actionable target.
        let current = self
            .session_id
            .clone()
            .unwrap_or_else(|| "current".into());
        Ok(CommandDispatch::ready(CommandResult::ForkTargets(
            AgentForkTargetCatalog {
                targets: vec![AgentForkTarget {
                    id: current.into(),
                    label: Some("current session".into()),
                    description: Some(
                        "Fork via /fork [directive]; optional --worktree/--no-worktree".into(),
                    ),
                }],
            },
        )))
    }

    fn fork(&mut self, target: Option<&str>) -> Result<CommandDispatch> {
        let session_id = self
            .session_id
            .clone()
            .ok_or_else(|| LucarneError::dialect("grok: no session id for fork"))?;
        // Prefer x.ai extension; dual-path slash without encode_user_message
        // (that helper takes pending_out and would drop the fork RPC frame).
        let directive = target.map(str::trim).filter(|s| !s.is_empty());
        let id = self.next_id();
        let mut params = json!({ "sessionId": session_id.clone() });
        if let Some(d) = directive {
            params["directive"] = json!(d);
        }
        self.enqueue_rpc(id, "x.ai/session/fork", params, PendingKind::SessionFork);
        let slash = match directive {
            Some(d) if !d.starts_with("--") && d != "current" => format!("/fork {d}"),
            Some(d) => format!("/fork {d}"),
            None => "/fork".into(),
        };
        if self.state == SessionState::Ready {
            let turn_id = self.begin_turn();
            let pid = self.next_id();
            self.enqueue_rpc(
                pid,
                "session/prompt",
                json!({
                    "sessionId": session_id,
                    "prompt": [{ "type": "text", "text": slash }],
                }),
                PendingKind::SessionPrompt {
                    turn_id,
                    on_complete: PromptCommandComplete::None,
                },
            );
        }
        Ok(CommandDispatch::deferred(std::mem::take(
            &mut self.pending_out,
        )))
    }

    fn quit(&self) -> Result<CommandDispatch> {
        Ok(CommandDispatch::ready(CommandResult::Quit))
    }

    fn dispatch_native_slash(&mut self, command: &AgentCommandInvocation) -> Result<CommandDispatch> {
        let text = build_native_slash_command(&self.command_catalog, command)?;
        self.encode_user_message(&Input {
            text,
            images: Vec::new(),
        })
        .map(CommandDispatch::deferred)
    }

    fn handle_permission_request(&mut self, id: i64, params: &Value) -> Vec<Event> {
        let tool_call_id = params
            .pointer("/toolCall/toolCallId")
            .or_else(|| params.get("toolCallId"))
            .and_then(|v| v.as_str())
            .unwrap_or("permission");
        let title = params
            .pointer("/toolCall/title")
            .or_else(|| params.get("title"))
            .and_then(|v| v.as_str())
            .unwrap_or("permission");
        let req_id = format!("grok-perm-{id}");
        self.permission_rpc_ids.insert(req_id.clone(), id);

        let options = vec![
            PermissionQuestionOption {
                label: "allow-once".into(),
                description: "Allow once".into(),
            },
            PermissionQuestionOption {
                label: "deny".into(),
                description: "Deny".into(),
            },
        ];
        vec![Event::new(Payload::PermissionRequest(PermissionRequest {
            req_id,
            tool: title.into(),
            input: Some(params.get("toolCall").cloned().unwrap_or_else(|| params.clone())),
            risk: Risk::Medium,
            questions: vec![PermissionQuestion {
                id: "decision".into(),
                header: "Permission".into(),
                question: format!("Allow tool `{title}` ({tool_call_id})?"),
                options,
                multi_select: false,
                is_other: false,
                is_secret: false,
            }],
        }))]
    }
}

impl Dialect for GrokAcp {
    fn name(&self) -> &'static str {
        "grok"
    }

    fn init(&mut self, cfg: &SessionParams) -> Vec<OutFrame> {
        self.cfg = cfg.clone();
        self.current_permission_mode = permission_mode_label(cfg.permission_mode).into();
        if !cfg.model.is_empty() {
            self.models.current_model = Some(cfg.model.clone().into());
        }
        self.state = SessionState::Initializing;
        let id = self.next_id();
        self.enqueue_rpc(
            id,
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": { "readTextFile": true, "writeTextFile": true },
                    "terminal": true
                },
                "clientInfo": { "name": "lucarne", "version": env!("CARGO_PKG_VERSION") }
            }),
            PendingKind::Initialize,
        );
        std::mem::take(&mut self.pending_out)
    }

    fn translate(&mut self, frame_bytes: &[u8]) -> Vec<Event> {
        let text = match std::str::from_utf8(frame_bytes) {
            Ok(t) => t.trim(),
            Err(_) => return Vec::new(),
        };
        if text.is_empty() {
            return Vec::new();
        }
        let obj: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(err) => {
                warn!(target: "lucarne::dialects::grok_acp", %err, "invalid json rpc line");
                return Vec::new();
            }
        };

        // Notifications (no id) or reverse requests from agent (method + id).
        if let Some(method) = obj.get("method").and_then(|m| m.as_str()) {
            let params = obj.get("params").cloned().unwrap_or(json!({}));
            if method == "session/update" || method == "_x.ai/session/update" {
                return self.handle_session_update(&params);
            }
            // Real Grok often emits turn_completed / tool deltas on this channel.
            if method == "_x.ai/session_notification" {
                return self.handle_session_update(&params);
            }
            if method == "_x.ai/models/update" {
                self.ingest_models(&params);
                return Vec::new();
            }
            if let Some(id) = Self::rpc_id(&obj) {
                match method {
                    "session/request_permission" | "request_permission" => {
                        return self.handle_permission_request(id, &params);
                    }
                    "fs/read_text_file" => {
                        self.handle_fs_read_text_file(id, &params);
                        return Vec::new();
                    }
                    "fs/write_text_file" => {
                        self.handle_fs_write_text_file(id, &params);
                        return Vec::new();
                    }
                    // Unknown reverse RPC with id — reply method-not-found so
                    // the agent does not hang waiting forever.
                    other => {
                        warn!(
                            target: "lucarne::dialects::grok_acp",
                            method = other,
                            id,
                            "unknown reverse acp request"
                        );
                        self.enqueue_error(
                            id,
                            -32601,
                            format!("method not found: {other}"),
                        );
                        return Vec::new();
                    }
                }
            }
            // Notifications without id (mcp progress, announcements, queue).
            return Vec::new();
        }

        // Responses
        let id = match obj.get("id").and_then(|v| v.as_i64()) {
            Some(id) => id,
            None => return Vec::new(),
        };
        if let Some(err) = obj.get("error") {
            let msg = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("rpc error");
            let kind = self.pending.remove(&id);
            let mut events = Vec::new();
            if matches!(
                kind,
                Some(
                    PendingKind::SessionNew
                        | PendingKind::CommandNewSession
                        | PendingKind::SessionLoad
                        | PendingKind::Initialize
                )
            ) {
                events.push(Event::new(Payload::SessionClosed(SessionClosed {
                    reason: msg.into(),
                    resume: None,
                })));
            } else if matches!(kind, Some(PendingKind::SessionPrompt { .. })) {
                self.turn_active = false;
                events.push(Event::new(Payload::TurnFailed(TurnFailed {
                    turn_id: self.current_turn_id(),
                    error: msg.into(),
                    code: String::new(),
                })));
            } else if let Some(PendingKind::SetModel { model, reasoning }) = kind {
                // session/set_model unsupported — fall back to slash /model prompt.
                let mut slash = format!("/model {model}");
                if let Some(effort) = reasoning.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    slash.push(' ');
                    slash.push_str(effort);
                }
                if let Ok(frames) = self.encode_user_message(&Input {
                    text: slash,
                    images: Vec::new(),
                }) {
                    self.pending_out.extend(frames);
                }
            } else if matches!(kind, Some(PendingKind::SessionFork)) {
                // RPC fork unsupported — slash already queued when possible; emit message.
                events.extend(crate::dialect::command_result_events(
                    &self.current_turn_id(),
                    CommandResult::Message(crate::dialect::CommandMessage::text(
                        "Fork requested (agent fork RPC unavailable; used /fork path when ready).",
                    )),
                ));
            }
            return events;
        }

        let result = obj.get("result").cloned().unwrap_or(Value::Null);
        let kind = self.pending.remove(&id);
        let emit_new_command = matches!(kind, Some(PendingKind::CommandNewSession));
        let mut events = Vec::new();
        match kind {
            Some(PendingKind::Initialize) => {
                // Detect loadSession capability if present
                if let Some(load) = result
                    .pointer("/agentCapabilities/loadSession")
                    .and_then(|v| v.as_bool())
                {
                    self.supports_load = load;
                }
                if let Some(ver) = result
                    .pointer("/_meta/agentVersion")
                    .or_else(|| result.pointer("/_meta/agent_version"))
                    .and_then(|v| v.as_str())
                {
                    self.agent_version = Some(ver.to_string());
                }
                // Start session
                if let Some(uuid) = self.resume_uuid() {
                    if self.supports_load {
                        let rid = self.next_id();
                        self.enqueue_rpc(
                            rid,
                            "session/load",
                            json!({
                                "sessionId": uuid,
                                "cwd": self.cfg.cwd,
                                "mcpServers": []
                            }),
                            PendingKind::SessionLoad,
                        );
                    } else {
                        // Fall back to new session if load unsupported
                        let rid = self.next_id();
                        self.enqueue_rpc(
                            rid,
                            "session/new",
                            json!({ "cwd": self.cfg.cwd, "mcpServers": [] }),
                            PendingKind::SessionNew,
                        );
                    }
                } else {
                    let rid = self.next_id();
                    self.enqueue_rpc(
                        rid,
                        "session/new",
                        json!({ "cwd": self.cfg.cwd, "mcpServers": [] }),
                        PendingKind::SessionNew,
                    );
                }
            }
            Some(PendingKind::SessionNew)
            | Some(PendingKind::CommandNewSession)
            | Some(PendingKind::SessionLoad) => {
                let mut sid = result
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                // session/load may omit sessionId in result; keep requested UUID.
                if sid.is_empty() {
                    if let Some(resume) = self.resume_uuid() {
                        sid = resume;
                    }
                }
                if let Some(models) = result.get("models") {
                    self.ingest_models(models);
                }
                if sid.is_empty() {
                    events.push(Event::new(Payload::SessionClosed(SessionClosed {
                        reason: "grok session missing sessionId".into(),
                        resume: None,
                    })));
                    self.state = SessionState::Closed;
                } else {
                    self.session_id = Some(sid.clone());
                    self.state = SessionState::Ready;
                    if let Some(ev) = self.emit_session_started() {
                        events.push(ev);
                    }
                    // Only `/new` AdapterMapped emits NewConversation — not bootstrap open.
                    if emit_new_command {
                        events.extend(crate::dialect::command_result_events(
                            &self.current_turn_id(),
                            CommandResult::NewConversation(Conversation {
                                session_ref: Some(SessionRef(sid.into())),
                            }),
                        ));
                    }
                    self.flush_pending_prompts();
                }
            }
            Some(PendingKind::SessionPrompt {
                turn_id,
                on_complete,
            }) => {
                // Prompt RPC completed — if turn_completed update was not sent,
                // finalize from buffer.
                if self.turn_active {
                    let final_text = self.assistant_buf.clone();
                    if !final_text.is_empty() {
                        events.push(Event::new(Payload::Timeline(Timeline {
                            item: event::new_timeline_assistant(&turn_id, &final_text, false),
                        })));
                    }
                    self.turn_active = false;
                    events.push(Event::new(Payload::TurnCompleted(TurnCompleted {
                        turn_id: turn_id.clone(),
                        usage: result.get("usage").map(|u| Usage {
                            input_tokens: u.get("inputTokens").and_then(|v| v.as_i64()).unwrap_or(0),
                            output_tokens: u
                                .get("outputTokens")
                                .and_then(|v| v.as_i64())
                                .unwrap_or(0),
                            ..Default::default()
                        }),
                    })));
                }
                if let PromptCommandComplete::PermissionsChanged { mode } = on_complete {
                    self.current_permission_mode = mode.clone();
                    events.extend(crate::dialect::command_result_events(
                        &turn_id,
                        CommandResult::PermissionsChanged(PermissionSelection {
                            mode: mode.into(),
                        }),
                    ));
                }
            }
            Some(PendingKind::SetModel { model, reasoning }) => {
                self.cfg.model = model.clone();
                self.models.current_model = Some(model.clone().into());
                if let Some(r) = reasoning.as_deref() {
                    self.models.current_reasoning = Some(r.into());
                }
                events.extend(crate::dialect::command_result_events(
                    &self.current_turn_id(),
                    CommandResult::ModelChanged(ModelSelection {
                        model: model.into(),
                        reasoning: reasoning.map(Into::into),
                    }),
                ));
            }
            Some(PendingKind::SessionFork) => {
                let new_id = result
                    .get("sessionId")
                    .or_else(|| result.pointer("/session/sessionId"))
                    .or_else(|| result.get("forkedSessionId"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let source = self.session_id.clone();
                if !new_id.is_empty() {
                    self.session_id = Some(new_id.to_string());
                }
                events.extend(crate::dialect::command_result_events(
                    &self.current_turn_id(),
                    CommandResult::Forked(AgentForkResult {
                        session_ref: (!new_id.is_empty())
                            .then(|| SessionRef(new_id.into())),
                        source_session_ref: source.map(|id| SessionRef(id.into())),
                    }),
                ));
            }
            Some(PendingKind::SessionCancel) => {
                self.turn_active = false;
            }
            None => {}
        }
        events
    }

    fn drain_out_frames(&mut self) -> Vec<OutFrame> {
        std::mem::take(&mut self.pending_out)
    }

    fn encode_user_message(&mut self, input: &Input) -> Result<Vec<OutFrame>> {
        if self.state != SessionState::Ready {
            self.pending_prompts.push_back(input.clone());
            return Ok(Vec::new());
        }
        let session_id = self
            .session_id
            .clone()
            .ok_or_else(|| LucarneError::dialect("grok: no session id"))?;
        let turn_id = self.begin_turn();
        let id = self.next_id();
        let mut prompt = vec![json!({"type": "text", "text": input.text})];
        for image in &input.images {
            prompt.push(json!({
                "type": "image",
                "mimeType": image.media_type,
                "data": base64_encode(&image.data),
            }));
        }
        self.enqueue_rpc(
            id,
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": prompt,
            }),
            PendingKind::SessionPrompt {
                turn_id,
                on_complete: PromptCommandComplete::None,
            },
        );
        Ok(std::mem::take(&mut self.pending_out))
    }

    fn encode_interrupt(&mut self) -> Result<Vec<OutFrame>> {
        if let Some(session_id) = self.session_id.clone() {
            let id = self.next_id();
            self.enqueue_rpc(
                id,
                "session/cancel",
                json!({ "sessionId": session_id }),
                PendingKind::SessionCancel,
            );
        }
        Ok(std::mem::take(&mut self.pending_out))
    }

    fn encode_permission_response(
        &mut self,
        req_id: &str,
        response: &PermissionResponse,
    ) -> Result<Vec<OutFrame>> {
        let rpc_id = self
            .permission_rpc_ids
            .remove(req_id)
            .ok_or_else(|| {
                LucarneError::dialect(format!(
                    "grok: unknown permission request {req_id}"
                ))
            })?;
        let outcome = match response.decision {
            Decision::Allow => {
                let option_id = response
                    .answers
                    .values()
                    .next()
                    .and_then(|a| a.answers.first().cloned())
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        response
                            .answers
                            .values()
                            .next()
                            .map(|a| a.text.clone())
                            .filter(|s| !s.is_empty())
                    })
                    .unwrap_or_else(|| "allow-once".into());
                json!({
                    "outcome": {
                        "outcome": "selected",
                        "optionId": option_id
                    }
                })
            }
            Decision::Deny => json!({
                "outcome": { "outcome": "cancelled" }
            }),
        };
        self.enqueue_result(rpc_id, outcome);
        Ok(std::mem::take(&mut self.pending_out))
    }

    fn command_catalog(&self) -> AgentCommandCatalog {
        // ProviderNative only — AdapterMapped names must not appear here.
        self.command_catalog.clone()
    }

    fn handle_system_command(
        &mut self,
        command: &AgentCommandInvocation,
    ) -> Result<CommandDispatch> {
        match normalize_agent_command_name(command.name.as_str()) {
            "model" | "list_models" => match model_args(command) {
                Some((model, reasoning)) => self.set_model(&model, reasoning.as_deref()),
                None => self.list_models(),
            },
            "permissions" => match permission_arg(command) {
                Some(mode) => self.set_permissions(&mode),
                None => self.list_permissions(),
            },
            "skills" => self.list_skills(),
            "status" => self.status(),
            "new" => self.new_conversation_command(),
            "quit" | "exit" => self.quit(),
            "fork" => match fork_name(command) {
                Some(target) => self.fork(Some(&target)),
                None => self.list_fork(),
            },
            "list_commands" => self.list_commands(),
            other => Err(LucarneError::dialect(format!(
                "grok: unsupported system command {other:?}"
            ))),
        }
    }

    fn handle_native_command(
        &mut self,
        command: &AgentCommandInvocation,
    ) -> Result<CommandDispatch> {
        self.dispatch_native_slash(command)
    }

    fn on_exit(&mut self, exit_code: i32, err: Option<String>) -> Vec<Event> {
        let reason = err.unwrap_or_else(|| {
            if exit_code != 0 {
                format!("grok exited with code {exit_code}")
            } else {
                String::new()
            }
        });
        let resume = self.session_id.clone().map(|session_id| ResumeHandle {
            version: 1,
            data: {
                let mut m = BTreeMap::new();
                m.insert("session_id".into(), Value::String(session_id));
                if !self.cfg.cwd.is_empty() {
                    m.insert("cwd".into(), Value::String(self.cfg.cwd.clone()));
                }
                m
            },
        });
        vec![Event::new(Payload::SessionClosed(SessionClosed {
            reason,
            resume,
        }))]
    }
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn normalize_command_name(raw: &str) -> &str {
    raw.trim().trim_start_matches('/')
}

fn is_adapter_mapped_command(name: &str) -> bool {
    ADAPTER_MAPPED_COMMANDS
        .iter()
        .any(|cmd| cmd.eq_ignore_ascii_case(name))
}

fn permission_mode_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Default => "default",
        PermissionMode::Write => "default",
        PermissionMode::ReadOnly => "default",
        PermissionMode::Auto => "auto",
        PermissionMode::Full | PermissionMode::Bypass => "always-approve",
    }
}

fn normalize_permission_mode(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "default" | "normal" | "ask" => Some("default"),
        "always-approve" | "always_approve" | "yolo" | "bypass" | "full" | "full-access" => {
            Some("always-approve")
        }
        "auto" | "auto-approve" | "classifier" => Some("auto"),
        _ => None,
    }
}

fn grok_permission_catalog(current: &str) -> AgentPermissionCatalog {
    AgentPermissionCatalog {
        current_mode: Some(current.into()),
        modes: vec![
            AgentPermissionOption {
                id: "default".into(),
                display_name: Some("Default".into()),
                description: Some("Prompt for tool permissions".into()),
            },
            AgentPermissionOption {
                id: "always-approve".into(),
                display_name: Some("Always approve".into()),
                description: Some("Skip permission prompts (/always-approve)".into()),
            },
            AgentPermissionOption {
                id: "auto".into(),
                display_name: Some("Auto".into()),
                description: Some("Classifier auto-approves safe tools (/auto)".into()),
            },
        ],
    }
}

fn build_native_slash_command(
    catalog: &AgentCommandCatalog,
    command: &AgentCommandInvocation,
) -> Result<String> {
    let name = normalize_command_name(command.name.as_str());
    if name.is_empty() {
        return Err(LucarneError::dialect("grok: empty command name"));
    }
    if name.contains(char::is_whitespace) || name.contains('/') {
        return Err(LucarneError::dialect(format!(
            "grok: invalid command name {:?}",
            command.name
        )));
    }
    if is_adapter_mapped_command(name) {
        return Err(LucarneError::dialect(format!(
            "grok: {name} is AdapterMapped, not a ProviderNative command"
        )));
    }
    if !catalog
        .commands
        .iter()
        .any(|cmd| cmd.name.as_str() == name)
    {
        return Err(LucarneError::dialect(format!(
            "grok: unsupported native command {:?}",
            command.name
        )));
    }
    let args = command.args.as_deref().unwrap_or("").trim();
    if args.is_empty() {
        Ok(format!("/{name}"))
    } else {
        Ok(format!("/{name} {args}"))
    }
}

/// Optional 1-based line window used by ACP `fs/read_text_file`.
fn apply_line_window(full: &str, line: Option<usize>, limit: Option<usize>) -> String {
    match (line, limit) {
        (None, None) => full.to_string(),
        (start, lim) => {
            let start_idx = start.unwrap_or(1).saturating_sub(1);
            let lines: Vec<&str> = full.lines().collect();
            let end = match lim {
                Some(n) => start_idx.saturating_add(n).min(lines.len()),
                None => lines.len(),
            };
            if start_idx >= lines.len() {
                return String::new();
            }
            lines[start_idx..end].join("\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn initialize_then_session_new_emits_session_started() {
        let mut d = GrokAcp::new();
        let frames = d.init(&SessionParams {
            cwd: "/tmp/project".into(),
            model: "grok-4.5".into(),
            ..Default::default()
        });
        assert!(!frames.is_empty());
        let init_line = match &frames[0] {
            OutFrame::Stdin(b) => String::from_utf8_lossy(b).into_owned(),
            _ => panic!("expected stdin"),
        };
        assert!(init_line.contains("initialize"));

        let events = d.translate(
            br#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}}"#,
        );
        assert!(events.is_empty());
        let out = d.drain_out_frames();
        assert!(
            String::from_utf8_lossy(match &out[0] {
                OutFrame::Stdin(b) => b,
                _ => panic!(),
            })
            .contains("session/new")
        );

        let events = d.translate(
            br#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"019f4f1c-8ae8-7632-adb2-6133aee3adf3"}}"#,
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(&e.payload, Payload::SessionStarted(s) if s.session_id == "019f4f1c-8ae8-7632-adb2-6133aee3adf3"))
        );
        // Bootstrap open must NOT fake a public `/new` command completion.
        assert!(
            !events.iter().any(|e| matches!(
                &e.payload,
                Payload::CommandResult(c) if c.command == "new" || c.command.contains("new")
            )),
            "bootstrap SessionNew must not emit NewConversation command result: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(&e.payload, Payload::TurnCompleted(_))),
            "bootstrap SessionNew must not emit TurnCompleted: {events:?}"
        );
    }

    #[test]
    fn command_new_emits_new_conversation_public_events() {
        let mut d = GrokAcp::new();
        ready_session(&mut d);
        let dispatch = d
            .handle_system_command(&AgentCommandInvocation {
                name: "new".into(),
                args: None,
                values: Value::Null,
                source: AgentCommandSource::AdapterMapped,
            })
            .expect("new");
        assert!(matches!(dispatch, CommandDispatch::Deferred(_)));
        // id was 3 after ready_session used 1,2
        let events = d.translate(
            br#"{"jsonrpc":"2.0","id":3,"result":{"sessionId":"sid-after-new","models":{"currentModelId":"grok-4.5","availableModels":[]}}}"#,
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(&e.payload, Payload::SessionStarted(s) if s.session_id == "sid-after-new")),
            "{events:?}"
        );
        assert!(
            events.iter().any(|e| matches!(
                &e.payload,
                Payload::TurnCompleted(_)
            )),
            "command /new must public-complete: {events:?}"
        );
    }

    #[test]
    fn foreign_session_update_ignored() {
        let mut d = GrokAcp::new();
        d.session_id = Some("expected".into());
        d.state = SessionState::Ready;
        d.session_started = true;
        let events = d.translate(
            br#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"other","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"nope"}}}}"#,
        );
        assert!(events.is_empty());
    }

    #[test]
    fn resume_uses_session_load() {
        let mut d = GrokAcp::new();
        let mut data = BTreeMap::new();
        data.insert(
            "session_id".into(),
            Value::String("uuid-resume".into()),
        );
        let frames = d.init(&SessionParams {
            cwd: "/tmp".into(),
            resume: Some(ResumeHandle { version: 1, data }),
            ..Default::default()
        });
        assert!(!frames.is_empty());
        let _ = d.translate(
            br#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}}"#,
        );
        let out = d.drain_out_frames();
        let line = String::from_utf8_lossy(match &out[0] {
            OutFrame::Stdin(b) => b,
            _ => panic!(),
        });
        assert!(line.contains("session/load"));
        assert!(line.contains("uuid-resume"));
    }

    #[test]
    fn fs_read_text_file_returns_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, "hello fs\n").unwrap();
        let mut d = GrokAcp::new();
        let req = json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "fs/read_text_file",
            "params": { "sessionId": "s", "path": path.to_string_lossy() }
        });
        let events = d.translate(serde_json::to_string(&req).unwrap().as_bytes());
        assert!(events.is_empty());
        let out = d.drain_out_frames();
        assert_eq!(out.len(), 1);
        let line = String::from_utf8_lossy(match &out[0] {
            OutFrame::Stdin(b) => b,
            _ => panic!("expected stdin response"),
        });
        let resp: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(resp["id"], 0);
        assert_eq!(resp["result"]["content"], "hello fs\n");
    }

    #[test]
    fn fs_write_text_file_writes_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        let mut d = GrokAcp::new();
        let req = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "fs/write_text_file",
            "params": {
                "sessionId": "s",
                "path": path.to_string_lossy(),
                "content": "written\n"
            }
        });
        let _ = d.translate(serde_json::to_string(&req).unwrap().as_bytes());
        let out = d.drain_out_frames();
        let line = String::from_utf8_lossy(match &out[0] {
            OutFrame::Stdin(b) => b,
            _ => panic!(),
        });
        let resp: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(resp["id"], 7);
        assert!(resp.get("result").is_some());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "written\n");
    }

    #[test]
    fn unknown_reverse_rpc_returns_method_not_found() {
        let mut d = GrokAcp::new();
        let events = d.translate(
            br#"{"jsonrpc":"2.0","id":9,"method":"terminal/create","params":{}}"#,
        );
        assert!(events.is_empty());
        let out = d.drain_out_frames();
        let line = String::from_utf8_lossy(match &out[0] {
            OutFrame::Stdin(b) => b,
            _ => panic!(),
        });
        let resp: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(resp["id"], 9);
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[test]
    fn apply_line_window_slices_1_based() {
        let full = "a\nb\nc\nd";
        assert_eq!(apply_line_window(full, Some(2), Some(2)), "b\nc");
        assert_eq!(apply_line_window(full, Some(4), None), "d");
        assert_eq!(apply_line_window(full, None, None), full);
    }

    fn ready_session(d: &mut GrokAcp) {
        d.init(&SessionParams {
            cwd: "/tmp/project".into(),
            model: "grok-4.5".into(),
            permission_mode: PermissionMode::Bypass,
            ..Default::default()
        });
        let _ = d.translate(
            br#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"_meta":{"agentVersion":"0.2.93"}}}"#,
        );
        let _ = d.drain_out_frames();
        let _ = d.translate(
            br#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"sid-cmd","models":{"currentModelId":"grok-4.5","availableModels":[{"modelId":"grok-4.5","name":"Grok 4.5","description":"frontier","_meta":{"reasoningEffort":"high","reasoningEfforts":[{"id":"high"}]}}]}}}"#,
        );
    }

    #[test]
    fn available_commands_update_builds_native_catalog_excluding_adapter_mapped() {
        let mut d = GrokAcp::new();
        ready_session(&mut d);
        let _ = d.translate(
            br#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sid-cmd","update":{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"compact","description":"Compress"},{"name":"status","description":"should be filtered"},{"name":"help","description":"Help"},{"name":"grill-me","description":"skill","_meta":{"scope":"user","path":"/skills/grill-me/SKILL.md"}}]}}}"#,
        );
        let catalog = d.command_catalog();
        let names: Vec<_> = catalog.commands.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"compact"), "{names:?}");
        assert!(names.contains(&"help"), "{names:?}");
        assert!(names.contains(&"grill-me"), "{names:?}");
        for banned in ADAPTER_MAPPED_COMMANDS {
            assert!(
                !names.iter().any(|n| n.eq_ignore_ascii_case(banned)),
                "catalog must not include AdapterMapped {banned}: {names:?}"
            );
        }
        assert!(catalog
            .commands
            .iter()
            .all(|c| c.source == AgentCommandSource::ProviderNative));
    }

    #[test]
    fn system_commands_status_model_permissions_skills_list_commands_new_fork_quit() {
        let mut d = GrokAcp::new();
        ready_session(&mut d);
        let _ = d.translate(
            br#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sid-cmd","update":{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"compact","description":"c"},{"name":"tdd","description":"skill","_meta":{"path":"/x/skills/tdd/SKILL.md"}}]}}}"#,
        );

        for name in [
            "status",
            "model",
            "permissions",
            "skills",
            "list_commands",
            "fork",
            "quit",
        ] {
            let dispatch = d
                .handle_system_command(&AgentCommandInvocation {
                    name: name.into(),
                    args: None,
                    values: Value::Null,
                    source: AgentCommandSource::AdapterMapped,
                })
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            match name {
                "quit" => assert!(matches!(dispatch, CommandDispatch::Ready(CommandResult::Quit))),
                "status" | "model" | "permissions" | "skills" | "list_commands" | "fork" => {
                    assert!(
                        matches!(dispatch, CommandDispatch::Ready(_)),
                        "{name} should be ready: {dispatch:?}"
                    );
                }
                _ => {}
            }
        }

        // model set + permissions set + new + fork with target
        let set_model = d
            .handle_system_command(&AgentCommandInvocation {
                name: "model".into(),
                args: Some("grok-4.5 high".into()),
                values: Value::Null,
                source: AgentCommandSource::AdapterMapped,
            })
            .expect("set model");
        let CommandDispatch::Deferred(model_frames) = set_model else {
            panic!("set model deferred");
        };
        let model_wire = model_frames
            .iter()
            .filter_map(|f| match f {
                OutFrame::Stdin(b) => Some(String::from_utf8_lossy(b).into_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            model_wire.contains("session/set_model") && model_wire.contains("reasoningEffort"),
            "set_model must send reasoningEffort on wire: {model_wire}"
        );
        assert!(
            model_wire.contains("\"modelId\":\"grok-4.5\"")
                || model_wire.contains("\"modelId\": \"grok-4.5\""),
            "set_model modelId: {model_wire}"
        );
        // Dual-path slash when reasoning present.
        assert!(
            model_wire.contains("/model grok-4.5 high") || model_wire.contains("session/prompt"),
            "reasoning set should dual-path slash/prompt: {model_wire}"
        );

        let set_perm = d
            .handle_system_command(&AgentCommandInvocation {
                name: "permissions".into(),
                args: Some("always-approve".into()),
                values: Value::Null,
                source: AgentCommandSource::AdapterMapped,
            })
            .expect("set permissions");
        let CommandDispatch::Deferred(perm_frames) = set_perm else {
            panic!("set permissions must deferred so agent receives slash: {set_perm:?}");
        };
        let perm_wire = perm_frames
            .iter()
            .filter_map(|f| match f {
                OutFrame::Stdin(b) => Some(String::from_utf8_lossy(b).into_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            perm_wire.contains("session/prompt") && perm_wire.contains("/always-approve"),
            "permissions set must send slash to agent: {perm_wire}"
        );

        let fork_cmd = d
            .handle_system_command(&AgentCommandInvocation {
                name: "fork".into(),
                args: Some("continue here".into()),
                values: Value::Null,
                source: AgentCommandSource::AdapterMapped,
            })
            .expect("fork");
        let CommandDispatch::Deferred(fork_frames) = fork_cmd else {
            panic!("fork must deferred with wire frames");
        };
        assert!(
            !fork_frames.is_empty(),
            "fork Deferred must not be empty after dual-path enqueue"
        );
        let fork_wire = fork_frames
            .iter()
            .filter_map(|f| match f {
                OutFrame::Stdin(b) => Some(String::from_utf8_lossy(b).into_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            fork_wire.contains("x.ai/session/fork"),
            "fork must enqueue x.ai/session/fork: {fork_wire}"
        );
        assert!(
            fork_wire.contains("/fork continue here") || fork_wire.contains("session/prompt"),
            "fork must dual-path slash prompt: {fork_wire}"
        );

        let new_cmd = d
            .handle_system_command(&AgentCommandInvocation {
                name: "new".into(),
                args: None,
                values: Value::Null,
                source: AgentCommandSource::AdapterMapped,
            })
            .expect("new");
        assert!(matches!(new_cmd, CommandDispatch::Deferred(_)));
    }

    #[test]
    fn native_compact_dispatches_slash_prompt() {
        let mut d = GrokAcp::new();
        ready_session(&mut d);
        let _ = d.translate(
            br#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sid-cmd","update":{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"compact","description":"Compress"}]}}}"#,
        );
        let dispatch = d
            .handle_native_command(&AgentCommandInvocation {
                name: "compact".into(),
                args: None,
                values: Value::Null,
                source: AgentCommandSource::ProviderNative,
            })
            .expect("native compact");
        let CommandDispatch::Deferred(frames) = dispatch else {
            panic!("expected deferred frames");
        };
        let line = String::from_utf8_lossy(match &frames[0] {
            OutFrame::Stdin(b) => b,
            _ => panic!(),
        });
        assert!(line.contains("session/prompt"), "{line}");
        assert!(line.contains("/compact"), "{line}");
    }

    #[test]
    fn models_update_notification_refreshes_catalog() {
        let mut d = GrokAcp::new();
        ready_session(&mut d);
        let _ = d.translate(
            br#"{"jsonrpc":"2.0","method":"_x.ai/models/update","params":{"currentModelId":"grok-4.5","availableModels":[{"modelId":"grok-4.5","name":"Grok 4.5"},{"modelId":"other","name":"Other"}]}}"#,
        );
        assert_eq!(d.models.models.len(), 2);
        assert_eq!(d.models.current_model.as_deref(), Some("grok-4.5"));
    }
}
