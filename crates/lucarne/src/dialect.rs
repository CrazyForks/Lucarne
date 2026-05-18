//! Dialect — the vendor-specific schema layer.
//!
//! A `Dialect` owns three translations:
//!
//! 1. Incoming frame → zero-or-more canonical [`crate::event::Event`]s.
//! 2. Outgoing user input → one or more [`OutFrame`]s.
//! 3. Permission decisions / interrupts → [`OutFrame`]s.
//!
//! Dialects are stateless where possible; stateful ones (Codex) keep
//! per-session state in the implementation. The [`crate::runtime`]
//! routes `OutFrame`s to stdin / signals.

use crate::agent_runtime::{
    AgentCommandCatalog as CommandCatalog, AgentCommandInvocation as CommandInvocation,
    AgentForkResult as ForkResult, AgentForkTargetCatalog as ForkTargetCatalog,
    AgentModelCatalog as ModelCatalog, AgentModelSelection,
    AgentPermissionCatalog as PermissionCatalog, AgentPermissionSelection,
    AgentReasoningOption as Reasoning, AgentSkillCatalog as SkillCatalog, SessionRef,
};
use crate::error::{LucarneError, Result};
use crate::event::{
    self, CommandResultData, CommandResultPayload, Decision, Event, Payload, PermissionResponse,
    ResumeHandle, Timeline, TurnCompleted,
};
use smol_str::SmolStr;
use std::collections::BTreeMap;

/// A single outbound frame produced by a dialect. The variant itself
/// encodes where the payload goes; all byte frames go to child stdin
/// regardless of whether the bytes contain raw text or JSON-RPC.
#[derive(Debug, Clone)]
pub enum OutFrame {
    /// Bytes to child stdin.
    Stdin(Vec<u8>),
    /// Close the child's stdin (EOF).
    CloseStdin,
    /// Send a POSIX signal by name (e.g. "SIGINT", "SIGTERM") to the
    /// agent process.
    Signal(String),
}

impl OutFrame {
    pub fn stdin(bytes: Vec<u8>) -> Self {
        Self::Stdin(bytes)
    }
    pub fn close_stdin() -> Self {
        Self::CloseStdin
    }
    pub fn signal(name: impl Into<String>) -> Self {
        Self::Signal(name.into())
    }
    pub fn rpc_request(bytes: Vec<u8>) -> Self {
        Self::Stdin(bytes)
    }
    pub fn rpc_response(bytes: Vec<u8>) -> Self {
        Self::Stdin(bytes)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Input {
    pub text: String,
    pub images: Vec<ImageRef>,
}

#[derive(Debug, Clone)]
pub struct ImageRef {
    pub media_type: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct SessionParams {
    pub model: String,
    pub system_prompt: String,
    pub cwd: String,
    pub first_prompt: String,
    pub resume: Option<ResumeHandle>,
    pub resume_session_at: String,
    pub extra_env: BTreeMap<String, String>,
    pub extra_args: Vec<String>,
    pub permission_mode: PermissionMode,
}

impl SessionParams {
    pub fn resume_data(&self) -> Option<&BTreeMap<String, serde_json::Value>> {
        self.resume.as_ref().map(|resume| &resume.data)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PermissionMode {
    #[default]
    Default,
    Write,
    ReadOnly,
    Auto,
    Full,
    Bypass,
}

impl PermissionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Write => "write",
            Self::ReadOnly => "read_only",
            Self::Auto => "auto",
            Self::Full => "full",
            Self::Bypass => "bypass",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "" | "default" => Some(Self::Default),
            "write" => Some(Self::Write),
            "read_only" => Some(Self::ReadOnly),
            "auto" => Some(Self::Auto),
            "full" => Some(Self::Full),
            "bypass" => Some(Self::Bypass),
            _ => None,
        }
    }

    pub const fn accepted_values() -> &'static [&'static str] {
        &["default", "write", "read_only", "auto", "full", "bypass"]
    }
}

#[derive(Debug, Clone)]
pub enum CommandDispatch {
    Ready(CommandResult),
    Deferred(Vec<OutFrame>),
}

impl CommandDispatch {
    pub fn ready(result: CommandResult) -> Self {
        Self::Ready(result)
    }

