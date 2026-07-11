//! Real **Grok ACP dialect** user journeys with **mocked channels only**.
//!
//! Stack under test:
//! - `adapters::grok` + `ProtocolProvider` + `lucarne-fakeagent` fixtures
//! - `LucarneCore` resume / submit / event pump (same path Telegram & WeChat use)
//! - Message binding APIs (channel-agnostic; what mock TG/WeChat call)
//!
//! Not under test: real Telegram Bot API / WeChat iLink (mocked via bindings only).

pub mod common;

use common::{fakeagent_bin, fixture_path};
use lucarne::{
    adapters::grok,
    agent_runtime::{AgentInput, AgentRuntime, Event, MessageRole, ProtocolProvider},
    control_plane::{ControlPlaneSqliteStore, ProviderSessionId, TurnSource, WorkspaceId},
    core_service::{
        CoreEvent, LucarneCore, OpenWorkspaceRequest, ResumeWorkspaceRequest, SubmitTurnRequest,
    },
};
use std::{
    ffi::OsString,
    sync::{Mutex, MutexGuard},
    time::Duration,
};
use std::sync::Arc;
use tokio::time::timeout;

const MAX_EVENTS_AFTER_RESUME_TURN: usize = 64;

static FIXTURE_ENV_LOCK: Mutex<()> = Mutex::new(());

struct GrokFloodFixtureEnv {
    _guard: MutexGuard<'static, ()>,
    prev_fixture: Option<OsString>,
}

impl GrokFloodFixtureEnv {
    fn install() -> Self {
        let guard = FIXTURE_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let fixture = fixture_path("grok", "resume_with_replay_flood.fixture");
        let prev_fixture = std::env::var_os("LUCARNE_FIXTURE");
        // Inherited by ProtocolAdapter child (fakeagent) via launcher apply_env.
        // SAFETY: test process is serialized by FIXTURE_ENV_LOCK for this env var.
        unsafe {
            std::env::set_var("LUCARNE_FIXTURE", &fixture);
        }
        Self {
            _guard: guard,
            prev_fixture,
        }
    }
}

impl Drop for GrokFloodFixtureEnv {
    fn drop(&mut self) {
        unsafe {
            match &self.prev_fixture {
                Some(v) => std::env::set_var("LUCARNE_FIXTURE", v),
                None => std::env::remove_var("LUCARNE_FIXTURE"),
            }
        }
    }
}

fn core_with_real_grok() -> Arc<LucarneCore> {
    let adapter = grok::new(grok::Options {
        binary: fakeagent_bin().to_string_lossy().into_owned(),
    });
    let runtime = Arc::new(AgentRuntime::new());
    runtime.register(Arc::new(
        ProtocolProvider::new(adapter).expect("grok ProtocolProvider"),
    ));
    LucarneCore::from_runtime_and_store(
        runtime,
        ControlPlaneSqliteStore::open_in_memory().expect("store"),
    )
    .expect("core")
}

