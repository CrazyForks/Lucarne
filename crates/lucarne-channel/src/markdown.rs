//! Markdown → channel-specific format conversion.
//!
//! The bot authors content in a subset of CommonMark. Telegram's
//! MarkdownV2 flavour is *almost* but not quite CommonMark:
//!
//! * Every occurrence of the metacharacters
//!   `_ * [ ] ( ) ~ \` > # + - = | { } . !` must be escaped with
//!   backslash **unless** it is part of a recognised formatting entity.
//! * Inline code and pre-formatted code blocks have their own escaping
//!   rules (only `\` and `` ` `` need escaping inside them).
//! * Bulleted lists (`- item`) are not a native concept — the hyphens
//!   must be escaped. We normalise them to a middle dot bullet.
//!
//! The [`MarkdownDialect`] trait leaves room for other channels
//! (Slack `mrkdwn`, Matrix HTML, …) to plug in later without touching
//! call sites.

/// A target dialect. Implementations are stateless and cheap; the
/// public API is [`render`].
pub trait MarkdownDialect {
    /// Convert a generic markdown string to the dialect's representation.
    fn render(&self, input: &str) -> String;
}

/// Convert to Telegram MarkdownV2.
#[derive(Debug, Clone, Copy, Default)]
pub struct TelegramMarkdownV2;

impl MarkdownDialect for TelegramMarkdownV2 {
    fn render(&self, input: &str) -> String {
        render_telegram_markdown_v2(input)
    }
}

