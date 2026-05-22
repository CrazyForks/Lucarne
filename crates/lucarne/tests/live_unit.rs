mod live;

use base64::Engine;
use live::{
    apply_side_effects, assistant_transcript, claude_allowed_dirs, compile_fixture_script,
    configured_live_providers, diff_side_effects, find_events_by_kind, live_delete_prompt,
    live_failure_prompt, live_provider_by_name, live_question_prompt, live_tool_prompt,
    preflight_live_provider_with_timeout, recorded_provider_or_return, select_recording_mode,
    snapshot_workdir, summarize_live_events, CapturedLine, ProviderKind, RecordedLiveCase,
    RecordingMode,
};
use once_cell::sync::Lazy;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

static ENV_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[test]
fn provider_filtering_respects_lucarne_live_providers() {
    let _guard = ENV_LOCK.lock().unwrap();
    let old = std::env::var("LUCARNE_LIVE_PROVIDERS").ok();
    std::env::set_var("LUCARNE_LIVE_PROVIDERS", "codex,gemini,pi");
    let providers = configured_live_providers();
    if let Some(old) = old {
        std::env::set_var("LUCARNE_LIVE_PROVIDERS", old);
    } else {
        std::env::remove_var("LUCARNE_LIVE_PROVIDERS");
    }
    let kinds: Vec<_> = providers.iter().map(|provider| provider.kind).collect();
    assert_eq!(
        kinds,
        vec![ProviderKind::Codex, ProviderKind::Gemini, ProviderKind::Pi]
    );
}

#[test]
fn claude_tool_prompt_keeps_write_tool_constraint() {
    let prompt = live_tool_prompt(
        "claude",
        Path::new("/tmp/workdir"),
        Path::new("/tmp/workdir/README.md"),
        Path::new("/tmp/workdir/live-output.txt"),
    );
    assert!(prompt.contains("Write tool"));
    assert!(prompt.contains("Do not switch to Bash"));
    assert!(prompt.contains("/tmp/workdir/README.md"));
    assert!(prompt.contains("/tmp/workdir/live-output.txt"));
    assert!(!prompt.contains("./README.md"));
    assert!(!prompt.contains("./live-output.txt"));
}

#[test]
fn pi_tool_prompt_uses_default_relative_paths() {
    let prompt = live_tool_prompt(
        "pi",
        Path::new("/tmp/workdir"),
        Path::new("/tmp/workdir/README.md"),
        Path::new("/tmp/workdir/live-output.txt"),
    );
    assert!(prompt.contains("Read ./README.md"));
    assert!(prompt.contains("create ./live-output.txt"));
    assert!(prompt.contains("TOOL_OK"));
    assert!(!prompt.contains("Write tool"));
    assert!(!prompt.contains("/tmp/workdir/README.md"));
}

#[test]
fn codex_delete_prompt_uses_tool_execution_not_chat_approval() {
    let prompt = live_delete_prompt(
        "codex",
        Path::new("/tmp/workdir"),
        Path::new("/tmp/workdir/delete-target.txt"),
    );
    assert!(prompt.contains("Use a shell command to delete"));
    assert!(prompt.contains("Do not ask a natural-language approval question first"));
    assert!(!prompt.contains("Ask for approval"));
    assert!(prompt.contains("DELETE_OK"));
    assert!(prompt.contains("./delete-target.txt"));
    assert!(!prompt.contains("/tmp/workdir/delete-target.txt"));
}

#[test]
fn pi_delete_prompt_uses_default_shell_language() {
    let prompt = live_delete_prompt(
        "pi",
        Path::new("/tmp/workdir"),
        Path::new("/tmp/workdir/delete-target.txt"),
    );
    assert!(prompt.contains("Use a shell or terminal command to delete"));
    assert!(prompt.contains("DELETE_OK"));
    assert!(prompt.contains("./delete-target.txt"));
    assert!(!prompt.contains("Do not ask a natural-language approval question first"));
    assert!(!prompt.contains("/tmp/workdir/delete-target.txt"));
}

