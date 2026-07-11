use std::collections::BTreeMap;

use smol_str::SmolStr;

#[cfg(any(
    feature = "claude",
    feature = "codex",
    feature = "cursor",
    feature = "gemini",
    feature = "grok",
    feature = "pi"
))]
pub(crate) mod projection;

pub(crate) fn smol_opt<T>(value: Option<T>) -> Option<SmolStr>
where
    T: Into<SmolStr>,
{
    value.map(Into::into)
}

/// Identifies which agent produced a session.
///
/// This is an open newtype rather than a closed enum so provider ids stay
/// owned by provider descriptors instead of a common catalog.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentKind(pub SmolStr);

impl AgentKind {
    pub fn new(name: impl Into<SmolStr>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for AgentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identifies the format version of a parsed session.
///
/// Like [`AgentKind`], this is an open newtype so provider version tags stay
/// provider-owned rather than listed in a common catalog.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VersionKind(pub SmolStr);

impl VersionKind {
    pub fn new(name: impl Into<SmolStr>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for VersionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionMeta {
    pub session_id: Option<SmolStr>,
    pub thread_id: Option<SmolStr>,
    pub cwd: Option<SmolStr>,
    pub title: Option<SmolStr>,
    pub models: Box<[SessionModelMeta]>,
    pub created_at: Option<SmolStr>,
    pub updated_at: Option<SmolStr>,
    pub source_kind: Option<SmolStr>,
    pub extra_json: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionModelMeta {
    pub model: SmolStr,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub cached_tokens: u64,
    pub reasoning_tokens: u64,
    pub tool_tokens: u64,
    pub total_tokens: u64,
    pub web_search_requests: u64,
}

impl SessionModelMeta {
    pub fn zero(model: impl Into<SmolStr>) -> Self {
        Self {
            model: model.into(),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub agent: AgentKind,
    pub version: VersionKind,
    pub meta: SessionMeta,
    pub events: Box<[Event]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub id: Option<SmolStr>,
    pub timestamp: Option<SmolStr>,
    pub actor: Actor,
    pub turn_id: Option<SmolStr>,
    pub op_id: Option<SmolStr>,
    pub parent_op_id: Option<SmolStr>,
    pub body: Body,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    User,
    Assistant,
    System,
    Tool,
    Subagent,
    Other(SmolStr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Prompt(Prompt),
    Response(Response),
    Operation(Operation),
    Usage(Usage),
    State(State),
    Snapshot(Snapshot),
    Unknown(Unknown),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Prompt {
    pub text: Option<SmolStr>,
    pub blocks: Box<[ContentBlock]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Response {
    pub model: Option<SmolStr>,
    pub phase: Option<SmolStr>,
    pub text: Option<SmolStr>,
    pub blocks: Box<[ContentBlock]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Operation {
    pub kind: OperationKind,
    pub phase: OperationPhase,
    pub name: SmolStr,
    pub input_json: Option<SmolStr>,
    pub output_json: Option<SmolStr>,
    pub command: Option<SmolStr>,
    pub file_path: Option<SmolStr>,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub is_error: bool,
    pub duration_seconds: Option<f64>,
    pub extra_json: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationKind {
    Shell,
    Read,
    Write,
    Edit,
    Search,
    Fetch,
    Web,
    Mcp,
    Subagent,
    Task,
    Custom(SmolStr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationPhase {
    Requested,
    Started,
    Progress,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Usage {
    pub model: Option<SmolStr>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub cached_tokens: u64,
    pub reasoning_tokens: u64,
    pub tool_tokens: u64,
    pub total_tokens: u64,
    pub web_search_requests: u64,
    pub speed: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub kind: SmolStr,
    pub value_json: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub kind: SmolStr,
    pub value_json: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unknown {
    pub kind: SmolStr,
    pub raw_json: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentBlock {
    Text(TextBlock),
    Image(ImageBlock),
    Thinking(ThinkingBlock),
    ToolUse(ToolUseBlock),
    ToolResult(ToolResultBlock),
    Raw(RawBlock),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextBlock {
    pub text: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageBlock {
    pub image_url: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingBlock {
    pub text: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolUseBlock {
    pub id: Option<SmolStr>,
    pub name: SmolStr,
    pub input_json: Option<SmolStr>,
    pub command: Option<SmolStr>,
    pub file_path: Option<SmolStr>,
    pub lines_added: u64,
    pub lines_removed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResultBlock {
    pub tool_use_id: Option<SmolStr>,
    pub content: SmolStr,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawBlock {
    pub kind: SmolStr,
    pub raw_json: SmolStr,
}

pub fn event(actor: Actor, body: Body, timestamp: Option<SmolStr>) -> Event {
    Event {
        id: None,
        timestamp: timestamp.map(Into::into),
        actor,
        turn_id: None,
        op_id: None,
        parent_op_id: None,
        body,
    }
}

pub(crate) fn filter_selection(mut session: Session, selection: crate::ParseSelection) -> Session {
    if selection.is_full() {
        return session;
    }
    if !selection.includes_meta() {
        session.meta = SessionMeta::default();
    }
    session.events = session
        .events
        .into_vec()
        .into_iter()
        .filter(|event| selection.includes_body(&event.body))
        .collect::<Vec<_>>()
        .into_boxed_slice();
    session
}

impl crate::ParseSelection {
    pub(crate) fn includes_body(self, body: &Body) -> bool {
        match body {
            Body::Prompt(_) | Body::Response(_) => self.includes_messages(),
            Body::Operation(_) => self.includes_operations(),
            Body::Usage(_) => self.includes_usage(),
            Body::State(_) => self.includes_state(),
            Body::Snapshot(_) => self.includes_snapshots(),
            Body::Unknown(_) => self.includes_raw_unknown(),
        }
    }
}

pub fn summarize_models(events: &[Event]) -> Box<[SessionModelMeta]> {
    let mut by_model = BTreeMap::<String, SessionModelMeta>::new();

    for event in events {
        match &event.body {
            Body::Response(response) if matches!(event.actor, Actor::Assistant) => {
                if let Some(model) = response.model.as_deref() {
                    by_model
                        .entry(model.to_string())
                        .or_insert_with(|| SessionModelMeta::zero(model));
                }
            }
            Body::Usage(usage) => {
                let Some(model) = usage.model.as_deref() else {
                    continue;
                };
                let summary = by_model
                    .entry(model.to_string())
                    .or_insert_with(|| SessionModelMeta::zero(model));
                summary.input_tokens += usage.input_tokens;
                summary.output_tokens += usage.output_tokens;
                summary.cache_creation_tokens += usage.cache_creation_tokens;
                summary.cache_read_tokens += usage.cache_read_tokens;
                summary.cached_tokens += usage.cached_tokens;
                summary.reasoning_tokens += usage.reasoning_tokens;
                summary.tool_tokens += usage.tool_tokens;
                summary.total_tokens += usage.total_tokens;
                summary.web_search_requests += usage.web_search_requests;
            }
            _ => {}
        }
    }

    by_model
        .into_values()
        .collect::<Vec<_>>()
        .into_boxed_slice()
}

pub fn text_from_blocks(blocks: &[ContentBlock]) -> Option<SmolStr> {
    let parts: Vec<&str> = blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(block) => Some(block.text.as_str()),
            _ => None,
        })
        .filter(|text| !text.is_empty())
        .collect();

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" ").into())
    }
}

pub fn earliest_timestamp(events: &[Event]) -> Option<SmolStr> {
    events
        .iter()
        .filter_map(|event| event.timestamp.as_deref())
        .min()
        .map(Into::into)
}

pub fn latest_timestamp(events: &[Event]) -> Option<SmolStr> {
    events
        .iter()
        .filter_map(|event| event.timestamp.as_deref())
        .max()
        .map(Into::into)
}

#[cfg(test)]
mod tests {
    #[test]
    fn internal_projection_helper_emits_structured_tracing() {
        let source = include_str!("mod.rs");
        for needle in [
            "agent_sessions::projection",
            "agent session projected (owned)",
            "events = projected.events.len()",
        ] {
            assert!(
                source.contains(needle),
                "internal agent session projection tracing must cover provider parity conversion: {needle}"
            );
        }
    }
}
