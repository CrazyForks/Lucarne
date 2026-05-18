use smol_str::SmolStr;
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Role {
    User,
    Assistant,
    Other(SmolStr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextBlock {
    pub(crate) text: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolUseBlock {
    pub(crate) id: Option<SmolStr>,
    pub(crate) name: SmolStr,
    pub(crate) input_json: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolResultBlock {
    pub(crate) tool_use_id: Option<SmolStr>,
    pub(crate) content: SmolStr,
    pub(crate) is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawBlock {
    pub(crate) kind: SmolStr,
    pub(crate) raw_json: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContentBlock {
    Text(TextBlock),
    ToolUse(ToolUseBlock),
    ToolResult(ToolResultBlock),
    Raw(RawBlock),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Version {
    V1,
}

#[derive(Debug)]
pub(crate) struct Body {
    pub(crate) session_id: Option<SmolStr>,
    pub(crate) entries: Box<[Entry]>,
}

#[derive(Debug)]
pub(crate) struct Entry {
    pub(crate) role: Role,
    pub(crate) timestamp: Option<SmolStr>,
    pub(crate) blocks: Box<[ContentBlock]>,
}