#[test]
fn pi_live_provider_defaults_to_deepseek_flash() {
    let _guard = ENV_LOCK.lock().unwrap();
    let old = std::env::var("LUCARNE_LIVE_PI_MODEL").ok();
    std::env::remove_var("LUCARNE_LIVE_PI_MODEL");

    let pi = live_provider_by_name("pi").expect("live provider");

    if let Some(old) = old {
        std::env::set_var("LUCARNE_LIVE_PI_MODEL", old);
    } else {
        std::env::remove_var("LUCARNE_LIVE_PI_MODEL");
    }

    assert_eq!(pi.model, "deepseek/deepseek-v4-flash");
}

#[test]
fn question_prompts_target_provider_native_question_tools() {
    assert!(live_question_prompt("claude").contains("AskUserQuestion"));
    assert!(live_question_prompt("codex").contains("Which response style should I use next?"));
    assert!(live_question_prompt("codex").contains("QUESTION_OK"));
    assert!(live_question_prompt("gemini").contains("Which response style should I use next?"));
    assert!(live_question_prompt("gemini").contains("QUESTION_OK"));
    assert!(live_question_prompt("pi").contains("Which response style should I use next?"));
    assert!(live_question_prompt("pi").contains("QUESTION_OK"));
}

#[test]
fn failure_prompts_keep_provider_specific_tool_language() {
    assert!(live_failure_prompt("claude").contains("Use Bash"));
    assert!(live_failure_prompt("codex").contains("Use a shell command"));
    assert!(live_failure_prompt("gemini").contains("Do not use any shell or terminal command"));
    assert!(live_failure_prompt("gemini").contains("available file-reading tool"));
}

#[test]
fn claude_allowed_dirs_dedups_clean_and_canonical() {
    let temp = tempfile::tempdir().unwrap();
    let dirs = claude_allowed_dirs(temp.path());
    assert!(!dirs.is_empty());
    assert!(dirs.len() <= 2);
    assert_eq!(dirs[0], temp.path().to_string_lossy());
}

#[test]
fn helper_exports_work_for_empty_inputs() {
    assert_eq!(assistant_transcript(&[]), "");
    assert_eq!(summarize_live_events(&[]), "<none>");
    assert!(find_events_by_kind(&[], lucarne::event::Kind::TurnFailed).is_empty());
}

#[test]
fn live_providers_do_not_advertise_permission_intercept() {
    for name in ["claude", "codex", "gemini"] {
        let provider = live_provider_by_name(name).expect("live provider");
        assert!(
            !provider.adapter().spec().capabilities.permission_intercept,
            "{name} should use provider-native permissions without interception"
        );
    }

    let pi = live_provider_by_name("pi").expect("live provider");
    assert!(
        pi.adapter().spec().capabilities.permission_intercept,
        "pi uses RPC extension_ui_request interception for permissions"
    );
}

