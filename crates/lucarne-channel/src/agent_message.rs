use std::time::Duration;

const FOOTER_SEPARATOR: &str = "==========";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentMessageFooter {
    pub cost: Option<String>,
    pub session: Option<String>,
    pub cwd: Option<String>,
}

pub fn render_agent_message_markdown(text: &str, footer: &AgentMessageFooter) -> String {
    let (text, extracted_cost) = split_trailing_cost(text);
    let mut footer_lines = Vec::new();
    if let Some(cost) = footer.cost.as_deref().or(extracted_cost) {
        footer_lines.push(format!("cost: {cost}"));
    }
    if let Some(session) = footer
        .session
        .as_deref()
        .map(str::trim)
        .filter(|session| !session.is_empty())
    {
        footer_lines.push(format!("session: `{}`", markdown_inline_code_text(session)));
    }
    if let Some(cwd) = footer
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty())
    {
        footer_lines.push(format!("cwd: `{}`", markdown_inline_code_text(cwd)));
    }

    let text = text.trim();
    if footer_lines.is_empty() {
        return text.to_string();
    }
    if text.is_empty() {
        return footer_lines.join("\n");
    }
    format!("{text}\n\n{FOOTER_SEPARATOR}\n{}", footer_lines.join("\n"))
}

pub fn format_cost_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    match (hours, minutes, seconds) {
        (0, 0, seconds) => format!("{seconds}s"),
        (0, minutes, seconds) => format!("{minutes}m {seconds}s"),
        (hours, minutes, seconds) => format!("{hours}h {minutes}m {seconds}s"),
    }
}

fn split_trailing_cost(text: &str) -> (&str, Option<&str>) {
    let trimmed = text.trim_end();
    let Some((before, line)) = trimmed.rsplit_once('\n') else {
        return (text.trim(), None);
    };
    if !before.ends_with('\n') {
        return (text.trim(), None);
    }
    let line = line.trim();
    let cost = line
        .strip_prefix("cost:")
        .or_else(|| line.strip_prefix("耗时:"))
        .map(str::trim)
        .filter(|cost| !cost.is_empty());
    match cost {
        Some(cost) => (before.trim_end(), Some(cost)),
        None => (text.trim(), None),
    }
}

fn markdown_inline_code_text(value: &str) -> String {
    value.replace('`', "'")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn footer_keeps_cost_session_and_cwd_in_one_block() {
        let body = render_agent_message_markdown(
            "done\n\ncost: 2m 5s",
            &AgentMessageFooter {
                cost: None,
                session: Some("thread-1".into()),
                cwd: Some("/tmp/workspace-a".into()),
            },
        );

        assert_eq!(
            body,
            "done\n\n==========\ncost: 2m 5s\nsession: `thread-1`\ncwd: `/tmp/workspace-a`"
        );
    }

    #[test]
    fn footer_normalizes_legacy_chinese_cost_label() {
        let body = render_agent_message_markdown(
            "done\n\n耗时: 41s",
            &AgentMessageFooter {
                cost: None,
                session: Some("thread-1".into()),
                cwd: None,
            },
        );

        assert_eq!(body, "done\n\n==========\ncost: 41s\nsession: `thread-1`");
    }

    #[test]
    fn format_cost_duration_uses_compact_units() {
        assert_eq!(format_cost_duration(Duration::from_secs(41)), "41s");
        assert_eq!(format_cost_duration(Duration::from_secs(125)), "2m 5s");
        assert_eq!(format_cost_duration(Duration::from_secs(3_725)), "1h 2m 5s");
    }
}
