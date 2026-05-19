use super::common::{ensure_live_git_repo, live_canonical_path, maybe_wrap_live_binary};
use super::providers::{preflight_live_provider, LiveProvider};
use super::recording::{prepare_recorded_provider, PreparedRecordingRun, RecordedLiveCase};
use crate::agent_runtime::{
    AgentEventStream, AgentInput, AgentRuntime, AgentSession, ApprovalDecision, ApprovalRequest,
    Event as RuntimeEvent, InterventionRequest, InterventionResponse, OpenSession,
    ProtocolProvider, Question, QuestionAnswer, QuestionRequest, QuestionResponse, ResumeSession,
    SessionRef,
};
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

pub type RuntimeApprovalResponseHook =
    Arc<dyn Fn(&ApprovalRequest) -> Option<ApprovalDecision> + Send + Sync>;
pub type RuntimeQuestionResponseHook =
    Arc<dyn Fn(&QuestionRequest) -> Option<QuestionResponse> + Send + Sync>;
pub type RuntimeInterruptPredicate = Arc<dyn Fn(&RuntimeEvent) -> bool + Send + Sync>;
pub type RuntimeFinishPredicate = Arc<dyn Fn(&[RuntimeEvent]) -> bool + Send + Sync>;

#[derive(Clone, Copy)]
pub enum ReplayEffectsTrigger {
    OnStreamClose,
    OnAssistantMessageContains(&'static str),
    OnApprovalThenSuccessToolResult,
}

#[derive(Clone)]
pub struct LiveRuntimeTurnSpec {
    pub provider: LiveProvider,
    pub workdir: std::path::PathBuf,
    pub prompt: String,
    pub input: AgentInput,
    pub recorded_case: Option<RecordedLiveCase>,
    pub replay_effects_trigger: ReplayEffectsTrigger,
}

impl LiveRuntimeTurnSpec {
    pub fn new(
        provider: LiveProvider,
        workdir: std::path::PathBuf,
        prompt: impl Into<String>,
    ) -> Self {
        let prompt = prompt.into();
        Self {
            provider,
            workdir,
            prompt: prompt.clone(),
            input: AgentInput {
                text: prompt.into(),
                images: Vec::new(),
            },
            recorded_case: None,
            replay_effects_trigger: ReplayEffectsTrigger::OnStreamClose,
        }
    }

    pub fn with_input(mut self, input: AgentInput) -> Self {
        self.prompt = input.text.to_string();
        self.input = input;
        self
    }

    pub fn recorded(mut self, suite: &'static str, case_id: &'static str) -> Self {
        self.recorded_case = Some(RecordedLiveCase { suite, case_id });
        self
    }