#[test]
fn codex_live_provider_does_not_override_codex_home() {
    let provider = live_provider_by_name("codex").expect("live provider");
    let temp = tempfile::tempdir().unwrap();
    let env = provider
        .extra_env(
            temp.path(),
            Path::new("/Volumes/Data/opensource/conductor/lucarnex"),
        )
        .expect("extra env");
    assert!(
        !env.contains_key("CODEX_HOME"),
        "live harness should not rewrite provider-owned CODEX_HOME"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn gemini_live_preflight_accepts_initialize_response() {
    let temp = tempfile::tempdir().unwrap();
    let script = temp.path().join("fake-gemini-ok.sh");
    std::fs::write(
        &script,
        concat!(
            "#!/usr/bin/env bash\n",
            "set -euo pipefail\n",
            "printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":1}}'\n",
            "sleep 30\n",
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let provider = live_provider_by_name("gemini")
        .expect("live provider")
        .with_binary(script.to_string_lossy());
    preflight_live_provider_with_timeout(
        &provider,
        temp.path(),
        temp.path(),
        Duration::from_secs(2),
    )
    .await
    .expect("preflight should accept a valid initialize response");
}

#[cfg(unix)]
#[tokio::test]
async fn codex_live_preflight_accepts_minimal_turn() {
    let temp = tempfile::tempdir().unwrap();
    let script = temp.path().join("fake-codex-ok.sh");
    std::fs::write(
        &script,
        concat!(
            "#!/usr/bin/env bash\n",
            "set -euo pipefail\n",
            "saw_initialize=0\n",
            "saw_initialized=0\n",
            "while IFS= read -r line; do\n",
            "  case \"$line\" in\n",
            "    *'\"method\":\"initialize\"'*)\n",
            "      saw_initialize=1\n",
            "      printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":1}}'\n",
            "      ;;\n",
            "    *'\"method\":\"initialized\"'*)\n",
            "      saw_initialized=1\n",
            "      ;;\n",
            "    *'\"method\":\"thread/start\"'*)\n",
            "      if [ \"$saw_initialize\" -ne 1 ] || [ \"$saw_initialized\" -ne 1 ]; then\n",
            "        printf '%s\\n' '{\"method\":\"error\",\"params\":{\"error\":{\"message\":\"thread/start before initialized\"}}}'\n",
            "        sleep 30\n",
            "        exit 0\n",
            "      fi\n",
            "      printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"thread\":{\"id\":\"thr-test\"}}}'\n",
            "      ;;\n",
            "    *'\"method\":\"turn/start\"'*)\n",
            "      printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"turn\":{\"id\":\"turn-test\",\"status\":\"inProgress\",\"items\":[],\"error\":null}}}'\n",
            "      printf '%s\\n' '{\"method\":\"item/completed\",\"params\":{\"item\":{\"type\":\"agentMessage\",\"text\":\"LIVE_OK\"}}}'\n",
            "      sleep 30\n",
            "      exit 0\n",
            "      ;;\n",
            "  esac\n",
            "done\n",
            "sleep 30\n",
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let provider = live_provider_by_name("codex")
        .expect("live provider")
        .with_binary(script.to_string_lossy());
    preflight_live_provider_with_timeout(
        &provider,
        temp.path(),
        temp.path(),
        Duration::from_secs(2),
    )
    .await
    .expect("preflight should accept a minimal codex turn");
}

#[cfg(unix)]
#[tokio::test]
async fn codex_live_preflight_reports_stream_disconnect() {
    let temp = tempfile::tempdir().unwrap();
    let script = temp.path().join("fake-codex-disconnect.sh");
    std::fs::write(
        &script,
        concat!(
            "#!/usr/bin/env bash\n",
            "set -euo pipefail\n",
            "printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":1}}'\n",
            "printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"thread\":{\"id\":\"thr-test\"}}}'\n",
            "printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"turn\":{\"id\":\"turn-test\",\"status\":\"inProgress\",\"items\":[],\"error\":null}}}'\n",
            "printf '%s\\n' '{\"method\":\"error\",\"params\":{\"error\":{\"message\":\"Reconnecting... 2/5\",\"codexErrorInfo\":{\"responseStreamDisconnected\":{\"httpStatusCode\":null}}}}}'\n",
            "sleep 30\n",
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let provider = live_provider_by_name("codex")
        .expect("live provider")
        .with_binary(script.to_string_lossy());
    let err = preflight_live_provider_with_timeout(
        &provider,
        temp.path(),
        temp.path(),
        Duration::from_secs(2),
    )
    .await
    .expect_err("preflight should fail fast on codex stream disconnect");
    assert!(err.contains("response stream"), "{err}");
}

#[cfg(unix)]
#[tokio::test]
async fn gemini_live_preflight_times_out_when_initialize_never_arrives() {
    let temp = tempfile::tempdir().unwrap();
    let script = temp.path().join("fake-gemini-hang.sh");
    std::fs::write(
        &script,
        concat!("#!/usr/bin/env bash\n", "set -euo pipefail\n", "sleep 30\n",),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let provider = live_provider_by_name("gemini")
        .expect("live provider")
        .with_binary(script.to_string_lossy());
    let err = preflight_live_provider_with_timeout(
        &provider,
        temp.path(),
        temp.path(),
        Duration::from_millis(150),
    )
    .await
    .expect_err("preflight should fail fast when initialize never arrives");
    assert!(
        err.contains("did not answer initialize within 150ms"),
        "{err}"
    );
}

#[test]
fn recording_mode_prefers_replay_before_live() {
    assert_eq!(
        select_recording_mode(true, false, false),
        RecordingMode::Replay
    );
    assert_eq!(
        select_recording_mode(true, true, false),
        RecordingMode::Replay
    );
    assert_eq!(
        select_recording_mode(true, true, true),
        RecordingMode::LiveRecord
    );
    assert_eq!(
        select_recording_mode(false, true, false),
        RecordingMode::LiveRecord
    );
    assert_eq!(
        select_recording_mode(false, false, false),
        RecordingMode::Unavailable
    );
}

#[test]
fn compile_fixture_script_interleaves_expected_inputs_and_outputs() {
    let fixture = compile_fixture_script(
        "codex",
        &[
            CapturedLine {
                ts_nanos: 10,
                line: r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}"#.into(),
            },
            CapturedLine {
                ts_nanos: 30,
                line: r#"{"jsonrpc":"2.0","id":1,"method":"turn/start","params":{"prompt":"Reply with exactly LIVE_OK and nothing else."}}"#.into(),
            },
        ],
        &[
            CapturedLine {
                ts_nanos: 20,
                line: r#"{"jsonrpc":"2.0","id":0,"result":{}}"#.into(),
            },
            CapturedLine {
                ts_nanos: 40,
                line: r#"{"jsonrpc":"2.0","method":"item/completed","params":{"item":{"type":"agent_message","text":"LIVE_OK"}}}"#.into(),
            },
        ],
        &[],
        &[],
        0,
    )
    .expect("compile fixture");

    assert!(fixture.contains(r#"EXPECT_IN_CONTAINS_NEXT "\"method\":\"initialize\"""#));
    assert!(fixture.contains(r#"EXPECT_IN_CONTAINS_NEXT "\"method\":\"turn/start\"""#));
    assert!(fixture.contains(r#"OUT {"jsonrpc":"2.0","id":0,"result":{}}"#));
    assert!(fixture.contains(
        r#"OUT {"jsonrpc":"2.0","method":"item/completed","params":{"item":{"type":"agent_message","text":"LIVE_OK"}}}"#
    ));
}

#[test]
fn compile_fixture_script_keeps_prompt_semantics() {
    let fixture = compile_fixture_script(
        "codex",
        &[CapturedLine {
            ts_nanos: 10,
            line: r#"{"jsonrpc":"2.0","id":2,"method":"turn/start","params":{"prompt":"Use tools. Read /tmp/lucarne-live/README.md and write /tmp/lucarne-live/live-output.txt before replying TOOL_OK."}}"#.into(),
        }],
        &[],
        &[],
        &[],
        0,
    )
    .expect("compile fixture");

    assert!(fixture.contains("TOOL_OK"));
    assert!(fixture.contains("README.md"));
    assert!(fixture.contains("live-output.txt"));
}

#[test]
fn compile_fixture_script_keeps_resume_and_approval_payloads() {
    let fixture = compile_fixture_script(
        "codex",
        &[
            CapturedLine {
                ts_nanos: 10,
                line: r#"{"jsonrpc":"2.0","id":7,"method":"session/load","params":{"sessionId":"sess-previous"}}"#.into(),
            },
            CapturedLine {
                ts_nanos: 20,
                line: r#"{"jsonrpc":"2.0","id":8,"method":"approval/respond","params":{"decision":{"behavior":"allow"},"requestId":"req-42"}}"#.into(),
            },
        ],
        &[],
        &[],
        &[],
        0,
    )
    .expect("compile fixture");

    assert!(fixture.contains(r#"\"sessionId\":\"sess-previous\""#));
    assert!(fixture.contains(r#"\"behavior\":\"allow\""#));
    assert!(fixture.contains(r#"\"id\":7"#));
    assert!(fixture.contains(r#"\"id\":8"#));
}

#[test]
fn compile_fixture_script_preserves_signal_expectations() {
    let fixture = compile_fixture_script(
        "codex",
        &[],
        &[],
        &[],
        &[CapturedLine {
            ts_nanos: 10,
            line: "SIGINT".into(),
        }],
        130,
    )
    .expect("compile fixture");

    assert!(fixture.contains("EXPECT_SIGNAL_NEXT SIGINT"));
    assert!(fixture.contains("EXIT 130"));
}

#[test]
fn replay_provider_lookup_ignores_filter_when_fixture_exists() {
    let _guard = ENV_LOCK.lock().unwrap();
    let old_live = std::env::var("LUCARNE_LIVE_E2E").ok();
    let old_rerecord = std::env::var("LUCARNE_LIVE_RERECORD").ok();
    let old_filter = std::env::var("LUCARNE_LIVE_PROVIDERS").ok();

    std::env::remove_var("LUCARNE_LIVE_E2E");
    std::env::remove_var("LUCARNE_LIVE_RERECORD");
    std::env::set_var("LUCARNE_LIVE_PROVIDERS", "claude");

    let provider = recorded_provider_or_return(
        "codex",
        RecordedLiveCase {
            suite: "live_e2e",
            case_id: "basic_conversation_codex",
        },
    );

    if let Some(old) = old_live {
        std::env::set_var("LUCARNE_LIVE_E2E", old);
    } else {
        std::env::remove_var("LUCARNE_LIVE_E2E");
    }
    if let Some(old) = old_rerecord {
        std::env::set_var("LUCARNE_LIVE_RERECORD", old);
    } else {
        std::env::remove_var("LUCARNE_LIVE_RERECORD");
    }
    if let Some(old) = old_filter {
        std::env::set_var("LUCARNE_LIVE_PROVIDERS", old);
    } else {
        std::env::remove_var("LUCARNE_LIVE_PROVIDERS");
    }

    assert!(
        provider.is_some(),
        "existing replay fixture should not depend on live provider filters"
    );
}

#[test]
fn replay_provider_lookup_panics_when_fixture_is_missing() {
    let _guard = ENV_LOCK.lock().unwrap();
    let old_live = std::env::var("LUCARNE_LIVE_E2E").ok();
    let old_rerecord = std::env::var("LUCARNE_LIVE_RERECORD").ok();

    std::env::remove_var("LUCARNE_LIVE_E2E");
    std::env::remove_var("LUCARNE_LIVE_RERECORD");

    let panic = std::panic::catch_unwind(|| {
        let _ = recorded_provider_or_return(
            "claude",
            RecordedLiveCase {
                suite: "live_e2e",
                case_id: "definitely-missing-recording",
            },
        );
    });

    if let Some(old) = old_live {
        std::env::set_var("LUCARNE_LIVE_E2E", old);
    } else {
        std::env::remove_var("LUCARNE_LIVE_E2E");
    }
    if let Some(old) = old_rerecord {
        std::env::set_var("LUCARNE_LIVE_RERECORD", old);
    } else {
        std::env::remove_var("LUCARNE_LIVE_RERECORD");
    }

    let payload = panic.expect_err("missing fixture should panic");
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&'static str>()
                .map(|msg| (*msg).to_string())
        })
        .unwrap_or_else(|| format!("{payload:?}"));
    assert!(message.contains("missing live recording bundle"));
}

#[test]
fn side_effect_manifest_replays_recorded_workdir_changes() {
    let before = tempfile::tempdir().unwrap();
    std::fs::write(before.path().join("README.md"), "before\n").unwrap();
    std::fs::write(before.path().join("delete-target.txt"), "delete me\n").unwrap();

    let baseline = snapshot_workdir(before.path()).expect("snapshot before");

    std::fs::write(before.path().join("README.md"), "after\n").unwrap();
    std::fs::write(before.path().join("live-output.txt"), "lucarne-live-tool\n").unwrap();
    std::fs::remove_file(before.path().join("delete-target.txt")).unwrap();

    let updated = snapshot_workdir(before.path()).expect("snapshot after");
    let manifest = diff_side_effects(&baseline, &updated).expect("diff side effects");

    let replay = tempfile::tempdir().unwrap();
    std::fs::write(replay.path().join("README.md"), "before\n").unwrap();
    std::fs::write(replay.path().join("delete-target.txt"), "delete me\n").unwrap();

    apply_side_effects(replay.path(), &manifest).expect("apply side effects");

    assert_eq!(
        std::fs::read_to_string(replay.path().join("README.md")).unwrap(),
        "after\n"
    );
    assert_eq!(
        std::fs::read_to_string(replay.path().join("live-output.txt")).unwrap(),
        "lucarne-live-tool\n"
    );
    assert!(!replay.path().join("delete-target.txt").exists());

    let expected_live_output =
        base64::engine::general_purpose::STANDARD.encode("lucarne-live-tool\n");
    assert!(manifest
        .writes
        .iter()
        .any(|write| write.path == "live-output.txt"
            && write.contents_base64 == expected_live_output));
    assert!(manifest
        .deletes
        .iter()
        .any(|path| path == "delete-target.txt"));
}

#[test]
fn side_effect_manifest_keeps_codex_credentials_out_of_recordings() {
    let workdir = tempfile::tempdir().unwrap();
    let codex_home = workdir.path().join(".codex-home");
    std::fs::create_dir_all(codex_home.join("sessions/2026/05/13")).unwrap();
    std::fs::write(codex_home.join("auth.json"), r#"{"token":"secret"}"#).unwrap();
    std::fs::write(codex_home.join("config.toml"), "model = \"gpt-test\"\n").unwrap();

    let baseline = snapshot_workdir(workdir.path()).expect("snapshot before");
    std::fs::write(codex_home.join("auth.json"), r#"{"token":"changed"}"#).unwrap();
    std::fs::write(
        codex_home.join("sessions/2026/05/13/rollout-test.jsonl"),
        r#"{"type":"session_meta","payload":{"id":"s","base_instructions":{"text":"secret system prompt"}}}"#.to_string()
            + "\n"
            + r#"{"type":"response_item","payload":{"type":"reasoning","encrypted_content":"opaque"}}"#
            + "\n",
    )
    .unwrap();

    let updated = snapshot_workdir(workdir.path()).expect("snapshot after");
    let manifest = diff_side_effects(&baseline, &updated).expect("diff side effects");

    assert!(manifest
        .writes
        .iter()
        .any(|write| { write.path == ".codex-home/sessions/2026/05/13/rollout-test.jsonl" }));
    assert!(
        manifest
            .writes
            .iter()
            .all(|write| !write.path.starts_with(".codex-home/auth.json")
                && !write.path.starts_with(".codex-home/config.toml")),
        "credential/config files must not enter recording effects: {manifest:?}"
    );
    let recorded_session = manifest
        .writes
        .iter()
        .find(|write| write.path == ".codex-home/sessions/2026/05/13/rollout-test.jsonl")
        .map(|write| {
            base64::engine::general_purpose::STANDARD
                .decode(&write.contents_base64)
                .expect("decode recorded session")
        })
        .expect("recorded session write");
    let recorded_session = String::from_utf8(recorded_session).expect("utf8 recorded session");
    assert!(!recorded_session.contains("base_instructions"));
    assert!(!recorded_session.contains("encrypted_content"));
}
