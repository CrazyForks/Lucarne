use std::io::Cursor;

use agent_sessions::reader::{ReverseLines, SessionReader};

#[test]
fn reverse_lines_reads_nonempty_lines_from_tail() {
    let bytes = b"\nfirst\nsecond\n\nthird without newline";
    let mut reader = ReverseLines::new(Cursor::new(bytes)).unwrap();

    let mut lines = Vec::new();
    while let Some(line) = reader.next_line().unwrap() {
        lines.push(String::from_utf8(line).unwrap());
    }

    assert_eq!(lines, vec!["third without newline", "second", "first"]);
}

#[test]
fn session_reader_opens_file_and_reads_reverse_lines() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("session.jsonl");
    std::fs::write(&path, b"one\ntwo\nthree\n").unwrap();
    let mut lines = SessionReader::open(&path).unwrap().reverse_lines().unwrap();

    assert_eq!(lines.next_line().unwrap().as_deref(), Some(&b"three"[..]));
    assert_eq!(lines.next_line().unwrap().as_deref(), Some(&b"two"[..]));
    assert_eq!(lines.next_line().unwrap().as_deref(), Some(&b"one"[..]));
    assert_eq!(lines.next_line().unwrap(), None);
}

#[test]
fn session_reader_reads_reverse_lines_before_byte_offset() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("session.jsonl");
    std::fs::write(&path, b"one\ntwo\nthree\n").unwrap();
    let before_three = b"one\ntwo\n".len() as u64;
    let mut lines = SessionReader::open(&path)
        .unwrap()
        .reverse_lines_before(before_three)
        .unwrap();

    assert_eq!(lines.next_line().unwrap().as_deref(), Some(&b"two"[..]));
    assert_eq!(lines.next_line().unwrap().as_deref(), Some(&b"one"[..]));
    assert_eq!(lines.next_line().unwrap(), None);
}

#[test]
fn reverse_lines_reports_start_offsets_for_byte_cursors() {
    let bytes = b"one\ntwo\nthree\n";
    let mut lines = ReverseLines::new(Cursor::new(bytes)).unwrap();

    let three = lines.next_line_with_start().unwrap().unwrap();
    assert_eq!(three.start, b"one\ntwo\n".len() as u64);
    assert_eq!(three.bytes, b"three");

    let two = lines.next_line_with_start().unwrap().unwrap();
    assert_eq!(two.start, b"one\n".len() as u64);
    assert_eq!(two.bytes, b"two");
}

#[cfg(all(feature = "codex", feature = "agent_session", feature = "discovery"))]
#[test]
fn provider_meta_probe_stops_before_malformed_line() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("rollout.jsonl");
    std::fs::write(
        &path,
        concat!(
            r#"{"timestamp":"2026-04-16T00:00:00.000Z","type":"session_meta","payload":{"session_id":"reader-probe","cwd":"/tmp/project","originator":"codex-cli","model":"gpt-5.4"}}"#,
            "\n",
            "not-json",
        ),
    )
    .unwrap();

    let provider = agent_sessions::agent_provider("codex").expect("codex provider");
    let meta = provider.parse_file_meta(path).unwrap();

    assert_eq!(meta.session_id.as_deref(), Some("reader-probe"));
    assert_eq!(meta.cwd.as_deref(), Some("/tmp/project"));
}

#[cfg(all(feature = "codex", feature = "agent_session", feature = "discovery"))]
#[test]
fn descriptor_can_probe_then_parse_agent_session_bytes_from_start() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("rollout.jsonl");
    std::fs::write(
        &path,
        concat!(
            r#"{"timestamp":"2026-04-16T00:00:00.000Z","type":"session_meta","payload":{"session_id":"reader-forward","cwd":"/tmp/project","originator":"codex-cli","model":"gpt-5.4"}}"#,
            "\n",
            r#"{"timestamp":"2026-04-16T00:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello reader"}]}}"#,
            "\n",
            r#"{"timestamp":"2026-04-16T00:00:02.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello back"}],"phase":"final_answer"}}"#,
            "\n",
        ),
    )
    .unwrap();

    let provider = agent_sessions::agent_provider("codex").expect("codex provider");
    let meta = provider.parse_file_meta(path.clone()).unwrap();
    let session = provider
        .parse_agent_session_bytes(
            std::fs::read(path).unwrap(),
            agent_sessions::ParseSelection::empty()
                .with_meta()
                .with_messages(),
        )
        .unwrap();

    let visible_text = session
        .events
        .iter()
        .filter_map(|event| match &event.body {
            agent_sessions::agent_session::Body::Prompt(prompt) => prompt.text.as_deref(),
            agent_sessions::agent_session::Body::Response(response) => response.text.as_deref(),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(meta.session_id.as_deref(), Some("reader-forward"));
    assert_eq!(session.meta.session_id.as_deref(), Some("reader-forward"));
    assert_eq!(visible_text, vec!["hello reader", "hello back"]);
}
