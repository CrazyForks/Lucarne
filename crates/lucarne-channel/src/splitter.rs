//! Long-message splitter.
//!
//! Telegram's hard limit per text message is 4096 UTF-16 code units.
//! We stay below that with a safety margin and prefer to cut at
//! paragraph → line → word boundaries. Fenced code blocks are kept
//! intact across the split — if a chunk ends in the middle of one we
//! close the fence and re-open it in the next chunk so each piece
//! stays renderable on its own.

/// Default safe limit used by [`split_for_channel`]. Channels with
/// tighter limits can pass their own value.
pub const DEFAULT_LIMIT: usize = 4000;

/// Split `text` into chunks each no larger than `limit` UTF-16 code
/// units. The splitter respects markdown code fences so neither half
/// renders as broken markup.
pub fn split_for_channel(text: &str, limit: usize) -> Vec<String> {
    let input_len = utf16_len(text);
    if limit == 0 || input_len <= limit {
        tracing::trace!(
            target: "lucarne_channel::splitter",
            input_len, limit, "no split needed"
        );
        return vec![text.to_string()];
    }

    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;
    let mut open_fence: Option<String> = None;

    for line in text.split_inclusive('\n') {
        let line_len = utf16_len(line);

        // Line itself exceeds the limit → hard-wrap it.
        if line_len > limit {
            flush(&mut chunks, &mut current, &mut current_len, &mut open_fence);
            for piece in hard_wrap(line, limit) {
                chunks.push(piece);
            }
            continue;
        }

        if current_len + line_len > limit {
            flush(&mut chunks, &mut current, &mut current_len, &mut open_fence);
            // Re-open fence if we split mid code-block.
            if let Some(lang) = &open_fence {
                let opener = if lang.is_empty() {
                    "```\n".to_string()
                } else {
                    format!("```{}\n", lang)
                };
                current.push_str(&opener);
                current_len += utf16_len(&opener);
            }
        }

        // Track fence state *before* appending so we know what to
        // re-open if this line causes a flush later.
        if let Some(lang) = detect_code_fence(line) {
            open_fence = match open_fence {
                Some(_) => None, // closing
                None => Some(lang),
            };
        }

        current.push_str(line);
        current_len += line_len;
    }

    flush(&mut chunks, &mut current, &mut current_len, &mut open_fence);
    let out: Vec<String> = chunks.into_iter().filter(|c| !c.is_empty()).collect();
    tracing::debug!(
        target: "lucarne_channel::splitter",
        input_len, limit, chunks = out.len(),
        "split message into chunks",
    );
    out
}

fn flush(
    chunks: &mut Vec<String>,
    current: &mut String,
    current_len: &mut usize,
    open_fence: &mut Option<String>,
) {
    if current.is_empty() {
        return;
    }
    // If we're flushing mid-fence, close it so the chunk is valid.
    if open_fence.is_some() {
        if !current.ends_with('\n') {
            current.push('\n');
        }
        current.push_str("```");
    }
    chunks.push(std::mem::take(current));
    *current_len = 0;
}

fn detect_code_fence(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("```")?;
    Some(rest.trim().trim_end_matches('\n').to_string())
}

/// Hard-wrap an over-long line at word boundaries, producing pieces
/// each ≤ `limit` UTF-16 units.
fn hard_wrap(line: &str, limit: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut buf_len = 0usize;

    let flush = |buf: &mut String, out: &mut Vec<String>, buf_len: &mut usize| {
        if !buf.is_empty() {
            out.push(std::mem::take(buf));
            *buf_len = 0;
        }
    };

    for word in split_whitespace_inclusive(line) {
        let w_len = utf16_len(word);
        if w_len > limit {
            flush(&mut buf, &mut out, &mut buf_len);
            // Break the word itself by UTF-16 code units.
            let mut tmp = String::new();
            let mut tmp_len = 0usize;
            for ch in word.chars() {
                let cl = ch.len_utf16();
                if tmp_len + cl > limit {
                    out.push(std::mem::take(&mut tmp));
                    tmp_len = 0;
                }
                tmp.push(ch);
                tmp_len += cl;
            }
            if !tmp.is_empty() {
                buf = tmp;
                buf_len = tmp_len;
            }
            continue;
        }
        if buf_len + w_len > limit {
            flush(&mut buf, &mut out, &mut buf_len);
        }
        buf.push_str(word);
        buf_len += w_len;
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

/// Like `str::split_whitespace` but yields each whitespace run too so
/// joining the output reproduces the input.
fn split_whitespace_inclusive(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_ws = s
        .as_bytes()
        .first()
        .is_some_and(|b| b.is_ascii_whitespace());
    for (i, c) in s.char_indices() {
        let is_ws = c.is_whitespace();
        if is_ws != in_ws && i > start {
            out.push(&s[start..i]);
            start = i;
            in_ws = is_ws;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

fn utf16_len(s: &str) -> usize {
    s.chars().map(|c| c.len_utf16()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_single_chunk() {
        let parts = split_for_channel("hello", 100);
        assert_eq!(parts, vec!["hello".to_string()]);
    }

    #[test]
    fn splits_at_paragraph_boundary() {
        let text = "a\n".repeat(50); // 100 chars
        let parts = split_for_channel(&text, 40);
        assert!(parts.len() >= 2);
        for p in &parts {
            assert!(utf16_len(p) <= 40, "{} > 40", p.len());
        }
        assert_eq!(parts.concat().chars().filter(|c| *c == 'a').count(), 50);
    }

    #[test]
    fn preserves_code_fence_across_split() {
        let mut text = String::from("```rust\n");
        for i in 0..40 {
            text.push_str(&format!("let x{} = 1;\n", i));
        }
        text.push_str("```\n");
        let parts = split_for_channel(&text, 100);
        assert!(parts.len() >= 2);
        for p in &parts {
            let opens = p.matches("```").count();
            assert_eq!(opens % 2, 0, "unbalanced fences in chunk: {}", p);
        }
        // Continuation chunk should reopen with language tag.
        assert!(parts[1].starts_with("```rust\n"), "got: {:?}", parts[1]);
    }

    #[test]
    fn hard_wraps_oversize_line() {
        let text = "abcdefghij".repeat(200); // 2000 chars, no newlines
        let parts = split_for_channel(&text, 100);
        assert!(parts.iter().all(|p| utf16_len(p) <= 100));
        assert_eq!(parts.concat().len(), 2000);
    }

    #[test]
    fn utf16_aware_for_emoji() {
        // Each 😀 is 2 UTF-16 units. Limit 4 ⇒ 2 emojis per chunk.
        let text = "😀😀😀😀😀";
        let parts = split_for_channel(text, 4);
        assert!(parts.iter().all(|p| utf16_len(p) <= 4));
        assert_eq!(parts.concat(), text);
    }
}
