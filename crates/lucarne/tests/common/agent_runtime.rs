use lucarne::adapters::{claude, codex, gemini};
use lucarne::agent_runtime::{
    AgentEventStream, AgentInput, AgentRuntime, AgentSession, Event, OpenSession, ProtocolProvider,
    ResumeSession, SessionRef,
};
use once_cell::sync::OnceCell;
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::time::timeout;

use super::{fakeagent_bin, fixture_path, repo_root};

const EVENT_TIMEOUT: Duration = Duration::from_secs(10);

pub fn runtime() -> AgentRuntime {
    let runtime = AgentRuntime::new();
    runtime.register(Arc::new(
        ProtocolProvider::new(adapter_for("claude")).expect("claude provider"),
    ));
    runtime.register(Arc::new(
        ProtocolProvider::new(adapter_for("codex")).expect("codex provider"),
    ));
    runtime.register(Arc::new(
        ProtocolProvider::new(adapter_for("gemini")).expect("gemini provider"),
    ));
    runtime
}

pub fn open_request(
    provider_id: &'static str,
    fixture: &str,
    initial_input: Option<&str>,
) -> OpenSession {
    OpenSession {
        cwd: Some(repo_root_string().into()),
        initial_input: initial_input.map(|text| AgentInput {
            text: text.into(),
            images: Vec::new(),
        }),
        args: provider_args(provider_id, fixture),
        ..Default::default()
    }
}

pub fn resume_request(
    provider_id: &'static str,
    fixture: &str,
    session_ref: &str,
) -> ResumeSession {
    ResumeSession {
        session_ref: SessionRef(session_ref.into()),
        idle_timeout_ms: None,
        args: resume_args(provider_id, fixture),
    }
}

pub async fn take_events(session: &dyn AgentSession) -> AgentEventStream {
    session.take_events().await.expect("take session events")
}

pub async fn submit_input(session: &dyn AgentSession, text: &str) {
    session
        .submit(AgentInput {
            text: text.into(),
            images: Vec::new(),
        })
        .await
        .expect("submit prompt");
}

pub async fn recv_event(events: &mut AgentEventStream) -> Event {
    timeout(EVENT_TIMEOUT, events.recv())
        .await
        .expect("event timeout")
        .expect("session event")
}

pub async fn collect_until_closed(events: &mut AgentEventStream) -> Vec<Event> {
    let mut out = Vec::new();
    loop {
        match timeout(EVENT_TIMEOUT, events.recv())
            .await
            .expect("event timeout")
        {
            Some(event) => out.push(event),
            None => return out,
        }
    }
}

pub async fn assert_stream_closed(events: &mut AgentEventStream) {
    assert!(
        timeout(EVENT_TIMEOUT, events.recv())
            .await
            .expect("stream close timeout")
            .is_none(),
        "expected event stream to close"
    );
}

pub fn assistant_texts(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Message(message)
                if message.role == lucarne::agent_runtime::MessageRole::Assistant =>
            {
                Some(message.text.to_string())
            }
            _ => None,
        })
        .collect()
}

pub fn reasoning_texts(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Reasoning(reasoning) => Some(reasoning.text.to_string()),
            _ => None,
        })
        .collect()
}

fn provider_args(provider_id: &'static str, fixture: &str) -> serde_json::Value {
    let fixture = fixture_path(provider_id, fixture);
    let extra_env = json!({
        "LUCARNE_FIXTURE": fixture.to_string_lossy(),
    });

    match provider_id {
        "claude" => json!({
            "extra_env": extra_env,
        }),
        "codex" | "gemini" => json!({
            "extra_env": extra_env,
        }),
        _ => panic!("unsupported provider {:?}", provider_id),
    }
}

fn resume_args(provider_id: &'static str, fixture: &str) -> serde_json::Value {
    let fixture = fixture_path(provider_id, fixture);
    let cwd = repo_root_string();
    let extra_env = json!({
        "LUCARNE_FIXTURE": fixture.to_string_lossy(),
    });

    match provider_id {
        "claude" => json!({
            "cwd": cwd,
            "extra_env": extra_env,
        }),
        "codex" | "gemini" => json!({
            "cwd": cwd,
            "extra_env": extra_env,
        }),
        _ => panic!("unsupported provider {:?}", provider_id),
    }
}

fn adapter_for(provider_id: &'static str) -> Arc<lucarne::adapter::ProtocolAdapter> {
    let binary = versioned_fakeagent_bin(provider_id);
    let binary = binary.to_string_lossy().into_owned();
    match provider_id {
        "claude" => claude::new(claude::Options { binary }),
        "codex" => codex::new(codex::Options { binary }),
        "gemini" => gemini::new(gemini::Options { binary }),
        _ => panic!("unsupported provider {:?}", provider_id),
    }
}

fn repo_root_string() -> String {
    repo_root().to_string_lossy().into_owned()
}

fn versioned_fakeagent_bin(provider_id: &'static str) -> PathBuf {
    static CLAUDE: OnceCell<PathBuf> = OnceCell::new();
    static CODEX: OnceCell<PathBuf> = OnceCell::new();
    static GEMINI: OnceCell<PathBuf> = OnceCell::new();

    match provider_id {
        "claude" => CLAUDE
            .get_or_init(|| write_version_wrapper("claude"))
            .clone(),
        "codex" => CODEX.get_or_init(|| write_version_wrapper("codex")).clone(),
        "gemini" => GEMINI
            .get_or_init(|| write_version_wrapper("gemini"))
            .clone(),
        _ => panic!("unsupported provider {:?}", provider_id),
    }
}

fn write_version_wrapper(provider_id: &'static str) -> PathBuf {
    let dir = tempfile::tempdir().expect("tempdir");
    #[cfg(unix)]
    let path = dir.path().join(format!("{provider_id}-fakeagent"));
    #[cfg(windows)]
    let path = dir.path().join(format!("{provider_id}-fakeagent.exe"));
    let fakeagent = fakeagent_bin();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&fakeagent, &path).expect("symlink fakeagent");
    #[cfg(windows)]
    std::fs::copy(&fakeagent, &path).expect("copy fakeagent");
    std::mem::forget(dir);
    path
}
