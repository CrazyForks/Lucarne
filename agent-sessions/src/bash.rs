use std::path::Path;

/// Replace bytes inside quoted strings with spaces, preserving byte
/// positions so that split-point indices still map to the original
/// command string.
///
/// Uses an in-place `Vec<u8>` instead of collecting chars into a
/// `Vec<char>` (which allocates 4× for ASCII input).
fn strip_quoted_strings(command: &str) -> String {
    let mut buf = Vec::from(command.as_bytes());
    let len = buf.len();
    let mut index = 0;

    while index < len {
        let ch = buf[index];
        if ch == b'"' || ch == b'\'' {
            let quote = ch;
            buf[index] = b' ';
            index += 1;
            while index < len && buf[index] != quote {
                buf[index] = b' ';
                index += 1;
            }
            if index < len {
                buf[index] = b' ';
                index += 1;
            }
            continue;
        }
        index += 1;
    }

    // SAFETY: we only replaced ASCII printable bytes (quotes, letters,
    // digits, etc.) with ASCII spaces.  The original was valid UTF-8,
    // and replacing any byte in a single-byte UTF-8 codepoint (0x00‥0x7F)
    // with another single-byte codepoint preserves validity.
    // Multi-byte codepoints (0x80‥0xFF) are never quote characters, so
    // they are left untouched.
    unsafe { String::from_utf8_unchecked(buf) }
}

pub fn extract_bash_commands(command: &str) -> Vec<String> {
    if command.trim().is_empty() {
        return Vec::new();
    }

    let stripped = strip_quoted_strings(command);
    let bytes = stripped.as_bytes();
    let mut segments = Vec::new();
    let mut start = 0;
    let mut index = 0;

    while index < bytes.len() {
        let separator_len = match bytes[index] {
            b';' | b'|' => 1,
            b'&' if bytes.get(index + 1) == Some(&b'&') => 2,
            _ => {
                index += 1;
                continue;
            }
        };
        segments.push((start, index));
        index += separator_len;
        start = index;
    }
    segments.push((start, command.len()));

    let mut commands = Vec::new();
    for (start, end) in segments {
        let segment = command[start..end].trim();
        if segment.is_empty() {
            continue;
        }
        let first_token = segment.split_whitespace().next().unwrap_or("");
        let base = Path::new(first_token)
            .file_name()
            .map(|file| file.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !base.is_empty() && base != "cd" {
            commands.push(base);
        }
    }

    commands
}

#[cfg(test)]
mod tests {
    use super::extract_bash_commands;

    #[test]
    fn extracts_chained_commands() {
        assert_eq!(
            extract_bash_commands("git status && cargo test"),
            vec!["git", "cargo"]
        );
    }

    #[test]
    fn ignores_quoted_separators() {
        assert_eq!(
            extract_bash_commands(r#"echo "hello && world" | tee out.log"#),
            vec!["echo", "tee"]
        );
    }

    #[test]
    fn filters_out_cd() {
        assert_eq!(extract_bash_commands("cd /tmp && git status"), vec!["git"]);
    }
}