/// Full product path without network:
/// bind notif (TG+WX) → resolve → Core `resume_workspace` (real Grok dialect +
/// isReplay flood fixture) → `submit_turn` → bounded live completion.
#[tokio::test]
async fn channel_notification_continue_uses_real_grok_resume_without_replay_flood() {
    let _fixture = GrokFloodFixtureEnv::install();
    let core = core_with_real_grok();
    let _global = core.watch_events(); // subscriber required for some paths

    let workspace_id = WorkspaceId::new("grok:resume:channel-journey");
    core.upsert_workspace_binding(
        workspace_id.clone(),
        OpenWorkspaceRequest {
            provider_id: "grok",
            project_path: Some("/tmp/grok-channel-journey".into()),
            title: "grok channel journey".into(),
        },
        Some("uuid-resume-flood"),
    )
    .expect("workspace + provider session");

    let provider_session_id = ProviderSessionId::new("grok:uuid-resume-flood");
    // Telegram notification topic message id
    core.bind_message_to_provider_session(
        "telegram",
        "chat-100",
        "tg-notif-1",
        provider_session_id.clone(),
    )
    .expect("tg bind");
    // WeChat outbound message id + quote-text hash of a typical body
    core.bind_message_to_provider_session(
        "wechat",
        "user-1",
        "wx-notif-1",
        provider_session_id.clone(),
    )
    .expect("wx bind");
    let wx_body = "🤖 grok\nagent done\n\n会话：uuid-resume-flood\n目录：/tmp/grok-channel-journey";
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in wx_body.trim().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let wx_quote = format!("quote:{hash:016x}");
    core.bind_message_to_provider_session(
        "wechat",
        "user-1",
        &wx_quote,
        provider_session_id.clone(),
    )
    .expect("wx quote bind");

    // Channel resolve step (must not be 无法路由 / no longer routable)
    assert_eq!(
        core.message_session_binding("telegram", "chat-100", "tg-notif-1")
            .map(|b| b.provider_session_id),
        Some(provider_session_id.clone())
    );
    assert_eq!(
        core.message_session_binding("wechat", "user-1", "wx-notif-1")
            .map(|b| b.provider_session_id),
        Some(provider_session_id.clone())
    );
    assert_eq!(
        core.message_session_binding("wechat", "user-1", &wx_quote)
            .map(|b| b.provider_session_id),
        Some(provider_session_id.clone())
    );

    // Subscribe before resume so we observe the same CoreEvent bus channels use.
    let mut core_events = core.watch_events();

    // Real Grok dialect resume (session/load + ~480 isReplay updates in fixture).
    // LUCARNE_FIXTURE is inherited by the fakeagent child process.
    core.resume_workspace_with_events(ResumeWorkspaceRequest {
        workspace_id: workspace_id.clone(),
        force_bypass_permissions: true,
    })
    .await
    .expect("resume real grok with flood fixture via process LUCARNE_FIXTURE");

    // User continues (same as TG notification reply / WeChat quote reply submit).
    let submitted = core
        .submit_turn(SubmitTurnRequest {
            workspace_id: workspace_id.clone(),
            source: TurnSource::UserMessage,
            input: AgentInput {
                text: "continue after load".into(),
                images: Vec::new(),
            },
            reply_to_channel_message_id: None,
        })
        .await
        .expect("submit continue");

    let mut collected: Vec<Event> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut saw_completed_for_turn = false;
    loop {
        if tokio::time::Instant::now() > deadline {
            break;
        }
        match timeout(Duration::from_millis(900), core_events.recv()).await {
            Ok(Ok(CoreEvent::TimelineEvent {
                workspace_id: ws,
                event,
                ..
            })) if ws == workspace_id => {
                if matches!(&event, Event::TurnCompleted(_)) {
                    saw_completed_for_turn = true;
                }
                collected.push(event);
                if saw_completed_for_turn {
                    while let Ok(Ok(CoreEvent::TimelineEvent {
                        workspace_id: ws,
                        event,
                        ..
                    })) = timeout(Duration::from_millis(120), core_events.recv()).await
                    {
                        if ws == workspace_id {
                            collected.push(event);
                        }
                    }
                    break;
                }
            }
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => break,
            Err(_) => {
                if saw_completed_for_turn || !collected.is_empty() {
                    break;
                }
            }
        }
    }

    assert!(
        collected.len() <= MAX_EVENTS_AFTER_RESUME_TURN,
        "real Grok resume+turn flooded core bus for workspace: {} events (limit {MAX_EVENTS_AFTER_RESUME_TURN}); first={:?}",
        collected.len(),
        collected.iter().take(12).collect::<Vec<_>>()
    );
    let assistant: String = collected
        .iter()
        .filter_map(|e| match e {
            Event::Message(m) if m.role == MessageRole::Assistant => Some(m.text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        assistant.contains("LIVE_AFTER_REPLAY_OK"),
        "live turn missing; turn={:?}; events={collected:?}",
        submitted.turn_id
    );
    assert!(
        !assistant.contains("replay-assistant-") && !assistant.contains("replay-user-"),
        "isReplay history leaked into channel event stream"
    );
    assert!(
        saw_completed_for_turn,
        "turn must complete (no lag/SIGKILL path)"
    );

    // Bindings still valid for the next quote/reply.
    assert!(
        core.message_session_binding("telegram", "chat-100", "tg-notif-1")
            .is_some()
    );
    assert!(
        core.message_session_binding("wechat", "user-1", "wx-notif-1")
            .is_some()
    );
}

#[tokio::test]
async fn unbound_channel_message_is_not_routable() {
    let core = core_with_real_grok();
    assert!(
        core.message_session_binding("telegram", "chat-100", "missing")
            .is_none()
    );
    assert!(
        core.message_session_binding("wechat", "user-1", "missing")
            .is_none()
    );
}