    pub fn replay_effects_on(mut self, trigger: ReplayEffectsTrigger) -> Self {
        self.replay_effects_trigger = trigger;
        self
    }
}

#[derive(Clone, Default)]
pub struct LiveRuntimeHooks {
    pub approval_response: Option<RuntimeApprovalResponseHook>,
    pub question_response: Option<RuntimeQuestionResponseHook>,
    pub interrupt_on_event: Option<RuntimeInterruptPredicate>,
    pub finish_when: Option<RuntimeFinishPredicate>,
}

#[derive(Debug, Clone, Default)]
pub struct LiveRuntimeResult {
    pub events: Vec<RuntimeEvent>,
    pub closed: bool,
}

pub struct LiveRuntimeSession {
    pub provider: LiveProvider,
    pub timeout: Duration,
    pub session: Box<dyn AgentSession>,
    pub events: LiveRuntimeEventStream,
    _temp_root: Arc<tempfile::TempDir>,
}

pub struct LiveRuntimeEventStream {
    inner: AgentEventStream,
    workdir: std::path::PathBuf,
    recorded: Option<PreparedRecordingRun>,
    replay_effects_trigger: ReplayEffectsTrigger,
    saw_approval: bool,
}

impl LiveRuntimeSession {
    pub fn post_turn_quiet(&self) -> Duration {
        if self
            .events
            .recorded
            .as_ref()
            .is_some_and(PreparedRecordingRun::is_replay)
        {
            super::providers::LIVE_REPLAY_POST_TURN_QUIET
        } else {
            self.provider.post_turn_quiet()
        }
    }
}

impl LiveRuntimeEventStream {
    pub async fn recv(&mut self) -> Option<RuntimeEvent> {
        let event = self.inner.recv().await;
        if let Some(event) = event.as_ref() {
            if matches!(
                event,
                RuntimeEvent::InterventionRequest(InterventionRequest::Approval(_))
            ) {
                self.saw_approval = true;
            }
            if let Some(recorded) = self.recorded.as_mut() {
                if should_apply_replay_effects(
                    self.replay_effects_trigger,
                    self.saw_approval,
                    event,
                ) {
                    recorded
                        .apply_recorded_effects(&self.workdir)
                        .unwrap_or_else(|err| panic!("apply runtime replay effects: {err}"));
                }
            }
        } else if let Some(recorded) = self.recorded.as_mut() {
            recorded
                .apply_recorded_effects(&self.workdir)
                .unwrap_or_else(|err| panic!("apply runtime replay effects on close: {err}"));
            recorded
                .finish(&self.workdir)
                .unwrap_or_else(|err| panic!("finalize runtime recording: {err}"));
        }
        event
    }
}

pub async fn open_live_runtime_session(
    mut spec: LiveRuntimeTurnSpec,
) -> Result<LiveRuntimeSession, String> {
    spec.workdir = live_canonical_path(&spec.workdir);
    ensure_live_git_repo(&spec.workdir)?;

    let timeout = live_runtime_timeout(&spec.provider);
    let temp_root = Arc::new(tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?);
    let recorded = match spec.recorded_case {
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
    let runtime = live_runtime(&provider);

    let open_request = live_open_request(&provider, temp_root.path(), &spec.workdir, spec.input)?;
    let session = tokio::time::timeout(timeout, runtime.open(provider.name(), open_request))
        .await
        .map_err(|_| {
            format!(
                "{} open(model={}) timed out waiting for a provider-native session id",
                provider.name(),
                provider.model,
            )
        })?
        .map_err(|err| {
            format!(
                "{} open(model={}): {}",
                provider.name(),
                provider.model,
                err
            )
        })?;

    let events = session
        .take_events()
        .await
        .map_err(|err| format!("take_events: {err}"))?;

    Ok(LiveRuntimeSession {
        provider: spec.provider.clone(),
        timeout,
        session,
        events: LiveRuntimeEventStream {
            inner: events,
            workdir: spec.workdir.clone(),
            recorded,
            replay_effects_trigger: spec.replay_effects_trigger,
            saw_approval: false,
        },
        _temp_root: temp_root,
    })
}

pub async fn resume_live_runtime_session_from_existing(
    prior: &LiveRuntimeSession,
    workdir: std::path::PathBuf,
    session_ref: impl Into<String>,
    recorded_case: Option<RecordedLiveCase>,
) -> Result<LiveRuntimeSession, String> {
    let workdir = live_canonical_path(&workdir);
    ensure_live_git_repo(&workdir)?;

    let provider = prior.provider.clone();
    let timeout = prior.timeout;
    let temp_root = Arc::clone(&prior._temp_root);
    let recorded = match recorded_case {
        Some(case) => prepare_recorded_provider(temp_root.path(), &provider, case, &workdir)?,
        None => None,
    };
    let active_provider = if let Some(recorded) = recorded.as_ref() {
        recorded.provider.clone()
    } else {
        let wrapped_binary =
            maybe_wrap_live_binary(temp_root.path(), provider.name(), &provider.binary)?;
        provider.with_binary(wrapped_binary)
    };
    let runtime = live_runtime(&active_provider);

    let resume_request = live_resume_request(
        &active_provider,
        temp_root.path(),
        &workdir,
        session_ref.into().as_str(),
    )?;
    let session = tokio::time::timeout(timeout, runtime.resume(provider.name(), resume_request))
        .await
        .map_err(|_| {
            format!(
                "{} resume(model={}) timed out waiting for a provider-native session id",
                active_provider.name(),
                active_provider.model,
            )
        })?
        .map_err(|err| {
            format!(
                "{} resume(model={}): {}",
                active_provider.name(),
                active_provider.model,
                err
            )
        })?;

    let events = session
        .take_events()
        .await
        .map_err(|err| format!("take_events: {err}"))?;

    Ok(LiveRuntimeSession {
        provider,
        timeout,
        session,
        events: LiveRuntimeEventStream {
            inner: events,
            workdir,
            recorded,
            replay_effects_trigger: ReplayEffectsTrigger::OnStreamClose,
            saw_approval: false,
        },
        _temp_root: temp_root,
    })
}

pub async fn run_live_runtime_turn_with_hooks(
    spec: LiveRuntimeTurnSpec,
    hooks: LiveRuntimeHooks,
) -> Result<LiveRuntimeResult, String> {
    let mut live = open_live_runtime_session(spec).await?;

    let hard_deadline = Instant::now() + live.timeout;
    let mut events = Vec::new();
    let mut quiet_deadline: Option<Instant> = None;
    let mut interrupted = false;

    loop {
        let maybe_event = if let Some(deadline) = quiet_deadline {
            tokio::select! {
                biased;
                maybe_ev = live.events.recv() => maybe_ev,
                _ = tokio::time::sleep_until(deadline) => break,
                _ = tokio::time::sleep_until(hard_deadline) => {
                    return Err(format!(
                        "runtime live scenario timeout; events so far: {}",
                        summarize_runtime_events(&events)
                    ));
                }
            }
        } else {
            tokio::select! {
                biased;
                maybe_ev = live.events.recv() => maybe_ev,
                _ = tokio::time::sleep_until(hard_deadline) => {
                    return Err(format!(
                        "runtime live scenario timeout; events so far: {}",
                        summarize_runtime_events(&events)
                    ));
                }
            }
        };

        let Some(event) = maybe_event else {
            break;
        };

        if let RuntimeEvent::InterventionRequest(request) = &event {
            let response = match request {
                InterventionRequest::Approval(request) => InterventionResponse::Approval(
                    hooks
                        .approval_response
                        .as_ref()
                        .and_then(|hook| hook(request))
                        .unwrap_or(ApprovalDecision::Allow),
                ),
                InterventionRequest::Question(request) => InterventionResponse::Answers(
                    hooks
                        .question_response
                        .as_ref()
                        .and_then(|hook| hook(request))
                        .unwrap_or_else(|| live_question_response(request)),
                ),
            };
            let req_id = match request {
                InterventionRequest::Approval(request) => request.req_id.as_str(),
                InterventionRequest::Question(request) => request.req_id.as_str(),
            };
            live.session
                .resolve(req_id, response)
                .await
                .map_err(|err| format!("resolve {req_id}: {err}"))?;
        }

        if !interrupted {
            if let Some(predicate) = &hooks.interrupt_on_event {
                if predicate(&event) {
                    live.session
                        .interrupt()
                        .await
                        .map_err(|err| format!("interrupt: {err}"))?;
                    interrupted = true;
                }
            }
        }

        events.push(event);
        if hooks
            .finish_when
            .as_ref()
            .is_some_and(|predicate| predicate(&events))
            || quiet_deadline.is_some()
        {
            quiet_deadline = Some(Instant::now() + live.post_turn_quiet());
        }
    }

    live.session
        .close()
        .await
        .map_err(|err| format!("close: {err}"))?;

    let close_deadline = Instant::now() + Duration::from_secs(5);
    let closed = loop {
        tokio::select! {
            maybe_ev = live.events.recv() => {
                match maybe_ev {
                    Some(event) => events.push(event),
                    None => {
                        break true;
                    }
                }
            }
            _ = tokio::time::sleep_until(close_deadline) => {
                return Err(format!(
                    "runtime live session did not close after turn; events so far: {}",
                    summarize_runtime_events(&events)
                ));
            }
        }
    };

    Ok(LiveRuntimeResult { events, closed })
}

pub fn summarize_runtime_events(events: &[RuntimeEvent]) -> String {
    if events.is_empty() {
        return "<none>".into();
    }
    events
        .iter()
        .map(|event| match event {
            RuntimeEvent::Message(message) => {
                let mut text = message.text.trim().to_string();
                if text.len() > 60 {
                    text.truncate(60);
                    text.push_str("...");
                }
                format!("message({:?}:{text})", message.role)
            }
            RuntimeEvent::Attachment(attachment) => {
                format!(
                    "attachment({}:{})",
                    attachment.media_type, attachment.filename
                )
            }
            RuntimeEvent::Reasoning(reasoning) => {
                let mut text = reasoning.text.trim().to_string();
                if text.len() > 60 {
                    text.truncate(60);
                    text.push_str("...");
                }
                format!("reasoning({text})")
            }
            RuntimeEvent::ToolCall(tool_call) => format!("tool_call({})", tool_call.name),
            RuntimeEvent::ToolResult(tool_result) => {
                format!(
                    "tool_result(error={})",
                    tool_result.is_error.unwrap_or(false)
                )
            }
            RuntimeEvent::Usage(_) => "usage".into(),
            RuntimeEvent::CommandResult(result) => format!("command_result({})", result.command),
            RuntimeEvent::InterventionRequest(InterventionRequest::Approval(request)) => {
                format!("approval({})", request.tool_name)
            }
            RuntimeEvent::InterventionRequest(InterventionRequest::Question(_)) => {
                "question".into()
            }
            RuntimeEvent::TurnCompleted(tc) => format!("turn_completed({})", tc.turn_id),
            RuntimeEvent::TurnFailed(tf) => format!("turn_failed({}:{})", tf.code, tf.error),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn live_runtime(provider: &LiveProvider) -> AgentRuntime {
    let runtime = AgentRuntime::new();
    let adapter = provider.adapter();
    runtime.register(Arc::new(
        ProtocolProvider::new(adapter).expect("live protocol provider"),
    ));
    runtime
}

fn live_open_request(
    provider: &LiveProvider,
    temp_root: &Path,
    workdir: &Path,
    input: AgentInput,
) -> Result<OpenSession, String> {
    let extra_env = provider.extra_env(temp_root, workdir)?;
    let extra_args = provider.extra_args(workdir);
    let args = match provider.name() {
        "claude" => json!({
            "system_prompt": "Be concise. When asked to reply with an exact token, output only that token.",
            "extra_env": extra_env,
            "extra_args": extra_args,
        }),
        "codex" | "gemini" | "pi" => json!({
            "extra_env": extra_env,
            "extra_args": extra_args,
        }),
        other => return Err(format!("unsupported live runtime provider {other}")),
    };

    Ok(OpenSession {
        model: Some(provider.model.clone().into()),
        cwd: Some(workdir.to_string_lossy().into_owned().into()),
        initial_input: Some(input),
        idle_timeout_ms: None,
        args,
    })
}

fn live_resume_request(
    provider: &LiveProvider,
    temp_root: &Path,
    workdir: &Path,
    session_ref: &str,
) -> Result<ResumeSession, String> {
    let extra_env = provider.extra_env(temp_root, workdir)?;
    let extra_args = provider.extra_args(workdir);
    let args = match provider.name() {
        "claude" => json!({
            "cwd": workdir,
            "system_prompt": "Be concise. When asked to reply with an exact token, output only that token.",
            "extra_env": extra_env,
            "extra_args": extra_args,
        }),
        "codex" | "gemini" | "pi" => json!({
            "cwd": workdir,
            "extra_env": extra_env,
            "extra_args": extra_args,
        }),
        other => return Err(format!("unsupported live runtime provider {other}")),
    };

    Ok(ResumeSession {
        session_ref: SessionRef(session_ref.into()),
        idle_timeout_ms: None,
        args,
    })
}

fn live_question_response(request: &QuestionRequest) -> QuestionResponse {
    QuestionResponse {
        answers: request
            .questions
            .iter()
            .map(question_answer)
            .collect::<Vec<_>>(),
    }
}

fn question_answer(question: &Question) -> QuestionAnswer {
    if let Some(first) = question.options.first() {
        return QuestionAnswer {
            values: vec![first.label.clone()],
        };
    }
    QuestionAnswer {
        values: vec!["yes".into()],
    }
}

fn live_runtime_timeout(provider: &LiveProvider) -> Duration {
    std::env::var("LUCARNE_LIVE_TIMEOUT")
        .ok()
        .and_then(|raw| parse_duration_env(raw.trim()))
        .filter(|duration| !duration.is_zero())
        .unwrap_or_else(|| provider.timeout())
}

fn should_apply_replay_effects(
    trigger: ReplayEffectsTrigger,
    saw_approval: bool,
    event: &RuntimeEvent,
) -> bool {
    match trigger {
        ReplayEffectsTrigger::OnStreamClose => false,
        ReplayEffectsTrigger::OnAssistantMessageContains(needle) => matches!(
            event,
            RuntimeEvent::Message(message)
                if message.role == crate::agent_runtime::MessageRole::Assistant
                    && message.text.contains(needle)
        ),
        ReplayEffectsTrigger::OnApprovalThenSuccessToolResult => {
            saw_approval
                && matches!(
                    event,
                    RuntimeEvent::ToolResult(tool_result)
                        if tool_result.is_error == Some(false)
                )
        }
    }
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

fn _assert_live_runtime_sync() {
    fn check<T: Send + Sync>() {}
    check::<LiveRuntimeTurnSpec>();
    check::<LiveRuntimeHooks>();
}
