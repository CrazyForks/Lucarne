use std::borrow::Cow;

use serde::Deserialize;
use smol_str::SmolStr;

#[derive(Default)]
pub(crate) struct ParsedToolInput {
    #[cfg(any(
        feature = "claude",
        feature = "codex",
        feature = "cursor",
        feature = "grok",
        feature = "pi"
    ))]
    pub(crate) command: Option<SmolStr>,
    pub(crate) file_path: Option<SmolStr>,
    pub(crate) lines_added: u64,
    pub(crate) lines_removed: u64,
}

#[derive(Deserialize, Default)]
struct ToolInputShape<'a> {
    #[cfg(any(
        feature = "claude",
        feature = "codex",
        feature = "cursor",
        feature = "grok",
        feature = "pi"
    ))]
    #[serde(borrow, default)]
    command: Option<Cow<'a, str>>,
    #[cfg(any(
        feature = "claude",
        feature = "codex",
        feature = "cursor",
        feature = "grok",
        feature = "pi"
    ))]
    #[serde(borrow, default)]
    cmd: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    file_path: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    path: Option<Cow<'a, str>>,
    #[serde(borrow, default, alias = "old_text")]
    old_string: Option<Cow<'a, str>>,
    #[serde(borrow, default, alias = "new_text")]
    new_string: Option<Cow<'a, str>>,
    #[serde(borrow, default, alias = "file_text")]
    content: Option<Cow<'a, str>>,
}

pub(crate) fn parsed_tool_input(tool_name: &str, input_json: Option<&str>) -> ParsedToolInput {
    let Some(input_json) = input_json else {
        return ParsedToolInput::default();
    };
    let Ok(parsed) = serde_json::from_str::<ToolInputShape<'_>>(input_json) else {
        return ParsedToolInput::default();
    };
    let tool = tool_name.to_ascii_lowercase();
    #[cfg(any(
        feature = "claude",
        feature = "codex",
        feature = "cursor",
        feature = "grok",
        feature = "pi"
    ))]
    let command = parsed.command.or(parsed.cmd).map(crate::util::cow_to_box);
    let file_path = parsed
        .file_path
        .or(parsed.path)
        .map(crate::util::cow_to_box);

    if tool.contains("edit") {
        let old_text = parsed.old_string.as_deref().unwrap_or("");
        let new_text = parsed.new_string.as_deref().unwrap_or("");
        return ParsedToolInput {
            #[cfg(any(
                feature = "claude",
                feature = "codex",
                feature = "cursor",
                feature = "grok",
                feature = "pi"
            ))]
            command,
            file_path,
            lines_added: count_lines(new_text),
            lines_removed: count_lines(old_text),
        };
    }

    if tool.contains("write") {
        let content = parsed.content.as_deref().unwrap_or("");
        return ParsedToolInput {
            #[cfg(any(
                feature = "claude",
                feature = "codex",
                feature = "cursor",
                feature = "grok",
                feature = "pi"
            ))]
            command,
            file_path,
            lines_added: count_lines(content),
            lines_removed: 0,
        };
    }

    ParsedToolInput {
        #[cfg(any(
            feature = "claude",
            feature = "codex",
            feature = "cursor",
            feature = "grok",
            feature = "pi"
        ))]
        command,
        file_path,
        lines_added: 0,
        lines_removed: 0,
    }
}

fn count_lines(text: &str) -> u64 {
    if text.is_empty() {
        0
    } else {
        text.lines().count() as u64
    }
}