/// Convenience: plain-text safe escape for Telegram MarkdownV2 (no
/// markdown syntax considered — every special char is backslash-escaped).
pub fn escape_v2(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for ch in text.chars() {
        if is_v2_special(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn is_v2_special(ch: char) -> bool {
    matches!(
        ch,
        '_' | '*'
            | '['
            | ']'
            | '('
            | ')'
            | '~'
            | '`'
            | '>'
            | '#'
            | '+'
            | '-'
            | '='
            | '|'
            | '{'
            | '}'
            | '.'
            | '!'
            | '\\'
    )
}

fn is_code_special(ch: char) -> bool {
    matches!(ch, '`' | '\\')
}

fn escape_code(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for ch in s.chars() {
        if is_code_special(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Render a subset of CommonMark to Telegram MarkdownV2.
///
/// Supported features:
/// * Fenced code blocks ```` ```lang ... ``` ````.
/// * Inline code `` `x` ``.
/// * Bold `**x**` / `__x__`.
/// * Italic `*x*` / `_x_` (single char delimiter; conservative).
/// * Strikethrough `~~x~~`.
/// * Links `[text](url)`.
/// * Unordered list items rendered as `• `.
/// * Block-quote lines `> …` rendered as `> …`.
/// * Headings `#…` rendered as bold lines.
///
/// Anything not recognised is emitted as escaped plain text. The
/// function never panics; on malformed markup it falls back to
/// escaping.
pub fn render_telegram_markdown_v2(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 16);

    let mut in_code_block: Option<String> = None; // language or empty
    let mut code_buf = String::new();

    for line_with_nl in input.split_inclusive('\n') {
        let has_nl = line_with_nl.ends_with('\n');
        let line = line_with_nl.trim_end_matches('\n');

        if let Some(lang) = &in_code_block {
            if line.trim_start().starts_with("```") {
                // close fence — emit ```lang\n body \n```
                out.push_str("```");
                if !lang.is_empty() {
                    out.push_str(&escape_code(lang));
                }
                out.push('\n');
                out.push_str(&escape_code(&code_buf));
                if !code_buf.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("```");
                if has_nl {
                    out.push('\n');
                }
                in_code_block = None;
                code_buf.clear();
            } else {
                code_buf.push_str(line);
                code_buf.push('\n');
            }
            continue;
        }

        if let Some(rest) = line.trim_start().strip_prefix("```") {
            let lang = rest.trim().to_string();
            in_code_block = Some(lang);
            // Emission happens on close; nothing to write yet.
            continue;
        }

        // Heading: #… → bold line
        if let Some(rest) = strip_heading_marker(line) {
            out.push('*');
            out.push_str(&render_inline(rest));
            out.push('*');
            if has_nl {
                out.push('\n');
            }
            continue;
        }

        // Unordered list item: "- ", "* ", "+ "
        if let Some(rest) = strip_list_marker(line) {
            out.push_str("• ");
            out.push_str(&render_inline(rest));
            if has_nl {
                out.push('\n');
            }
            continue;
        }

        // Block quote: "> …"
        if let Some(rest) = line.strip_prefix("> ") {
            out.push_str("> ");
            out.push_str(&render_inline(rest));
            if has_nl {
                out.push('\n');
            }
            continue;
        }

        out.push_str(&render_inline(line));
        if has_nl {
            out.push('\n');
        }
    }

    // Unterminated code fence: flush as code block.
    if let Some(lang) = in_code_block {
        out.push_str("```");
        if !lang.is_empty() {
            out.push_str(&escape_code(&lang));
        }
        out.push('\n');
        out.push_str(&escape_code(&code_buf));
        if !code_buf.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```");
    }

    out
}

fn strip_heading_marker(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let hashes: usize = trimmed.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.starts_with(' ') {
        return None;
    }
    Some(rest.trim_start())
}

fn strip_list_marker(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return Some(rest);
        }
    }
    None
}

/// Render inline formatting for one logical line. Handles code spans,
/// bold, italic, strikethrough, and links.
fn render_inline(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut out = String::with_capacity(line.len() + 4);

    while i < bytes.len() {
        let b = bytes[i];
        // Inline code: `…`
        if b == b'`' {
            if let Some(end) = find_next(bytes, i + 1, b'`') {
                let code = &line[i + 1..end];
                out.push('`');
                out.push_str(&escape_code(code));
                out.push('`');
                i = end + 1;
                continue;
            }
        }
        // Bold: ** … ** or __ … __
        if i + 1 < bytes.len()
            && ((b == b'*' && bytes[i + 1] == b'*') || (b == b'_' && bytes[i + 1] == b'_'))
        {
            let marker = &line[i..i + 2];
            if let Some(end) = find_sequence(bytes, i + 2, marker.as_bytes()) {
                let inner = &line[i + 2..end];
                out.push('*');
                out.push_str(&render_inline(inner));
                out.push('*');
                i = end + 2;
                continue;
            }
        }
        // Strikethrough: ~~ … ~~
        if b == b'~' && i + 1 < bytes.len() && bytes[i + 1] == b'~' {
            if let Some(end) = find_sequence(bytes, i + 2, b"~~") {
                let inner = &line[i + 2..end];
                out.push('~');
                out.push_str(&render_inline(inner));
                out.push('~');
                i = end + 2;
                continue;
            }
        }
        // Italic: *x* or _x_ (single, non-bold already handled above)
        if (b == b'*' || b == b'_')
            && i + 1 < bytes.len()
            && bytes[i + 1] != b
            && !bytes[i + 1].is_ascii_whitespace()
            && (b != b'_' || is_underscore_emphasis_boundary(bytes, i))
        {
            if let Some(end) = find_next(bytes, i + 1, b) {
                // avoid ** matching
                if (end + 1 >= bytes.len() || bytes[end + 1] != b)
                    && (b != b'_' || is_underscore_emphasis_boundary(bytes, end))
                {
                    let inner = &line[i + 1..end];
                    out.push('_');
                    out.push_str(&render_inline(inner));
                    out.push('_');
                    i = end + 1;
                    continue;
                }
            }
        }
        // Link: [text](url)
        if b == b'[' {
            if let Some(text_end) = find_next(bytes, i + 1, b']') {
                if text_end + 1 < bytes.len() && bytes[text_end + 1] == b'(' {
                    if let Some(url_end) = find_next(bytes, text_end + 2, b')') {
                        let text = &line[i + 1..text_end];
                        let url = &line[text_end + 2..url_end];
                        out.push('[');
                        out.push_str(&render_inline(text));
                        out.push(']');
                        out.push('(');
                        out.push_str(&escape_link_url(url));
                        out.push(')');
                        i = url_end + 1;
                        continue;
                    }
                }
            }
        }

        // Default: escape the char.
        let ch = line[i..].chars().next().unwrap();
        if is_v2_special(ch) {
            out.push('\\');
        }
        out.push(ch);
        i += ch.len_utf8();
    }

    out
}

fn is_underscore_emphasis_boundary(bytes: &[u8], pos: usize) -> bool {
    let before = pos
        .checked_sub(1)
        .and_then(|idx| bytes.get(idx))
        .copied()
        .is_some_and(is_identifier_byte);
    let after = bytes.get(pos + 1).copied().is_some_and(is_identifier_byte);
    !(before || after)
}

fn is_identifier_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn find_next(bytes: &[u8], from: usize, target: u8) -> Option<usize> {
    bytes[from..]
        .iter()
        .position(|&b| b == target)
        .map(|p| p + from)
}

fn find_sequence(bytes: &[u8], from: usize, target: &[u8]) -> Option<usize> {
    if target.is_empty() || from >= bytes.len() {
        return None;
    }
    let end = bytes.len().saturating_sub(target.len());
    let mut i = from;
    while i <= end {
        if &bytes[i..i + target.len()] == target {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Inside `(url)` in MarkdownV2 only `)` and `\\` must be escaped.
fn escape_link_url(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for ch in url.chars() {
        if ch == ')' || ch == '\\' {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_v2_escapes_all_specials() {
        let s = "a.b!c_d*e[f](g){h}";
        let out = escape_v2(s);
        assert_eq!(out, r"a\.b\!c\_d\*e\[f\]\(g\)\{h\}");
    }

    #[test]
    fn plain_paragraph_escapes_dots() {
        assert_eq!(
            render_telegram_markdown_v2("Hello world."),
            r"Hello world\."
        );
    }

    #[test]
    fn bold_and_italic() {
        assert_eq!(
            render_telegram_markdown_v2("**bold** and *it*"),
            r"*bold* and _it_"
        );
    }

    #[test]
    fn identifier_underscores_are_not_italic() {
        assert_eq!(
            render_telegram_markdown_v2("UI_QUEUE_ONE and foo_bar_baz"),
            r"UI\_QUEUE\_ONE and foo\_bar\_baz"
        );
    }

    #[test]
    fn inline_code_preserves_dots() {
        assert_eq!(render_telegram_markdown_v2("run `a.b`."), "run `a.b`\\.");
    }

    #[test]
    fn fenced_code_block_round_trips() {
        let md = "```rust\nlet x = 1;\n```\n";
        let out = render_telegram_markdown_v2(md);
        assert!(out.starts_with("```rust\n"));
        assert!(out.contains("let x = 1;"));
        assert!(out.trim_end().ends_with("```"));
    }

    #[test]
    fn list_rendered_as_bullets() {
        let md = "- one\n- two";
        let out = render_telegram_markdown_v2(md);
        assert!(out.contains("• one"));
        assert!(out.contains("• two"));
        assert!(!out.contains("\\- "));
    }

    #[test]
    fn heading_rendered_as_bold() {
        assert_eq!(render_telegram_markdown_v2("# Title"), "*Title*");
    }

    #[test]
    fn link_rendered() {
        let md = "see [docs](https://example.com/a.b)";
        let out = render_telegram_markdown_v2(md);
        assert!(out.contains("[docs](https://example.com/a.b)"));
    }

    #[test]
    fn unterminated_code_fence_is_salvaged() {
        let md = "```\nhello";
        let out = render_telegram_markdown_v2(md);
        assert!(out.starts_with("```\n"));
        assert!(out.contains("hello"));
        assert!(out.trim_end().ends_with("```"));
    }

    #[test]
    fn backslash_in_code_is_escaped() {
        let md = "`a\\b`";
        let out = render_telegram_markdown_v2(md);
        assert_eq!(out, "`a\\\\b`");
    }
}
