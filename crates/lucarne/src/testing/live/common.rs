use super::providers::{configured_live_providers, preflight_live_provider, LiveProvider};
use super::recording::{prepare_recorded_provider, PreparedRecordingRun, RecordedLiveCase};
use crate::dialect::{Input, SessionParams};
use crate::event::{
    Decision, Event, Kind, Payload, PermissionAnswer, PermissionRequest, PermissionResponse,
    Timeline, TimelineItem, TimelineType, TurnCompleted, TurnFailed,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

pub const LIVE_ENABLED_ENV: &str = "LUCARNE_LIVE_E2E";

pub type PermissionResponseHook =
    Arc<dyn Fn(&PermissionRequest) -> Option<PermissionResponse> + Send + Sync>;
pub type InterruptPredicate = Arc<dyn Fn(&Event) -> bool + Send + Sync>;

#[derive(Clone)]
pub struct LiveTurnSpec {
    pub provider: LiveProvider,
    pub workdir: PathBuf,
    pub prompt: String,
    pub recorded_case: Option<RecordedLiveCase>,
}

impl LiveTurnSpec {
    pub fn new(provider: LiveProvider, workdir: PathBuf, prompt: impl Into<String>) -> Self {
        Self {
            provider,
            workdir,
            prompt: prompt.into(),
            recorded_case: None,
        }
    }

    pub fn recorded(mut self, suite: &'static str, case_id: &'static str) -> Self {
        self.recorded_case = Some(RecordedLiveCase { suite, case_id });
        self
    }
}

#[derive(Clone, Default)]
pub struct LiveTurnHooks {
    pub permission_response: Option<PermissionResponseHook>,
    pub interrupt_on_event: Option<InterruptPredicate>,
}

#[derive(Debug, Clone, Default)]
pub struct LiveTurnResult {
    pub events: Vec<Event>,
    pub closed: bool,
}

pub async fn live_providers() -> Vec<LiveProvider> {
    if std::env::var(LIVE_ENABLED_ENV).unwrap_or_default() != "1" {
        return Vec::new();
    }
    configured_live_providers()
}

pub async fn run_live_turn(spec: LiveTurnSpec) -> Result<LiveTurnResult, String> {
    run_live_turn_with_hooks(spec, LiveTurnHooks::default()).await
}

pub async fn run_live_turn_with_hooks(
    mut spec: LiveTurnSpec,
    hooks: LiveTurnHooks,
) -> Result<LiveTurnResult, String> {
    spec.workdir = live_canonical_path(&spec.workdir);
    ensure_live_git_repo(&spec.workdir)?;

    let timeout = std::env::var("LUCARNE_LIVE_TIMEOUT")
        .ok()
        .and_then(|raw| parse_duration_env(raw.trim()))
        .filter(|duration| !duration.is_zero())
        .unwrap_or_else(|| spec.provider.timeout());

    let temp_root = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
    let mut recorded = match spec.recorded_case {
        Some(case) => {
            prepare_recorded_provider(temp_root.path(), &spec.provider, case, &spec.workdir)?
        }
        None => None,
    };
    if recorded
        .as_ref()
        .map(|recorded| !recorded.is_replay())
        .unwrap_or(true)
    {
        preflight_live_provider(&spec.provider, temp_root.path(), &spec.workdir).await?;
    }
    let provider = if let Some(recorded) = recorded.as_ref() {
        recorded.provider.clone()
    } else {
        let wrapped_binary = maybe_wrap_live_binary(
            temp_root.path(),
            spec.provider.name(),
            &spec.provider.binary,
        )?;
        spec.provider.with_binary(wrapped_binary)
    };
    let adapter = provider.adapter();
    let extra_env = provider.extra_env(temp_root.path(), &spec.workdir)?;
    let extra_args = provider.extra_args(&spec.workdir);
    let session = Arc::new(
        adapter
            .start(SessionParams {
                model: provider.model.clone(),
                system_prompt:
                    "Be concise. When asked to reply with an exact token, output only that token."
                        .into(),
                cwd: spec.workdir.to_string_lossy().into_owned(),
                extra_env,
                extra_args,
                ..Default::default()
            })
            .await
            .map_err(|err| {
                format!(
                    "{} Start(model={}): {}",
                    provider.name(),
                    provider.model,
                    err
                )
            })?,
    );

    let mut rx = session
        .events()
        .await
        .ok_or_else(|| "events receiver unavailable".to_string())?;
    session
        .send(Input {
            text: spec.prompt.clone(),
            images: Vec::new(),
        })
        .await
        .map_err(|err| format!("send: {err}"))?;

    let post_turn_quiet = if recorded
        .as_ref()
        .is_some_and(PreparedRecordingRun::is_replay)
    {
        super::providers::LIVE_REPLAY_POST_TURN_QUIET
    } else {
        provider.post_turn_quiet()
    };
    let hard_deadline = Instant::now() + timeout;
    let mut events = Vec::new();
    let mut terminal_observed = false;
    let mut quiet_deadline: Option<Instant> = None;
    let mut interrupted = false;

    loop {
        let maybe_event = if let Some(deadline) = quiet_deadline {
            tokio::select! {
                biased;
                maybe_ev = rx.recv() => maybe_ev,
                _ = tokio::time::sleep_until(deadline) => break,
                _ = tokio::time::sleep_until(hard_deadline) => {
                    return Err(format!(
                        "scenario timeout; events so far: {}",
                        summarize_live_events(&events)
                    ));
                }
            }
        } else {
            tokio::select! {
                biased;
                maybe_ev = rx.recv() => maybe_ev,
                _ = tokio::time::sleep_until(hard_deadline) => {
                    return Err(format!(
                        "scenario timeout; events so far: {}",
                        summarize_live_events(&events)
                    ));
                }
            }
        };

        let Some(event) = maybe_event else {
            break;
        };

        if let Payload::PermissionRequest(req) = &event.payload {
            let resp = hooks
                .permission_response
                .as_ref()
                .and_then(|hook| hook(req))
                .unwrap_or_else(|| live_permission_response(req));
            session
                .resolve_with_response(&req.req_id, &resp)
                .await
                .map_err(|err| format!("resolve {}: {err}", req.req_id))?;
        }

        if !interrupted {
            if let Some(predicate) = &hooks.interrupt_on_event {
                if predicate(&event) {
                    session
                        .interrupt()
                        .await
                        .map_err(|err| format!("interrupt: {err}"))?;
                    interrupted = true;
                }
            }
        }

        if matches!(
            event.payload,
            Payload::TurnCompleted(_) | Payload::TurnFailed(_)
        ) {
            if let Some(recorded) = recorded.as_mut() {
                recorded.apply_recorded_effects(&spec.workdir)?;
            }
            terminal_observed = true;
            quiet_deadline = Some(Instant::now() + post_turn_quiet);
        } else if terminal_observed {
            quiet_deadline = Some(Instant::now() + post_turn_quiet);
        }
        events.push(event);
    }

    session.close().await;

    let close_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        tokio::select! {
            maybe_ev = rx.recv() => {
                let Some(event) = maybe_ev else {
                    break;
                };
                events.push(event);
            }
            _ = tokio::time::sleep_until(close_deadline) => {
                return Err(format!(
                    "live session did not close after turn; events so far: {}",
                    summarize_live_events(&events)
                ));
            }
        }
    }

    let closed = events
        .iter()
        .any(|event| matches!(event.payload, Payload::SessionClosed(_)));
    if let Some(recorded) = recorded.as_mut() {
        recorded.finish(&spec.workdir)?;
    }
    Ok(LiveTurnResult { events, closed })
}

