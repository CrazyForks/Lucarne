// fakeagent is a deterministic stand-in for real agent CLIs
// (claude / codex / copilot / gemini / pi) used in tests. It replays a
// scripted session:
//
//   * reads a fixture script path from $LUCARNE_FIXTURE
//   * streams OUT lines to stdout
//   * optionally waits for EXPECT_IN_CONTAINS substrings on stdin
//     before proceeding, so permission round-trips can be tested end
//     to end
//   * honors WAIT_MS milliseconds
//   * exits with EXIT code
//
// Script grammar (one directive per line; blank lines and # comments OK):
//
//   OUT <raw line>                  — print <raw line>\n to stdout
//   OUT_TMPL <template>             — expand ${RAND_ID} placeholders first
//   EXPECT_IN_CONTAINS <substring>  — block until stdin contains <substring>
//   EXPECT_IN_CONTAINS_NEXT <substring>
//                                 — block until unread stdin contains <substring>,
//                                   then advance the unread cursor past the match
//   EXPECT_SIGNAL_NEXT <signal>     — block until the process receives <signal>
//   WAIT_MS <n>                     — sleep n milliseconds
//   ENV_ECHO <key>                  — print value of env var <key>
//   EXIT <code>                     — exit with code
//   STDERR <raw>                    — print <raw> to stderr
//
// This is a faithful Rust port of lucarne/internal/fakeagent/main.go. Tests
// rely on it consuming tests/data/**/*.fixture unchanged.

mod signals;