    pub fn deferred(frames: Vec<OutFrame>) -> Self {
        Self::Deferred(frames)
    }
}

impl From<Vec<OutFrame>> for CommandDispatch {
    fn from(frames: Vec<OutFrame>) -> Self {
        Self::deferred(frames)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CommandResult {
    Models(ModelCatalog),
    ModelChanged(ModelSelection),
    Permissions(PermissionCatalog),
    PermissionsChanged(PermissionSelection),
    Status(crate::agent_runtime::AgentStatus),
    Skills(SkillCatalog),
    NewConversation(Conversation),
    Quit,
    Forked(ForkResult),
    ForkTargets(ForkTargetCatalog),
    Commands(CommandCatalog),
    Message(CommandMessage),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSelection {
    pub model: SmolStr,
    pub reasoning: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionSelection {
    pub mode: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Conversation {
    pub session_ref: Option<SessionRef>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandMessage {
    pub title: Option<SmolStr>,
    pub text: SmolStr,
    pub data: serde_json::Value,
}

impl CommandMessage {
    pub fn text(text: impl Into<SmolStr>) -> Self {
        Self {
            title: None,
            text: text.into(),
            data: serde_json::Value::Null,
        }
    }
}

pub fn command_result_events(turn_id: &str, result: CommandResult) -> Vec<Event> {
    let text = command_result_text(&result);
    let turn_id = turn_id.to_string();
    let mut events = Vec::new();
    if let Some(result) = command_result_payload(&result) {
        events.push(Event::new(Payload::CommandResult(CommandResultPayload {
            command: command_result_name(&result).to_string(),
            result,
        })));
    }
    events.push(Event::new(Payload::Timeline(Timeline {
        item: event::new_timeline_assistant(&turn_id, &text, false),
    })));
    events.push(Event::new(Payload::TurnCompleted(TurnCompleted {
        turn_id,
        usage: None,
    })));
    events
}

fn command_result_payload(result: &CommandResult) -> Option<CommandResultData> {
    match result {
        CommandResult::Models(catalog) => Some(CommandResultData::Models(catalog.clone())),
        CommandResult::ModelChanged(selection) => {
            Some(CommandResultData::ModelChanged(AgentModelSelection {
                model: selection.model.clone(),
                reasoning: selection.reasoning.clone(),
            }))
        }
        CommandResult::Permissions(catalog) => {
            Some(CommandResultData::Permissions(catalog.clone()))
        }
        CommandResult::PermissionsChanged(selection) => Some(
            CommandResultData::PermissionsChanged(AgentPermissionSelection {
                mode: selection.mode.clone(),
            }),
        ),
        CommandResult::Status(status) => Some(CommandResultData::Status(status.clone())),
        CommandResult::Skills(catalog) => Some(CommandResultData::Skills(catalog.clone())),
        CommandResult::Forked(result) => Some(CommandResultData::Forked(result.clone())),
        CommandResult::ForkTargets(catalog) => {
            Some(CommandResultData::ForkTargets(catalog.clone()))
        }
        CommandResult::Commands(catalog) => Some(CommandResultData::Commands(catalog.clone())),
        CommandResult::Message(message) => Some(CommandResultData::Text {
            text: message.text.clone(),
        }),
        CommandResult::NewConversation(_) | CommandResult::Quit => None,
    }
}

fn command_result_name(result: &CommandResultData) -> &'static str {
    match result {
        CommandResultData::Models(_) => "model",
        CommandResultData::ModelChanged(_) => "model",
        CommandResultData::Permissions(_) => "permissions",
        CommandResultData::PermissionsChanged(_) => "permissions",
        CommandResultData::Status(_) => "status",
        CommandResultData::Skills(_) => "skills",
        CommandResultData::Forked(_) => "fork",
        CommandResultData::ForkTargets(_) => "fork",
        CommandResultData::Commands(_) => "commands",
        CommandResultData::Text { .. } => "message",
    }
}

pub fn command_result_text(result: &CommandResult) -> String {
    match result {
        CommandResult::Models(catalog) => models_text(catalog),
        CommandResult::ModelChanged(selection) => match selection.reasoning.as_deref() {
            Some(reasoning) => format!(
                "Updated model to {} with reasoning effort {}.",
                selection.model, reasoning
            ),
            None => format!("Updated model to {}.", selection.model),
        },
        CommandResult::Permissions(catalog) => permissions_text(catalog),
        CommandResult::PermissionsChanged(selection) => {
            format!("Updated permissions to {}.", selection.mode)
        }
        CommandResult::Status(status) => status_text(status),
        CommandResult::Skills(catalog) => skills_text(catalog),
        CommandResult::NewConversation(conversation) => match &conversation.session_ref {
            Some(session_ref) => format!("Started new thread {}.", session_ref.0),
            None => "Started new thread.".into(),
        },
        CommandResult::Quit => "Quit session.".into(),
        CommandResult::Forked(result) => match &result.session_ref {
            Some(session_ref) => format!("Started new thread {}.", session_ref.0),
            None => "Forked current thread.".into(),
        },
        CommandResult::ForkTargets(catalog) => fork_targets_text(catalog),
        CommandResult::Commands(catalog) => commands_text(catalog),
        CommandResult::Message(message) => message.text.to_string(),
    }
}

fn fork_targets_text(catalog: &ForkTargetCatalog) -> String {
    if catalog.targets.is_empty() {
        return "Fork targets:\n(none)".into();
    }
    let mut lines = vec!["Fork targets:".to_string()];
    for (index, target) in catalog.targets.iter().enumerate() {
        let label = target.label.as_deref().unwrap_or(target.id.as_str());
        let mut line = format!("{}. `{}`", index + 1, target.id);
        if label != target.id.as_str() {
            line.push_str(" — ");
            line.push_str(label);
        }
        if let Some(description) = &target.description {
            line.push_str(" — ");
            line.push_str(description);
        }
        lines.push(line);
    }
    lines.join("\n")
}

fn models_text(catalog: &ModelCatalog) -> String {
    let mut lines = vec![
        "Available models:".to_string(),
        format!(
            "current: `{}`",
            catalog.current_model.as_deref().unwrap_or("unknown")
        ),
    ];
    if catalog.models.is_empty() {
        lines.push("(none)".into());
    } else {
        for (index, model) in catalog.models.iter().enumerate() {
            let mut line = format!("{}. `{}`", index + 1, model.id);
            if let Some(name) = &model.display_name {
                if name.as_str() != model.id.as_str() {
                    line.push_str(" — ");
                    line.push_str(name);
                }
            }
            if let Some(description) = &model.description {
                line.push_str(" — ");
                line.push_str(description);
            }
            if !model.supported_reasoning.is_empty() {
                line.push_str(" — reasoning: ");
                line.push_str(&reasoning_text(&model.supported_reasoning));
            }
            lines.push(line);
        }
    }
    lines.join("\n")
}

fn reasoning_text(items: &[Reasoning]) -> String {
    items
        .iter()
        .map(|item| match item.is_default {
            Some(true) => format!("`{}` (default)", item.value),
            _ => format!("`{}`", item.value),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn permissions_text(catalog: &PermissionCatalog) -> String {
    let mut lines = vec![
        "Permission modes:".to_string(),
        format!(
            "current: `{}`",
            catalog.current_mode.as_deref().unwrap_or("unknown")
        ),
    ];
    if catalog.modes.is_empty() {
        lines.push("(none)".into());
    } else {
        for (index, mode) in catalog.modes.iter().enumerate() {
            let mut line = format!("{}. `{}`", index + 1, mode.id);
            if let Some(name) = &mode.display_name {
                if name.as_str() != mode.id.as_str() {
                    line.push_str(" — ");
                    line.push_str(name);
                }
            }
            if let Some(description) = &mode.description {
                line.push_str(" — ");
                line.push_str(description);
            }
            lines.push(line);
        }
    }
    lines.join("\n")
}

fn status_text(status: &crate::agent_runtime::AgentStatus) -> String {
    let mut lines = vec!["Status:".to_string()];
    if let Some(version) = status.version.as_deref() {
        lines.push(format!("Version: `{version}`"));
    }
    if let Some(model) = status.model.as_deref() {
        let mut line = format!("Model: `{model}`");
        match (status.model_detail.as_deref(), status.reasoning.as_deref()) {
            (Some(detail), Some(reasoning)) => {
                line.push_str(&format!(" (`{detail}`, reasoning {reasoning})"));
            }
            (Some(detail), None) => {
                line.push_str(&format!(" (`{detail}`)"));
            }
            (None, Some(reasoning)) => {
                line.push_str(&format!(" (reasoning {reasoning})"));
            }
            (None, None) => {}
        }
        lines.push(line);
    }
    if let Some(directory) = status.directory.as_deref() {
        lines.push(format!("Directory: `{directory}`"));
    }
    if let Some(permissions) = status.permissions.as_deref() {
        lines.push(format!("Permissions: `{permissions}`"));
    }
    if let Some(path) = status.agents_md.as_deref() {
        lines.push(format!("Agents.md: `{path}`"));
    }
    if let Some(account) = status.account.as_deref() {
        lines.push(format!("Account: {account}"));
    }
    if let Some(base_url) = status.base_url.as_deref() {
        lines.push(format!("Base URL: `{base_url}`"));
    }
    if let Some(proxy) = status.proxy.as_deref() {
        lines.push(format!("Proxy: `{proxy}`"));
    }
    if let Some(sources) = status.setting_sources.as_deref() {
        lines.push(format!("Setting sources: {sources}"));
    }
    if let Some(session_id) = status.session_id.as_deref() {
        lines.push(format!("Session: `{session_id}`"));
    }
    if let Some(tokens) = &status.tokens {
        if tokens.input_tokens.is_some() || tokens.output_tokens.is_some() {
            lines.push(format!(
                "Token usage: {} in / {} out",
                compact_number(tokens.input_tokens.unwrap_or(0)),
                compact_number(tokens.output_tokens.unwrap_or(0))
            ));
        }
    }
    if let Some(context) = &status.context {
        if let (Some(used), Some(max)) = (context.used_tokens, context.max_tokens) {
            let percent = context.percent_used.unwrap_or_else(|| {
                if max == 0 {
                    0
                } else {
                    ((used as f64 / max as f64) * 100.0).round() as u8
                }
            });
            lines.push(format!(
                "Context: {}/{} ({}%)",
                compact_number(used),
                compact_number(max),
                percent
            ));
        }
    }
    if let Some(compactions) = status.compactions {
        lines.push(format!("Compactions: {compactions}"));
    }
    lines.join("\n")
}

fn compact_number(value: u64) -> String {
    if value >= 1_000_000 {
        let n = value as f64 / 1_000_000.0;
        trim_number(n, "m")
    } else if value >= 1_000 {
        let n = value as f64 / 1_000.0;
        trim_number(n, "k")
    } else {
        value.to_string()
    }
}

fn trim_number(value: f64, suffix: &str) -> String {
    let rendered = format!("{value:.1}");
    format!(
        "{}{}",
        rendered.trim_end_matches('0').trim_end_matches('.'),
        suffix
    )
}

fn skills_text(catalog: &SkillCatalog) -> String {
    if catalog.skills.is_empty() {
        return "Available skills:\n(none)".into();
    }
    let mut lines = vec!["Available skills:".to_string()];
    for (index, skill) in catalog.skills.iter().enumerate() {
        let title = skill.display_name.as_deref().unwrap_or(skill.name.as_str());
        let mut line = if title == skill.name.as_str() {
            format!("{}. `{}`", index + 1, skill.name)
        } else {
            format!("{}. {title}", index + 1)
        };
        if title != skill.name.as_str() {
            line.push_str(&format!("\n   `{}`", skill.name));
        }
        if let Some(description) = &skill.description {
            line.push_str("\n   ");
            line.push_str(description);
        }
        if let Some(source) = &skill.source {
            line.push_str("\n   source: ");
            line.push_str(source);
        }
        if let Some(tokens) = skill.tokens {
            line.push_str(&format!("\n   tokens: {tokens}"));
        }
        lines.push(line);
    }
    lines.join("\n")
}

fn commands_text(catalog: &CommandCatalog) -> String {
    if catalog.commands.is_empty() {
        return "Commands:\n(none)".into();
    }
    let mut lines = vec!["Commands:".to_string()];
    for command in &catalog.commands {
        let mut line = format!("`/{}`", command.name);
        if let Some(description) = &command.description {
            line.push_str(" — ");
            line.push_str(description);
        }
        lines.push(line);
    }
    lines.join("\n")
}

pub fn normalize_agent_command_name(raw: &str) -> &str {
    raw.trim().trim_start_matches('/')
}

#[cfg(any(feature = "claude", feature = "gemini", feature = "pi"))]
pub(crate) fn model_args(command: &CommandInvocation) -> Option<(String, Option<String>)> {
    let model = command
        .values
        .get("model")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let reasoning = command
        .values
        .get("reasoning")
        .or_else(|| command.values.get("effort"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(model) = model {
        return Some((model.into(), reasoning.map(Into::into)));
    }

    let raw = command.args.as_deref().unwrap_or("").trim();
    if raw.is_empty() {
        return None;
    }
    let mut parts = raw.split_whitespace();
    let model = parts.next().unwrap_or_default();
    let reasoning = parts.next();
    Some((model.into(), reasoning.map(Into::into)))
}

#[cfg(any(feature = "claude", feature = "gemini", feature = "pi"))]
pub(crate) fn permission_arg(command: &CommandInvocation) -> Option<String> {
    let mode = command
        .values
        .get("mode")
        .or_else(|| command.values.get("permissionMode"))
        .or_else(|| command.values.get("approval_policy"))
        .and_then(serde_json::Value::as_str)
        .or(command.args.as_deref())
        .unwrap_or("")
        .trim();
    (!mode.is_empty()).then(|| mode.into())
}

#[cfg(any(feature = "claude", feature = "pi"))]
pub(crate) fn fork_name(command: &CommandInvocation) -> Option<String> {
    command
        .values
        .get("target_id")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            command
                .values
                .get("name")
                .and_then(serde_json::Value::as_str)
        })
        .or(command.args.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(Into::into)
}

/// A dialect is implemented once per agent family. Implementations are
/// expected to be `Send` so they can be owned by a per-session runtime
/// task.
pub trait Dialect: Send {
    fn name(&self) -> &'static str;

    /// Handshake frames sent right after the process starts.
    fn init(&mut self, cfg: &SessionParams) -> Vec<OutFrame> {
        let _ = cfg;
        Vec::new()
    }

    fn translate(&mut self, frame_bytes: &[u8]) -> Vec<Event>;

    /// Additional outbound frames produced reactively while translating
    /// (e.g. Codex reverse-RPC acks). Drained by the runtime after each
    /// `translate`.
    fn drain_out_frames(&mut self) -> Vec<OutFrame> {
        Vec::new()
    }

    fn encode_user_message(&mut self, input: &Input) -> Result<Vec<OutFrame>>;

    fn command_catalog(&self) -> CommandCatalog {
        CommandCatalog::default()
    }

    fn handle_system_command(&mut self, command: &CommandInvocation) -> Result<CommandDispatch> {
        Err(LucarneError::dialect(format!(
            "{}: system command {:?} is not supported",
            self.name(),
            command.name
        )))
    }

    fn handle_native_command(&mut self, command: &CommandInvocation) -> Result<CommandDispatch> {
        Err(LucarneError::dialect(format!(
            "{}: native command {:?} is not supported",
            self.name(),
            command.name
        )))
    }

    fn encode_permission(&mut self, req_id: &str, decision: Decision) -> Result<Vec<OutFrame>> {
        self.encode_permission_response(req_id, &PermissionResponse::from_decision(decision))
    }

    fn encode_permission_response(
        &mut self,
        req_id: &str,
        resp: &PermissionResponse,
    ) -> Result<Vec<OutFrame>>;

    fn encode_interrupt(&mut self) -> Result<Vec<OutFrame>>;

    fn on_exit(&mut self, exit_code: i32, err: Option<String>) -> Vec<Event> {
        let _ = (exit_code, err);
        Vec::new()
    }
}