pub fn ensure_live_git_repo(workdir: &Path) -> Result<(), String> {
    let output = std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(workdir)
        .output()
        .map_err(|err| format!("git init {}: {err}", workdir.display()))?;
    if !output.status.success() {
        return Err(format!(
            "git init {}: {}",
            workdir.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

pub fn live_canonical_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub fn maybe_wrap_live_binary(
    script_dir: &Path,
    provider_name: &str,
    real_binary: &str,
) -> Result<String, String> {
    if std::env::var("LUCARNE_LIVE_CAPTURE_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .is_none()
    {
        return Ok(real_binary.into());
    }
    #[cfg(unix)]
    {
        fs::create_dir_all(script_dir)
            .map_err(|err| format!("mkdir script dir {}: {err}", script_dir.display()))?;
        let capture_root =
            Path::new(&std::env::var("LUCARNE_LIVE_CAPTURE_DIR").unwrap()).join(provider_name);
        fs::create_dir_all(&capture_root)
            .map_err(|err| format!("mkdir capture dir {}: {err}", capture_root.display()))?;
        let script_path = script_dir.join(format!("{provider_name}-capture.sh"));
        let script = format!(
            "#!/usr/bin/env bash\nset -euo pipefail\nreal_binary={real_binary:?}\ncapture_root={capture_root:?}\nmkdir -p \"$capture_root\"\nstamp=\"$(date +%Y%m%dT%H%M%S)-$$\"\nstdin_log=\"$capture_root/${{stamp}}.stdin\"\nstdout_log=\"$capture_root/${{stamp}}.stdout\"\nstderr_log=\"$capture_root/${{stamp}}.stderr\"\nstdin_pipe=\"$capture_root/.${{stamp}}.stdin.fifo\"\ncleanup() {{\n  rm -f \"$stdin_pipe\"\n}}\ntrap cleanup EXIT\nmkfifo \"$stdin_pipe\"\ncat <&0 | tee \"$stdin_log\" > \"$stdin_pipe\" &\ncat_pid=$!\n\"$real_binary\" \"$@\" < \"$stdin_pipe\" > >(tee \"$stdout_log\") 2> >(tee \"$stderr_log\" >&2)\nstatus=$?\nwait \"$cat_pid\" || true\nexit \"$status\"\n",
            real_binary = real_binary,
            capture_root = capture_root.to_string_lossy()
        );
        fs::write(&script_path, script)
            .map_err(|err| format!("write wrapper script {}: {err}", script_path.display()))?;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755))
            .map_err(|err| format!("chmod {}: {err}", script_path.display()))?;
        Ok(script_path.to_string_lossy().into_owned())
    }
    #[cfg(windows)]
    {
        let _ = script_dir;
        let _ = provider_name;
        Err("live capture wrappers are not supported on Windows".into())
    }
}

pub fn live_permission_response(req: &PermissionRequest) -> PermissionResponse {
    let mut resp = PermissionResponse::from_decision(Decision::Allow);
    if req.questions.is_empty() {
        return resp;
    }

    let mut answers = BTreeMap::new();
    for question in &req.questions {
        let key = first_non_empty(&[
            Some(question.id.clone()),
            Some(question.header.clone()),
            Some(question.question.clone()),
        ]);
        if key.is_empty() {
            continue;
        }
        if let Some(first) = question.options.first() {
            answers.insert(
                key,
                PermissionAnswer {
                    answers: vec![first.label.clone()],
                    text: String::new(),
                },
            );
        } else {
            answers.insert(
                key,
                PermissionAnswer {
                    answers: Vec::new(),
                    text: "yes".into(),
                },
            );
        }
    }
    if !answers.is_empty() {
        resp.answers = answers;
    }
    resp
}

fn first_non_empty(candidates: &[Option<String>]) -> String {
    candidates
        .iter()
        .filter_map(|value| value.as_ref())
        .find(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or_default()
}

pub fn summarize_live_events(events: &[Event]) -> String {
    if events.is_empty() {
        return "<none>".into();
    }
    let mut parts = Vec::with_capacity(events.len());
    for event in events {
        match &event.payload {
            Payload::SessionStarted(payload) => {
                parts.push(format!(
                    "session_started(session_id={})",
                    payload.session_id
                ));
            }
            Payload::TurnCompleted(_) => parts.push("turn_completed".into()),
            Payload::TurnFailed(payload) => parts.push(format!("turn_failed({})", payload.error)),
            Payload::Timeline(payload) => parts.push(format!("timeline({:?})", payload.item.ty)),
            Payload::PermissionRequest(payload) => {
                parts.push(format!("permission_request({})", payload.tool));
            }
            Payload::Log(payload) => {
                let mut text = payload.text.trim().to_string();
                if text.len() > 80 {
                    text.truncate(80);
                    text.push_str("...");
                }
                parts.push(format!("log({text})"));
            }
            Payload::SessionClosed(payload) => {
                parts.push(format!("session_closed({})", payload.reason));
            }
            _ => parts.push(format!("{:?}", event.kind())),
        }
    }
    parts.join(", ")
}

pub fn collect_timelines(events: &[Event], ty: TimelineType) -> Vec<TimelineItem> {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            Payload::Timeline(Timeline { item }) if item.ty == ty => Some(item.clone()),
            _ => None,
        })
        .collect()
}

pub fn find_events_by_kind(events: &[Event], kind: Kind) -> Vec<&Event> {
    events.iter().filter(|event| event.kind() == kind).collect()
}

pub fn assistant_transcript(items: &[TimelineItem]) -> String {
    items
        .iter()
        .filter_map(|item| item.assistant_message.as_ref())
        .map(|message| message.text.as_str())
        .collect::<Vec<_>>()
        .join("")
}

pub fn failed_messages(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            Payload::TurnFailed(failed) => Some(if failed.code.is_empty() {
                failed.error.clone()
            } else {
                format!("{} ({})", failed.error, failed.code)
            }),
            _ => None,
        })
        .collect()
}