use crate::signals::{install_signal_handlers, normalize_signal_name, signal_counter};
use std::{
    collections::BTreeMap,
    env, fmt,
    fs::File,
    io::{self, BufRead, BufReader, Read, Write},
    process::exit,
    sync::{atomic::Ordering, Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};

fn main() {
    install_signal_handlers();

    let mut args = env::args();
    let argv0 = args.next().unwrap_or_default();
    if args.next().as_deref() == Some("--version") {
        write_stdout_line(version_output(&argv0));
        return;
    }

    let fix = match env::var("LUCARNE_FIXTURE") {
        Ok(v) if !v.is_empty() => v,
        _ => exit_with_error(format_args!("fakeagent: LUCARNE_FIXTURE unset"), 2),
    };
    let file = match File::open(&fix) {
        Ok(f) => f,
        Err(e) => exit_with_error(format_args!("fakeagent: open: {e}"), 2),
    };

    // Rolling stdin buffer, fed by a background thread so EXPECT checks
    // can block until content arrives.
    let state = Arc::new((Mutex::new(StdinState::default()), Condvar::new()));
    {
        let state = Arc::clone(&state);
        thread::spawn(move || {
            let mut stdin = io::stdin();
            let mut chunk = [0u8; 4096];
            loop {
                match stdin.read(&mut chunk) {
                    Ok(0) => {
                        let (lock, cv) = &*state;
                        let mut s = lock.lock().unwrap();
                        s.closed = true;
                        cv.notify_all();
                        return;
                    }
                    Ok(n) => {
                        let (lock, cv) = &*state;
                        let mut s = lock.lock().unwrap();
                        s.buf.extend_from_slice(&chunk[..n]);
                        cv.notify_all();
                    }
                    Err(e) => {
                        let (lock, cv) = &*state;
                        let mut s = lock.lock().unwrap();
                        s.closed = true;
                        cv.notify_all();
                        write_stderr_line(format_args!("fakeagent: stdin: {e}"));
                        return;
                    }
                }
            }
        });
    }

    let wait_for = |sub: &str| {
        let (lock, cv) = &*state;
        let mut s = lock.lock().unwrap();
        loop {
            if contains_bytes(&s.buf, sub.as_bytes()) {
                return;
            }
            if s.closed {
                exit_with_error(
                    format_args!("fakeagent: stdin closed; never saw {sub:?}"),
                    3,
                );
            }
            s = cv.wait(s).unwrap();
        }
    };

    let wait_for_next = |sub: &str| {
        let (lock, cv) = &*state;
        let mut s = lock.lock().unwrap();
        loop {
            if let Some(idx) = find_bytes(&s.buf[s.consumed..], sub.as_bytes()) {
                s.consumed += idx + sub.len();
                return;
            }
            if s.closed {
                exit_with_error(
                    format_args!("fakeagent: stdin closed; never saw next {sub:?}"),
                    3,
                );
            }
            s = cv.wait(s).unwrap();
        }
    };

    let mut seen_signals = BTreeMap::<&'static str, usize>::new();
    let wait_for_signal_next = |raw: &str, seen: &mut BTreeMap<&'static str, usize>| {
        let Some(name) = normalize_signal_name(raw) else {
            exit_with_error(format_args!("fakeagent: unknown signal {raw:?}"), 2);
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let observed = signal_counter(name).load(Ordering::SeqCst);
            let consumed = seen.get(name).copied().unwrap_or(0);
            if observed > consumed {
                seen.insert(name, consumed + 1);
                return;
            }
            if Instant::now() >= deadline {
                exit_with_error(
                    format_args!("fakeagent: timed out waiting for signal {name:?}"),
                    3,
                );
            }
            thread::sleep(Duration::from_millis(10));
        }
    };

    let reader = BufReader::with_capacity(64 * 1024, file);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let stderr = io::stderr();

    let mut line_no = 0usize;
    for line in reader.lines() {
        line_no += 1;
        let raw = match line {
            Ok(l) => l,
            Err(e) => exit_with_error(format_args!("fakeagent: script: {e}"), 2),
        };
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let (directive, payload) = match raw.find(' ') {
            Some(i) => (
                &raw[..i],
                raw[i + 1..].trim_start_matches([' ', '\t']).to_string(),
            ),
            None => (raw.as_str(), String::new()),
        };

        match directive {
            "OUT" => {
                writeln!(out, "{}", payload)
                    .and_then(|_| out.flush())
                    .expect("stdout");
            }
            "OUT_TMPL" => {
                writeln!(out, "{}", expand(&payload))
                    .and_then(|_| out.flush())
                    .expect("stdout");
            }
            "EXPECT_IN_CONTAINS" => {
                let s = payload.trim().to_string();
                let unquoted = go_unquote(&s).unwrap_or(s);
                wait_for(&unquoted);
            }
            "EXPECT_IN_CONTAINS_NEXT" => {
                let s = payload.trim().to_string();
                let unquoted = go_unquote(&s).unwrap_or(s);
                wait_for_next(&unquoted);
            }
            "EXPECT_SIGNAL_NEXT" => {
                wait_for_signal_next(payload.trim(), &mut seen_signals);
            }
            "WAIT_MS" => {
                let n: u64 = match payload.trim().parse() {
                    Ok(v) => v,
                    Err(e) => fail(line_no, &raw, &e.to_string()),
                };
                thread::sleep(Duration::from_millis(n));
            }
            "ENV_ECHO" => {
                let v = env::var(payload.trim()).unwrap_or_default();
                writeln!(out, "{}", v)
                    .and_then(|_| out.flush())
                    .expect("stdout");
            }
            "EXIT" => {
                let code: i32 = match payload.trim().parse() {
                    Ok(v) => v,
                    Err(e) => fail(line_no, &raw, &e.to_string()),
                };
                exit(code);
            }
            "STDERR" => {
                let mut e = stderr.lock();
                writeln!(e, "{}", payload).ok();
            }
            other => fail(line_no, &raw, &format!("unknown directive \"{}\"", other)),
        }
    }
    exit(0);
}

fn write_stdout_line(line: &str) {
    let mut out = io::stdout().lock();
    writeln!(out, "{line}").expect("stdout");
}

fn write_stderr_line(args: fmt::Arguments<'_>) {
    let mut err = io::stderr().lock();
    let _ = writeln!(err, "{args}");
}

fn exit_with_error(args: fmt::Arguments<'_>, code: i32) -> ! {
    write_stderr_line(args);
    exit(code);
}

fn version_output(argv0: &str) -> &'static str {
    if argv0.contains("claude") {
        "claude 2.1.119"
    } else if argv0.contains("codex") {
        "codex 0.100.0"
    } else if argv0.contains("gemini") {
        "gemini 1.0.0"
    } else {
        "fakeagent 0.1.0"
    }
}

#[derive(Default)]
struct StdinState {
    buf: Vec<u8>,
    closed: bool,
    consumed: usize,
}

