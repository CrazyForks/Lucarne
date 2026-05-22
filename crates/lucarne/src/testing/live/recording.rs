use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use super::providers::{live_provider_by_name, LiveProvider};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordedLiveCase {
    pub suite: &'static str,
    pub case_id: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingMode {
    Replay,
    LiveRecord,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedLine {
    pub ts_nanos: u128,
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SideEffectManifest {
    pub writes: Vec<RecordWrite>,
    pub deletes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordWrite {
    pub path: String,
    pub contents_base64: String,
}

#[derive(Debug, Clone)]
pub struct PreparedRecordingRun {
    pub provider: LiveProvider,
    replay_effects: Option<SideEffectManifest>,
    recorder: Option<LiveRecorder>,
    effects_applied: bool,
}

impl PreparedRecordingRun {
    pub fn is_replay(&self) -> bool {
        self.recorder.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkdirSnapshot {
    files: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug, Clone)]
struct LiveRecorder {
    provider_name: &'static str,
    bundle: BundlePaths,
    before: WorkdirSnapshot,
    finished: bool,
}

#[derive(Debug, Clone)]
struct BundlePaths {
    root: PathBuf,
    fixture: PathBuf,
    effects: PathBuf,
    wire: PathBuf,
}

pub fn select_recording_mode(
    has_fixture: bool,
    live_enabled: bool,
    force_rerecord: bool,
) -> RecordingMode {
    if force_rerecord && live_enabled {
        return RecordingMode::LiveRecord;
    }
    if has_fixture {
        return RecordingMode::Replay;
    }
    if live_enabled {
        return RecordingMode::LiveRecord;
    }
    RecordingMode::Unavailable
}

pub fn recorded_provider_or_return(
    provider_name: &str,
    case: RecordedLiveCase,
) -> Option<LiveProvider> {
    let bundle = bundle_paths(case, provider_name);
    let fixture_exists = bundle.fixture.exists();
    match select_recording_mode(fixture_exists, live_enabled(), force_rerecord()) {
        RecordingMode::Replay => live_provider_by_name(provider_name),
        RecordingMode::LiveRecord => super::providers::configured_live_providers()
            .into_iter()
            .find(|provider| provider.name() == provider_name),
        RecordingMode::Unavailable => {
            panic!(
                "missing live recording bundle for {provider_name} case {}/{}; rerun with LUCARNE_LIVE_E2E=1 to record it",
                case.suite,
                case.case_id,
            );
        }
    }
}

pub fn prepare_recorded_provider(
    script_dir: &Path,
    provider: &LiveProvider,
    case: RecordedLiveCase,
    workdir: &Path,
) -> Result<Option<PreparedRecordingRun>, String> {
    let bundle = bundle_paths(case, provider.name());
    let mode = select_recording_mode(bundle.fixture.exists(), live_enabled(), force_rerecord());
    match mode {
        RecordingMode::Unavailable => Err(format!(
            "missing live recording bundle for {} case {}/{}; rerun with LUCARNE_LIVE_E2E=1 to record it",
            provider.name(),
            case.suite,
            case.case_id,
        )),
        RecordingMode::Replay => {
            let binary = write_replay_wrapper(script_dir, provider.name(), &bundle)?;
            let effects = load_side_effects(&bundle.effects)?;
            Ok(Some(PreparedRecordingRun {
                provider: provider.with_binary(binary),
                replay_effects: effects,
                recorder: None,
                effects_applied: false,
            }))
        }
        RecordingMode::LiveRecord => {
            fs::create_dir_all(&bundle.root)
                .map_err(|err| format!("mkdir {}: {err}", bundle.root.display()))?;
            if bundle.wire.exists() {
                fs::remove_file(&bundle.wire)
                    .map_err(|err| format!("remove {}: {err}", bundle.wire.display()))?;
            }
            let binary = write_capture_wrapper(script_dir, provider, &bundle)?;
            Ok(Some(PreparedRecordingRun {
                provider: provider.with_binary(binary),
                replay_effects: None,
                recorder: Some(LiveRecorder {
                    provider_name: provider.name(),
                    bundle,
                    before: snapshot_workdir(workdir)?,
                    finished: false,
                }),
                effects_applied: false,
            }))
        }
    }
}

pub fn compile_fixture_script(
    provider_name: &str,
    stdin: &[CapturedLine],
    stdout: &[CapturedLine],
    stderr: &[CapturedLine],
    signals: &[CapturedLine],
    exit_code: i32,
) -> Result<String, String> {
    let mut records = Vec::new();
    records.extend(
        stdin
            .iter()
            .cloned()
            .map(|line| (line.ts_nanos, 0u8, "stdin", line.line)),
    );
    records.extend(
        stdout
            .iter()
            .cloned()
            .map(|line| (line.ts_nanos, 1u8, "stdout", line.line)),
    );
    records.extend(
        stderr
            .iter()
            .cloned()
            .map(|line| (line.ts_nanos, 2u8, "stderr", line.line)),
    );
    records.extend(
        signals
            .iter()
            .cloned()
            .map(|line| (line.ts_nanos, 3u8, "signal", line.line)),
    );
    records.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    let mut script = String::new();
    script.push_str(&format!("# Recorded live capture for {provider_name}\n"));
    for (_, _, stream, line) in records {
        match stream {
            "stdin" => {
                for expect in expectation_fragments(&line)? {
                    script.push_str("EXPECT_IN_CONTAINS_NEXT ");
                    script.push_str(&go_quote(&expect));
                    script.push('\n');
                }
            }
            "stdout" => {
                script.push_str("OUT ");
                script.push_str(&line);
                script.push('\n');
            }
            "stderr" => {
                script.push_str("STDERR ");
                script.push_str(&line);
                script.push('\n');
            }
            "signal" => {
                script.push_str("EXPECT_SIGNAL_NEXT ");
                script.push_str(line.trim());
                script.push('\n');
            }
            _ => {}
        }
    }
    script.push_str(&format!("EXIT {exit_code}\n"));
    Ok(script)
}

pub fn snapshot_workdir(root: &Path) -> Result<WorkdirSnapshot, String> {
    let mut snapshot = WorkdirSnapshot::default();
    collect_files(root, root, &mut snapshot.files)?;
    Ok(snapshot)
}

pub fn diff_side_effects(
    before: &WorkdirSnapshot,
    after: &WorkdirSnapshot,
) -> Result<SideEffectManifest, String> {
    let mut manifest = SideEffectManifest::default();

    for (path, bytes) in &after.files {
        if before.files.get(path) == Some(bytes) {
            continue;
        }
        manifest.writes.push(RecordWrite {
            path: path.clone(),
            contents_base64: STANDARD.encode(sanitize_recorded_bytes(path, bytes)),
        });
    }

    for path in before.files.keys() {
        if !after.files.contains_key(path) {
            manifest.deletes.push(path.clone());
        }
    }

    manifest.writes.sort_by(|a, b| a.path.cmp(&b.path));
    manifest.deletes.sort();
    Ok(manifest)
}

fn sanitize_recorded_bytes(path: &str, bytes: &[u8]) -> Vec<u8> {
    if !path.ends_with(".jsonl") {
        return bytes.to_vec();
    }
    let Ok(raw) = std::str::from_utf8(bytes) else {
        return bytes.to_vec();
    };
    let mut out = String::new();
    for line in raw.lines() {
        if line.trim().is_empty() {
            out.push('\n');
            continue;
        }
        let Ok(mut value) = serde_json::from_str::<Value>(line) else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        redact_recorded_json(&mut value);
        match serde_json::to_string(&value) {
            Ok(line) => {
                out.push_str(&line);
                out.push('\n');
            }
            Err(_) => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out.into_bytes()
}

fn redact_recorded_json(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for key in [
                "base_instructions",
                "developer_instructions",
                "encrypted_content",
            ] {
                map.remove(key);
            }
            for value in map.values_mut() {
                redact_recorded_json(value);
            }
        }
        Value::Array(items) => {
            for value in items {
                redact_recorded_json(value);
            }
        }
        _ => {}
    }
}

pub fn apply_side_effects(root: &Path, manifest: &SideEffectManifest) -> Result<(), String> {
    for path in &manifest.deletes {
        let target = root.join(path);
        if target.exists() {
            fs::remove_file(&target)
                .map_err(|err| format!("remove {}: {err}", target.display()))?;
        }
    }

    for write in &manifest.writes {
        let target = root.join(&write.path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("mkdir {}: {err}", parent.display()))?;
        }
        let bytes = STANDARD
            .decode(&write.contents_base64)
            .map_err(|err| format!("decode {}: {err}", write.path))?;
        fs::write(&target, bytes).map_err(|err| format!("write {}: {err}", target.display()))?;
    }

    Ok(())
}

impl PreparedRecordingRun {
    pub fn apply_recorded_effects(&mut self, workdir: &Path) -> Result<(), String> {
        if self.effects_applied {
            return Ok(());
        }
        let Some(effects) = self.replay_effects.as_ref() else {
            return Ok(());
        };
        apply_side_effects(workdir, effects)?;
        self.effects_applied = true;
        Ok(())
    }

    pub fn finish(&mut self, workdir: &Path) -> Result<(), String> {
        if let Some(recorder) = self.recorder.as_mut() {
            recorder.finish(workdir)?;
        }
        Ok(())
    }
}

pub fn repo_root() -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn collect_files(
    root: &Path,
    dir: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|err| format!("read_dir {}: {err}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("read_dir entry {}: {err}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|err| format!("file_type {}: {err}", path.display()))?;
        let rel = path
            .strip_prefix(root)
            .map_err(|err| format!("strip_prefix {}: {err}", path.display()))?;
        if skip_recorded_path(rel) {
            continue;
        }
        if file_type.is_dir() {
            collect_files(root, &path, files)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        files.insert(
            rel.to_string_lossy().into_owned(),
            fs::read(&path).map_err(|err| format!("read {}: {err}", path.display()))?,
        );
    }
    Ok(())
}

fn skip_recorded_path(rel: &Path) -> bool {
    let mut components = rel.components();
    let Some(first) = components.next() else {
        return false;
    };
    if first.as_os_str() == ".git" {
        return true;
    }
    if first.as_os_str() == ".codex-home" {
        return components
            .next()
            .map(|component| component.as_os_str() != "sessions")
            .unwrap_or(false);
    }
    false
}

fn expectation_fragments(line: &str) -> Result<Vec<String>, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let value = match serde_json::from_str::<Value>(trimmed) {
        Ok(value) => value,
        Err(_) => return Ok(vec![trimmed.to_string()]),
    };

    let mut fragments = Vec::new();
    collect_expectation_fragments(None, &value, &mut fragments);
    let ordered = ordered_fragments(trimmed, &fragments);
    if ordered.is_empty() {
        return Ok(vec![trimmed.to_string()]);
    }
    Ok(ordered)
}

fn ordered_fragments(line: &str, fragments: &[String]) -> Vec<String> {
    let mut remaining = fragments.to_vec();
    let mut ordered = Vec::with_capacity(remaining.len());
    let mut cursor = 0usize;
    while !remaining.is_empty() {
        let mut best: Option<(usize, usize)> = None;
        for (idx, fragment) in remaining.iter().enumerate() {
            let Some(offset) = line[cursor..].find(fragment) else {
                continue;
            };
            let position = cursor + offset;
            match best {
                Some((best_pos, best_idx))
                    if position > best_pos
                        || (position == best_pos
                            && fragment.len() <= remaining[best_idx].len()) => {}
                _ => best = Some((position, idx)),
            }
        }
        let Some((position, idx)) = best else {
            break;
        };
        let fragment = remaining.remove(idx);
        cursor = position + fragment.len();
        ordered.push(fragment);
    }
    ordered
}

fn collect_expectation_fragments(key: Option<&str>, value: &Value, fragments: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(method) = map.get("method").and_then(Value::as_str) {
                push_fragment(fragments, format!(r#""method":"{method}""#));
            }
            if let Some(kind) = map.get("type").and_then(Value::as_str) {
                push_fragment(fragments, format!(r#""type":"{kind}""#));
            }
            if let Some(id) = map.get("id") {
                if !id.is_null() {
                    push_fragment(fragments, format!(r#""id":{}"#, id));
                }
            }
            for (child_key, child_value) in map {
                collect_expectation_fragments(Some(child_key.as_str()), child_value, fragments);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_expectation_fragments(key, item, fragments);
            }
        }
        Value::String(text) => collect_string_fragments(key, text, fragments),
        _ => {}
    }
}

fn collect_string_fragments(key: Option<&str>, text: &str, fragments: &mut Vec<String>) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }

    match key {
        Some("cwd") | Some("path") => return,
        Some("session_id") | Some("sessionId") | Some("thread_id") | Some("threadId")
        | Some("behavior") | Some("subtype") | Some("tool") => {
            push_fragment(fragments, format!(r#""{}":"{}""#, key.unwrap(), trimmed));
            return;
        }
        _ => {}
    }

    if matches!(key, Some("prompt") | Some("text")) {
        let before = fragments.len();
        if let Some(prefix) = stable_text_prefix(trimmed) {
            push_fragment(fragments, prefix);
        }
        for basename in path_basenames(trimmed) {
            push_fragment(fragments, basename);
        }
        for token in sentinel_tokens(trimmed) {
            push_fragment(fragments, token);
        }
        if fragments.len() == before && trimmed.len() <= 96 {
            push_fragment(fragments, trimmed.to_string());
        }
    }
}

fn stable_text_prefix(text: &str) -> Option<String> {
    let cutoff = first_path_index(text).unwrap_or(text.len());
    let prefix = text[..cutoff].trim();
    if prefix.len() < 12 {
        return None;
    }
    Some(
        prefix
            .chars()
            .take(64)
            .collect::<String>()
            .trim()
            .to_string(),
    )
}

fn first_path_index(text: &str) -> Option<usize> {
    ["/", "./", "../"]
        .into_iter()
        .filter_map(|needle| text.find(needle))
        .min()
}

fn path_basenames(text: &str) -> Vec<String> {
    let mut basenames = Vec::new();
    for raw in text.split_whitespace() {
        let cleaned = raw.trim_matches(|ch: char| {
            matches!(
                ch,
                '"' | '\'' | ',' | '.' | ':' | ';' | '(' | ')' | '[' | ']' | '`'
            )
        });
        if !(cleaned.starts_with('/') || cleaned.starts_with("./") || cleaned.starts_with("../")) {
            continue;
        }
        let Some(name) = Path::new(cleaned)
            .file_name()
            .and_then(|part| part.to_str())
        else {
            continue;
        };
        if name.len() >= 3 {
            push_fragment(&mut basenames, name.to_string());
        }
    }
    basenames
}

fn sentinel_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for raw in text.split(|ch: char| {
        ch.is_whitespace()
            || matches!(
                ch,
                '"' | '\'' | ',' | '.' | ':' | ';' | '(' | ')' | '[' | ']' | '`'
            )
    }) {
        let token = raw.trim();
        if token.len() < 6 {
            continue;
        }
        if token
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch == '_' || ch.is_ascii_digit())
        {
            push_fragment(&mut tokens, token.to_string());
        }
    }
    tokens
}

fn push_fragment(fragments: &mut Vec<String>, fragment: String) {
    if fragment.is_empty() || fragments.contains(&fragment) {
        return;
    }
    fragments.push(fragment);
}

fn go_quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for ch in text.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

impl LiveRecorder {
    fn finish(&mut self, workdir: &Path) -> Result<(), String> {
        if self.finished {
            return Ok(());
        }
        let captured = read_wire_capture(&self.bundle.wire)?;
        let fixture = compile_fixture_script(
            self.provider_name,
            &captured.stdin,
            &captured.stdout,
            &captured.stderr,
            &captured.signals,
            captured.exit_code,
        )?;
        fs::write(&self.bundle.fixture, fixture)
            .map_err(|err| format!("write {}: {err}", self.bundle.fixture.display()))?;

        let after = snapshot_workdir(workdir)?;
        let effects = diff_side_effects(&self.before, &after)?;
        fs::write(
            &self.bundle.effects,
            serde_json::to_vec_pretty(&effects)
                .map_err(|err| format!("encode {}: {err}", self.bundle.effects.display()))?,
        )
        .map_err(|err| format!("write {}: {err}", self.bundle.effects.display()))?;
        self.finished = true;
        Ok(())
    }
}

#[derive(Default)]
struct CapturedWire {
    stdin: Vec<CapturedLine>,
    stdout: Vec<CapturedLine>,
    stderr: Vec<CapturedLine>,
    signals: Vec<CapturedLine>,
    exit_code: i32,
}

fn bundle_paths(case: RecordedLiveCase, provider_name: &str) -> BundlePaths {
    let root = repo_root()
        .join("tests")
        .join("data")
        .join("live_recordings")
        .join(case.suite)
        .join(provider_name)
        .join(case.case_id);
    BundlePaths {
        fixture: root.join("session.fixture"),
        effects: root.join("effects.json"),
        wire: root.join("wire.log"),
        root,
    }
}

fn live_enabled() -> bool {
    std::env::var("LUCARNE_LIVE_E2E").unwrap_or_default() == "1"
}

fn force_rerecord() -> bool {
    std::env::var("LUCARNE_LIVE_RERECORD").unwrap_or_default() == "1"
}

fn fakeagent_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();

    BIN.get_or_init(|| {
        let root = repo_root();
        let status = Command::new("cargo")
            .args([
                "build",
                "-p",
                "lucarne-fakeagent",
                "--bin",
                "lucarne-fakeagent",
                "--quiet",
            ])
            .current_dir(&root)
            .status()
            .expect("cargo build lucarne-fakeagent");
        assert!(status.success(), "build lucarne-fakeagent failed");

        let mut target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target"));
        target.push("debug");
        target.push(if cfg!(windows) {
            "lucarne-fakeagent.exe"
        } else {
            "lucarne-fakeagent"
        });

        for _ in 0..20 {
            if target.exists() {
                return target;
            }
            thread::sleep(Duration::from_millis(50));
        }

        panic!("fakeagent missing at {:?}", target);
    })
    .clone()
}

#[cfg(unix)]
fn write_replay_wrapper(
    script_dir: &Path,
    provider_name: &str,
    bundle: &BundlePaths,
) -> Result<String, String> {
    fs::create_dir_all(script_dir)
        .map_err(|err| format!("mkdir {}: {err}", script_dir.display()))?;
    let script_path = script_dir.join("live-replay.sh");
    let script = format!(
        "#!/usr/bin/env bash\nset -euo pipefail\nif [[ \"${{1:-}}\" == \"--version\" ]]; then\n  printf '%s\\n' {version:?}\n  exit 0\nfi\nexport LUCARNE_FIXTURE={fixture:?}\nexec {fakeagent:?} \"$@\"\n",
        version = replay_version_output(provider_name),
        fixture = bundle.fixture.to_string_lossy(),
        fakeagent = fakeagent_bin().to_string_lossy(),
    );
    fs::write(&script_path, script)
        .map_err(|err| format!("write {}: {err}", script_path.display()))?;
    chmod_executable(&script_path)?;
    Ok(script_path.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn write_replay_wrapper(
    script_dir: &Path,
    provider_name: &str,
    bundle: &BundlePaths,
) -> Result<String, String> {
    fs::create_dir_all(script_dir)
        .map_err(|err| format!("mkdir {}: {err}", script_dir.display()))?;
    let script_path = script_dir.join("live-replay.cmd");
    let script = format!(
        "@echo off\r\nif \"%~1\"==\"--version\" (\r\n  echo {version}\r\n  exit /b 0\r\n)\r\nset \"LUCARNE_FIXTURE={fixture}\"\r\n\"{fakeagent}\" %*\r\n",
        version = replay_version_output(provider_name),
        fixture = bundle.fixture.display(),
        fakeagent = fakeagent_bin().display(),
    );
    fs::write(&script_path, script)
        .map_err(|err| format!("write {}: {err}", script_path.display()))?;
    Ok(script_path.to_string_lossy().into_owned())
}

fn replay_version_output(provider_name: &str) -> &'static str {
    match provider_name {
        "claude" => "claude 2.1.119",
        "codex" => "codex 0.100.0",
        "gemini" => "gemini 1.0.0",
        _ => "agent 1.0.0",
    }
}

#[cfg(unix)]
fn write_capture_wrapper(
    script_dir: &Path,
    provider: &LiveProvider,
    bundle: &BundlePaths,
) -> Result<String, String> {
    fs::create_dir_all(script_dir)
        .map_err(|err| format!("mkdir {}: {err}", script_dir.display()))?;
    let script_path = script_dir.join("live-capture.sh");
    let script = format!(
        "#!/usr/bin/env bash\nset -euo pipefail\nreal_binary={real_binary:?}\nwire_log={wire_log:?}\nmkdir -p \"$(dirname \"$wire_log\")\"\n: > \"$wire_log\"\nstdin_pipe=\"${{TMPDIR:-/tmp}}/lucarne-live-capture-$$.stdin\"\ncleanup() {{\n  rm -f \"$stdin_pipe\"\n  rmdir \"$wire_log.lock\" 2>/dev/null || true\n}}\nrecord_line() {{\n  local stream=\"$1\"\n  local line=\"$2\"\n  local encoded\n  encoded=\"$(printf '%s' \"$line\" | base64 | tr -d '\\n')\"\n  while ! mkdir \"$wire_log.lock\" 2>/dev/null; do\n    sleep 0.01\n  done\n  printf '%s\\t%s\\n' \"$stream\" \"$encoded\" >> \"$wire_log\"\n  rmdir \"$wire_log.lock\"\n}}\nchild_pid=\"\"\nforward_signal() {{\n  local sig=\"$1\"\n  record_line signal \"$sig\"\n  if [[ -n \"$child_pid\" ]]; then\n    kill -s \"${{sig#SIG}}\" \"$child_pid\" 2>/dev/null || true\n  fi\n}}\ntrap cleanup EXIT\ntrap 'forward_signal SIGINT' INT\ntrap 'forward_signal SIGTERM' TERM\ntrap 'forward_signal SIGHUP' HUP\nmkfifo \"$stdin_pipe\"\n(\n  while IFS= read -r line; do\n    record_line stdin \"$line\"\n    printf '%s\\n' \"$line\"\n  done\n) <&0 > \"$stdin_pipe\" &\nstdin_pid=$!\n\"$real_binary\" \"$@\" < \"$stdin_pipe\" > >(\n  while IFS= read -r line; do\n    record_line stdout \"$line\"\n    printf '%s\\n' \"$line\"\n  done\n) 2> >(\n  while IFS= read -r line; do\n    record_line stderr \"$line\"\n    printf '%s\\n' \"$line\" >&2\n  done\n) &\nchild_pid=$!\nstatus=0\nwhile true; do\n  if wait \"$child_pid\"; then\n    status=0\n    break\n  fi\n  status=$?\n  if ! kill -0 \"$child_pid\" 2>/dev/null; then\n    break\n  fi\ndone\nrecord_line exit \"$status\"\nkill \"$stdin_pid\" 2>/dev/null || true\nwait \"$stdin_pid\" || true\nexit \"$status\"\n",
        real_binary = provider.binary,
        wire_log = bundle.wire.to_string_lossy(),
    );
    fs::write(&script_path, script)
        .map_err(|err| format!("write {}: {err}", script_path.display()))?;
    chmod_executable(&script_path)?;
    Ok(script_path.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn write_capture_wrapper(
    _script_dir: &Path,
    _provider: &LiveProvider,
    _bundle: &BundlePaths,
) -> Result<String, String> {
    Err("live recording capture wrappers are not supported on Windows".into())
}

fn read_wire_capture(path: &Path) -> Result<CapturedWire, String> {
    let raw = fs::read_to_string(path).map_err(|err| format!("read {}: {err}", path.display()))?;
    let mut seq = 0u128;
    let mut captured = CapturedWire::default();
    for line in raw.lines() {
        let Some((stream, encoded)) = line.split_once('\t') else {
            continue;
        };
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|err| format!("decode {}: {err}", path.display()))?;
        let text =
            String::from_utf8(bytes).map_err(|err| format!("utf8 {}: {err}", path.display()))?;
        seq += 1;
        let exit_code = if stream == "exit" {
            Some(
                text.trim()
                    .parse()
                    .map_err(|err| format!("parse exit {}: {err}", path.display()))?,
            )
        } else {
            None
        };
        let captured_line = CapturedLine {
            ts_nanos: seq,
            line: text,
        };
        match stream {
            "stdin" => captured.stdin.push(captured_line),
            "stdout" => captured.stdout.push(captured_line),
            "stderr" => captured.stderr.push(captured_line),
            "signal" => captured.signals.push(captured_line),
            "exit" => {
                captured.exit_code = exit_code.expect("exit code parsed above");
            }
            _ => {}
        }
    }
    Ok(captured)
}

fn load_side_effects(path: &Path) -> Result<Option<SideEffectManifest>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(path).map_err(|err| format!("read {}: {err}", path.display()))?;
    let manifest = serde_json::from_slice(&bytes)
        .map_err(|err| format!("decode {}: {err}", path.display()))?;
    Ok(Some(manifest))
}

#[cfg(unix)]
fn chmod_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .map_err(|err| format!("chmod {}: {err}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expectation_fragments_follow_raw_json_order() {
        let line = r#"{"id":2,"jsonrpc":"2.0","method":"turn/start","params":{"input":[{"text":"[Image #1]","type":"text"},{"type":"image","url":"data:image/png;base64,AAAA"},{"type":"text","text":"Reply with exactly SHRIMP if the attached image is a shrimp illustration, otherwise reply with exactly UNKNOWN."}],"threadId":"thr-1"}}"#;
        let fragments = expectation_fragments(line).unwrap();

        let id = fragments.iter().position(|f| f == r#""id":2"#).unwrap();
        let method = fragments
            .iter()
            .position(|f| f == r#""method":"turn/start""#)
            .unwrap();
        let image_marker = fragments.iter().position(|f| f == "[Image #1]").unwrap();
        let text_type = fragments
            .iter()
            .position(|f| f == r#""type":"text""#)
            .unwrap();
        let image_type = fragments
            .iter()
            .position(|f| f == r#""type":"image""#)
            .unwrap();
        let prompt_prefix = fragments
            .iter()
            .position(|f| f.contains("Reply with exactly SHRIMP"))
            .unwrap();
        let thread_id = fragments
            .iter()
            .position(|f| f == r#""threadId":"thr-1""#)
            .unwrap();

        assert!(id < method, "{fragments:?}");
        assert!(image_marker < text_type, "{fragments:?}");
        assert!(text_type < image_type, "{fragments:?}");
        assert!(image_type < prompt_prefix, "{fragments:?}");
        assert!(prompt_prefix < thread_id, "{fragments:?}");
    }
}