pub fn contains_string(items: &[String], needle: &str) -> bool {
    items.iter().any(|item| item.contains(needle))
}

pub fn first_payload<T, F>(events: &[Event], mut f: F) -> Option<T>
where
    F: FnMut(&Payload) -> Option<T>,
{
    events.iter().find_map(|event| f(&event.payload))
}

pub fn turn_completed(events: &[Event]) -> Option<TurnCompleted> {
    first_payload(events, |payload| match payload {
        Payload::TurnCompleted(done) => Some(done.clone()),
        _ => None,
    })
}

pub fn turn_failed(events: &[Event]) -> Option<TurnFailed> {
    first_payload(events, |payload| match payload {
        Payload::TurnFailed(failed) => Some(failed.clone()),
        _ => None,
    })
}

fn parse_duration_env(raw: &str) -> Option<Duration> {
    if raw.is_empty() {
        return None;
    }
    if let Some(ms) = raw.strip_suffix("ms") {
        return ms.trim().parse::<u64>().ok().map(Duration::from_millis);
    }
    if let Some(secs) = raw.strip_suffix('s') {
        return secs.trim().parse::<u64>().ok().map(Duration::from_secs);
    }
    if let Some(mins) = raw.strip_suffix('m') {
        return mins
            .trim()
            .parse::<u64>()
            .ok()
            .map(|mins| Duration::from_secs(mins * 60));
    }
    if let Some(hours) = raw.strip_suffix('h') {
        return hours
            .trim()
            .parse::<u64>()
            .ok()
            .map(|hours| Duration::from_secs(hours * 60 * 60));
    }
    raw.parse::<u64>().ok().map(Duration::from_secs)
}