fn contains_bytes(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > hay.len() {
        return false;
    }
    hay.windows(needle.len()).any(|w| w == needle)
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() > hay.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

fn expand(s: &str) -> String {
    if !s.contains("${RAND_ID}") {
        return s.to_string();
    }
    let mut bytes = [0u8; 4];
    // Tiny PRNG seed from nanos — RAND_ID only needs collision-rare, not secure.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let mut st: u32 = nanos ^ 0x9E37_79B9;
    for b in bytes.iter_mut() {
        st = st.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *b = (st >> 24) as u8;
    }
    let hex = hex4(&bytes);
    s.replace("${RAND_ID}", &hex)
}

fn hex4(b: &[u8; 4]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(8);
    for byte in b {
        s.push(HEX[(byte >> 4) as usize] as char);
        s.push(HEX[(byte & 0x0f) as usize] as char);
    }
    s
}

fn fail(line: usize, raw: &str, err: &str) -> ! {
    exit_with_error(format_args!("fakeagent: line {line} ({raw}): {err}"), 2)
}

/// Minimal Go-style double-quoted string unquoter. Returns None if the
/// input is not a valid Go double-quoted string (so the caller falls
/// back to using the raw payload).
fn go_unquote(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'"' || bytes[bytes.len() - 1] != b'"' {
        return None;
    }
    let inner = &s[1..s.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let esc = chars.next()?;
        match esc {
            'a' => out.push('\x07'),
            'b' => out.push('\x08'),
            'f' => out.push('\x0c'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'v' => out.push('\x0b'),
            '\\' => out.push('\\'),
            '\'' => out.push('\''),
            '"' => out.push('"'),
            'x' => {
                let a = chars.next()?;
                let b = chars.next()?;
                let hi = a.to_digit(16)?;
                let lo = b.to_digit(16)?;
                out.push(((hi * 16 + lo) as u8) as char);
            }
            'u' => {
                let mut code: u32 = 0;
                for _ in 0..4 {
                    code = code * 16 + chars.next()?.to_digit(16)?;
                }
                out.push(char::from_u32(code)?);
            }
            _ => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{contains_bytes, find_bytes, version_output, StdinState};
    use crate::signals::normalize_signal_name;

    #[test]
    fn contains_bytes_matches_expected_substring() {
        assert!(contains_bytes(
            br#"{"method":"initialize"}"#,
            br#""method":"initialize""#
        ));
        assert!(!contains_bytes(
            br#"{"method":"initialize"}"#,
            br#""method":"turn/start""#
        ));
    }

    #[test]
    fn find_bytes_returns_relative_index() {
        assert_eq!(
            find_bytes(
                br#"abc "method":"initialize" def"#,
                br#""method":"initialize""#
            ),
            Some(4)
        );
        assert_eq!(
            find_bytes(
                br#"abc "method":"initialize" def"#,
                br#""method":"turn/start""#
            ),
            None
        );
    }

    #[test]
    fn consumed_cursor_allows_repeated_next_matches() {
        let mut state = StdinState {
            buf: br#"{"type":"user"}{"type":"user"}"#.to_vec(),
            closed: false,
            consumed: 0,
        };

        let first = find_bytes(&state.buf[state.consumed..], br#""type":"user""#).unwrap();
        state.consumed += first + br#""type":"user""#.len();

        let second = find_bytes(&state.buf[state.consumed..], br#""type":"user""#).unwrap();
        state.consumed += second + br#""type":"user""#.len();

        assert_eq!(state.consumed, state.buf.len() - 1);
    }

    #[cfg(unix)]
    #[test]
    fn normalize_signal_name_accepts_unix_short_and_long_forms() {
        assert_eq!(normalize_signal_name("SIGINT"), Some("SIGINT"));
        assert_eq!(normalize_signal_name("INT"), Some("SIGINT"));
        assert_eq!(normalize_signal_name("SIGTERM"), Some("SIGTERM"));
        assert_eq!(normalize_signal_name("HUP"), Some("SIGHUP"));
        assert_eq!(normalize_signal_name("SIGKILL"), None);
    }

    #[cfg(windows)]
    #[test]
    fn normalize_signal_name_maps_windows_term_to_console_break_counter() {
        assert_eq!(normalize_signal_name("SIGINT"), Some("SIGINT"));
        assert_eq!(normalize_signal_name("INT"), Some("SIGINT"));
        assert_eq!(normalize_signal_name("SIGTERM"), Some("SIGINT"));
        assert_eq!(normalize_signal_name("TERM"), Some("SIGINT"));
        assert_eq!(normalize_signal_name("HUP"), None);
    }

    #[test]
    fn version_output_uses_provider_name_from_argv0() {
        assert_eq!(version_output("/tmp/claude-fakeagent"), "claude 2.1.119");
        assert_eq!(version_output("/tmp/codex-fakeagent"), "codex 0.100.0");
        assert_eq!(version_output("/tmp/gemini-fakeagent"), "gemini 1.0.0");
    }
}
