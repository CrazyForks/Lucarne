//! Helpers to decide whether an inbound attachment should be treated as
//! plain-text input for the agent and, if so, to extract that text.
//!
//! The bot inlines user-uploaded text files into the prompt. This keeps
//! the agent flow uniform regardless of whether the user pasted code or
//! dropped a `.py` / `.md` file into the chat.

use super::types::{ChannelError, IncomingAttachment, Result};
use tracing::{debug, warn};

/// Upper bound on bytes accepted from an uploaded "text" file. Larger
/// payloads are rejected so we don't blow past the channel's per-message
/// character limit after inlining.
pub const DEFAULT_MAX_TEXT_BYTES: u64 = 256 * 1024;

/// Textual MIME-type prefixes we accept directly.
const TEXT_MIME_PREFIXES: &[&str] = &["text/"];

/// Extra MIME types that are technically "application/..." but still
/// pure text from a user's perspective.
const TEXTISH_MIMES: &[&str] = &[
    "application/json",
    "application/xml",
    "application/x-yaml",
    "application/yaml",
    "application/toml",
    "application/x-sh",
    "application/javascript",
    "application/x-shellscript",
];

/// Extension whitelist for files whose MIME type is missing or
/// unreliable (mobile clients often report `application/octet-stream`).
const TEXTISH_EXTENSIONS: &[&str] = &[
    "txt",
    "md",
    "markdown",
    "rst",
    "log",
    "csv",
    "tsv",
    "rs",
    "py",
    "js",
    "ts",
    "tsx",
    "jsx",
    "go",
    "c",
    "h",
    "cpp",
    "hpp",
    "java",
    "kt",
    "swift",
    "rb",
    "php",
    "scala",
    "cs",
    "sh",
    "bash",
    "zsh",
    "fish",
    "yaml",
    "yml",
    "toml",
    "json",
    "xml",
    "html",
    "htm",
    "css",
    "scss",
    "less",
    "sql",
    "ini",
    "conf",
    "cfg",
    "env",
    "lock",
    "proto",
    "gradle",
    "mk",
    "cmake",
    "dockerfile",
];

/// Classify an attachment. Returns `true` if [`read_text`] should be
/// called.
#[must_use]
pub fn looks_textual(att: &IncomingAttachment) -> bool {
    if let Some(mime) = &att.mime_type {
        let mime = mime.to_ascii_lowercase();
        if TEXT_MIME_PREFIXES.iter().any(|p| mime.starts_with(p))
            || TEXTISH_MIMES.iter().any(|m| mime == *m)
        {
            return true;
        }
    }
    if let Some(name) = &att.filename {
        let lower = name.to_ascii_lowercase();
        if let Some(ext) = lower.rsplit('.').next() {
            if TEXTISH_EXTENSIONS.contains(&ext) {
                return true;
            }
        }
        // Dockerfile, Makefile, etc. with no extension.
        let base = lower.rsplit('/').next().unwrap_or(&lower);
        if matches!(base, "dockerfile" | "makefile" | "readme") {
            return true;
        }
    }
    false
}

/// Decode `bytes` coming from an attachment into UTF-8 text. Rejects
/// oversize payloads and payloads that are not valid UTF-8 (we don't
/// try to auto-detect encodings — the agent expects UTF-8 anyway).
pub fn read_text(att: &IncomingAttachment, bytes: Vec<u8>, max_bytes: u64) -> Result<String> {
    let filename = att.filename.as_deref().unwrap_or("<unnamed>");
    let mime_type = att.mime_type.as_deref().unwrap_or("<unknown>");
    if let Some(size) = att.size {
        if size > max_bytes {
            warn!(
                target: "lucarne_channel::ingest",
                filename,
                mime_type,
                size,
                limit = max_bytes,
                "attachment rejected as too large"
            );
            return Err(ChannelError::AttachmentTooLarge {
                size,
                limit: max_bytes,
            });
        }
    }
    let byte_len = bytes.len() as u64;
    if byte_len > max_bytes {
        warn!(
            target: "lucarne_channel::ingest",
            filename,
            mime_type,
            size = byte_len,
            limit = max_bytes,
            "attachment rejected as too large"
        );
        return Err(ChannelError::AttachmentTooLarge {
            size: byte_len,
            limit: max_bytes,
        });
    }
    match String::from_utf8(bytes) {
        Ok(text) => {
            debug!(
                target: "lucarne_channel::ingest",
                filename,
                mime_type,
                bytes = byte_len,
                "attachment text decoded"
            );
            Ok(text)
        }
        Err(_) => {
            warn!(
                target: "lucarne_channel::ingest",
                filename,
                mime_type,
                bytes = byte_len,
                "attachment rejected as non-text"
            );
            Err(ChannelError::NotTextual {
                reason: "not valid UTF-8".into(),
            })
        }
    }
}

/// Compose a prompt snippet for the agent that inlines file content
/// with a short header so the model knows the provenance.
#[must_use]
pub fn format_for_agent(att: &IncomingAttachment, content: &str) -> String {
    let name = att.filename.as_deref().unwrap_or("attachment.txt");
    format!(
        "[user attached file: {name}]\n```\n{body}\n```",
        name = name,
        body = content.trim_end()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(name: &str, mime: Option<&str>) -> IncomingAttachment {
        IncomingAttachment {
            file_ref: "x".into(),
            filename: Some(name.into()),
            mime_type: mime.map(|s| s.into()),
            size: Some(10),
        }
    }

    #[test]
    fn text_mime_detected() {
        assert!(looks_textual(&att("a.bin", Some("text/plain"))));
    }

    #[test]
    fn extension_fallback() {
        assert!(looks_textual(&att(
            "main.rs",
            Some("application/octet-stream")
        )));
        assert!(looks_textual(&att("Dockerfile", None)));
    }

    #[test]
    fn binary_rejected() {
        assert!(!looks_textual(&att("img.png", Some("image/png"))));
    }

    #[test]
    fn oversize_rejected() {
        let a = att("big.txt", Some("text/plain"));
        let err = read_text(&a, vec![0u8; 10], 5).unwrap_err();
        assert!(matches!(err, ChannelError::AttachmentTooLarge { .. }));
    }

    #[test]
    fn non_utf8_rejected() {
        let a = IncomingAttachment {
            file_ref: "x".into(),
            filename: Some("a.txt".into()),
            mime_type: Some("text/plain".into()),
            size: None,
        };
        let err = read_text(&a, vec![0xff, 0xfe, 0xfd], 1024).unwrap_err();
        assert!(matches!(err, ChannelError::NotTextual { .. }));
    }

    #[test]
    fn format_for_agent_inlines() {
        let a = att("notes.md", Some("text/markdown"));
        let out = format_for_agent(&a, "hello\n");
        assert!(out.contains("notes.md"));
        assert!(out.contains("hello"));
    }

    #[test]
    fn ingest_emits_structured_tracing() {
        let source = include_str!("ingest.rs")
            .split("\n#[cfg(test)]")
            .next()
            .expect("production source");
        for needle in [
            "lucarne_channel::ingest",
            "attachment text decoded",
            "attachment rejected as too large",
            "attachment rejected as non-text",
        ] {
            assert!(
                source.contains(needle),
                "ingest tracing must cover attachment decode boundary: {needle}"
            );
        }
    }
}
