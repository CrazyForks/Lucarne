mod live;

use live::{
    assistant_transcript, collect_timelines, contains_string, failed_messages, find_events_by_kind,
    live_delete_prompt, live_failure_prompt, live_tool_prompt,
    recorded_provider_or_return as replay_provider_or_return, run_live_turn,
    run_live_turn_with_hooks, turn_completed, turn_failed, LiveProvider, LiveTurnHooks,
    LiveTurnSpec, RecordedLiveCase,
};
use lucarne::event::{Decision, Kind, Payload, PermissionResponse, TimelineType};
use std::fs;
use std::sync::Arc;

fn recorded_provider_or_return(
    name: &str,
    suite: &'static str,
    case_id: &'static str,
) -> Option<LiveProvider> {
    replay_provider_or_return(name, RecordedLiveCase { suite, case_id })
}

fn workspace_with_readme(contents: &str) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("README.md"), contents).unwrap();
    temp
}

#[tokio::test]
async fn basic_conversation_claude() {
    let Some(provider) =
        recorded_provider_or_return("claude", "live_e2e", "basic_conversation_claude")
    else {
        return;
    };
    let temp = workspace_with_readme("live basic workspace\n");

    let res = run_live_turn(
        LiveTurnSpec::new(
            provider,
            temp.path().to_path_buf(),
            "Reply with exactly LIVE_OK and nothing else.",
        )
        .recorded("live_e2e", "basic_conversation_claude"),
    )
    .await
    .unwrap();

    let messages = collect_timelines(&res.events, TimelineType::AssistantMessage);
    assert!(
        !messages.is_empty(),
        "expected assistant message; events: {}",
        live::summarize_live_events(&res.events)
    );
    let transcript = assistant_transcript(&messages);
    assert!(
        transcript.contains("LIVE_OK"),
        "unexpected assistant reply: {:?}; events: {}",
        transcript,
        live::summarize_live_events(&res.events)
    );
    assert!(
        failed_messages(&res.events).is_empty(),
        "unexpected failures: {:?}; events: {}",
        failed_messages(&res.events),
        live::summarize_live_events(&res.events)
    );
}

#[tokio::test]
async fn basic_conversation_codex() {
    let Some(provider) =
        recorded_provider_or_return("codex", "live_e2e", "basic_conversation_codex")
    else {
        return;
    };
    let temp = workspace_with_readme("live basic workspace\n");

    let res = run_live_turn(
        LiveTurnSpec::new(
            provider,
            temp.path().to_path_buf(),
            "Reply with exactly LIVE_OK and nothing else.",
        )
        .recorded("live_e2e", "basic_conversation_codex"),
    )
    .await
    .unwrap();

    let messages = collect_timelines(&res.events, TimelineType::AssistantMessage);
    assert!(
        !messages.is_empty(),
        "expected assistant message; events: {}",
        live::summarize_live_events(&res.events)
    );
    let transcript = assistant_transcript(&messages);
    assert!(
        transcript.contains("LIVE_OK"),
        "unexpected assistant reply: {:?}; events: {}",
        transcript,
        live::summarize_live_events(&res.events)
    );
    assert!(
        failed_messages(&res.events).is_empty(),
        "unexpected failures: {:?}; events: {}",
        failed_messages(&res.events),
        live::summarize_live_events(&res.events)
    );
}

#[tokio::test]
async fn basic_conversation_pi() {
    let Some(provider) = recorded_provider_or_return("pi", "live_e2e", "basic_conversation_pi")
    else {
        return;
    };
    let temp = workspace_with_readme("live basic workspace\n");

    let res = run_live_turn(
        LiveTurnSpec::new(
            provider,
            temp.path().to_path_buf(),
            "Reply with exactly LIVE_OK and nothing else.",
        )
        .recorded("live_e2e", "basic_conversation_pi"),
    )
    .await
    .unwrap();

    let messages = collect_timelines(&res.events, TimelineType::AssistantMessage);
    assert!(
        !messages.is_empty(),
        "expected assistant message; events: {}",
        live::summarize_live_events(&res.events)
    );
    let transcript = assistant_transcript(&messages);
    assert!(
        transcript.contains("LIVE_OK"),
        "unexpected assistant reply: {:?}; events: {}",
        transcript,
        live::summarize_live_events(&res.events)
    );
    assert!(
        failed_messages(&res.events).is_empty(),
        "unexpected failures: {:?}; events: {}",
        failed_messages(&res.events),
        live::summarize_live_events(&res.events)
    );
}

#[tokio::test]
async fn tool_flow_codex() {
    let Some(provider) = recorded_provider_or_return("codex", "live_e2e", "tool_flow_codex") else {
        return;
    };
    let temp = workspace_with_readme("live e2e workspace\n");
    let readme_path = temp.path().join("README.md");
    let output_path = temp.path().join("live-output.txt");

    let res = run_live_turn(
        LiveTurnSpec::new(
            provider.clone(),
            temp.path().to_path_buf(),
            live_tool_prompt(provider.name(), temp.path(), &readme_path, &output_path),
        )
        .recorded("live_e2e", "tool_flow_codex"),
    )
    .await
    .unwrap();

    assert!(
        !collect_timelines(&res.events, TimelineType::ToolCall).is_empty(),
        "expected tool call; events: {}",
        live::summarize_live_events(&res.events)
    );
    assert!(
        !collect_timelines(&res.events, TimelineType::ToolResult).is_empty(),
        "expected tool result; events: {}",
        live::summarize_live_events(&res.events)
    );
    let raw = fs::read_to_string(&output_path).unwrap();
    assert_eq!(
        raw.trim(),
        live::expected_live_tool_contents(
            "codex",
            "live_e2e",
            "tool_flow_codex",
            "live-output.txt"
        )
        .trim()
    );
    let transcript = assistant_transcript(&collect_timelines(
        &res.events,
        TimelineType::AssistantMessage,
    ));
    assert!(
        transcript.contains("TOOL_OK"),
        "unexpected assistant reply: {:?}; events: {}",
        transcript,
        live::summarize_live_events(&res.events)
    );
    assert!(
        failed_messages(&res.events).is_empty(),
        "unexpected failures: {:?}; events: {}",
        failed_messages(&res.events),
        live::summarize_live_events(&res.events)
    );
}

#[tokio::test]
async fn tool_flow_gemini() {
    let Some(provider) = recorded_provider_or_return("gemini", "live_e2e", "tool_flow_gemini")
    else {
        return;
    };
    let temp = workspace_with_readme("live e2e workspace\n");
    let readme_path = temp.path().join("README.md");
    let output_path = temp.path().join("live-output.txt");

    let res = run_live_turn(
        LiveTurnSpec::new(
            provider.clone(),
            temp.path().to_path_buf(),
            live_tool_prompt(provider.name(), temp.path(), &readme_path, &output_path),
        )
        .recorded("live_e2e", "tool_flow_gemini"),
    )
    .await
    .unwrap();

    if provider.adapter().spec().capabilities.permission_intercept {
        assert!(
            !find_events_by_kind(&res.events, Kind::PermissionRequest).is_empty(),
            "expected permission request; events: {}",
            live::summarize_live_events(&res.events)
        );
    }
    assert!(
        !collect_timelines(&res.events, TimelineType::ToolCall).is_empty(),
        "expected tool call; events: {}",
        live::summarize_live_events(&res.events)
    );
    assert!(
        !collect_timelines(&res.events, TimelineType::ToolResult).is_empty(),
        "expected tool result; events: {}",
        live::summarize_live_events(&res.events)
    );
    let raw = fs::read_to_string(&output_path).unwrap();
    assert_eq!(
        raw.trim(),
        live::expected_live_tool_contents(
            "gemini",
            "live_e2e",
            "tool_flow_gemini",
            "live-output.txt"
        )
        .trim()
    );
    let transcript = assistant_transcript(&collect_timelines(
        &res.events,
        TimelineType::AssistantMessage,
    ));
    assert!(
        transcript.contains("TOOL_OK"),
        "unexpected assistant reply: {:?}; events: {}",
        transcript,
        live::summarize_live_events(&res.events)
    );
    assert!(
        failed_messages(&res.events).is_empty(),
        "unexpected failures: {:?}; events: {}",
        failed_messages(&res.events),
        live::summarize_live_events(&res.events)
    );
}

#[tokio::test]
async fn tool_flow_pi() {
    let Some(provider) = recorded_provider_or_return("pi", "live_e2e", "tool_flow_pi") else {
        return;
    };
    let temp = workspace_with_readme("live tool workspace\n");
    let readme_path = temp.path().join("README.md");
    let output_path = temp.path().join("live-output.txt");

    let res = run_live_turn(
        LiveTurnSpec::new(
            provider.clone(),
            temp.path().to_path_buf(),
            live_tool_prompt(provider.name(), temp.path(), &readme_path, &output_path),
        )
        .recorded("live_e2e", "tool_flow_pi"),
    )
    .await
    .unwrap();

    assert!(
        !collect_timelines(&res.events, TimelineType::ToolCall).is_empty(),
        "expected tool call; events: {}",
        live::summarize_live_events(&res.events)
    );
    assert!(
        !collect_timelines(&res.events, TimelineType::ToolResult).is_empty(),
        "expected tool result; events: {}",
        live::summarize_live_events(&res.events)
    );
    let raw = fs::read_to_string(&output_path).unwrap();
    assert_eq!(
        raw.trim(),
        live::expected_live_tool_contents("pi", "live_e2e", "tool_flow_pi", "live-output.txt")
            .trim()
    );
    let transcript = assistant_transcript(&collect_timelines(
        &res.events,
        TimelineType::AssistantMessage,
    ));
    assert!(
        transcript.contains("TOOL_OK"),
        "unexpected assistant reply: {:?}; events: {}",
        transcript,
        live::summarize_live_events(&res.events)
    );
    assert!(
        failed_messages(&res.events).is_empty(),
        "unexpected failures: {:?}; events: {}",
        failed_messages(&res.events),
        live::summarize_live_events(&res.events)
    );
}

#[tokio::test]
async fn delete_flow_codex() {
    let Some(provider) = recorded_provider_or_return("codex", "live_e2e", "delete_flow_codex")
    else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let target_path = temp.path().join("delete-target.txt");
    fs::write(&target_path, "delete me\n").unwrap();

    let res = run_live_turn(
        LiveTurnSpec::new(
            provider.clone(),
            temp.path().to_path_buf(),
            live_delete_prompt(provider.name(), temp.path(), &target_path),
        )
        .recorded("live_e2e", "delete_flow_codex"),
    )
    .await
    .unwrap();

    if provider.adapter().spec().capabilities.permission_intercept {
        assert!(
            !find_events_by_kind(&res.events, Kind::PermissionRequest).is_empty(),
            "expected permission request; events: {}",
            live::summarize_live_events(&res.events)
        );
    }
    assert!(
        !collect_timelines(&res.events, TimelineType::ToolCall).is_empty(),
        "expected tool call; events: {}",
        live::summarize_live_events(&res.events)
    );
    assert!(
        !collect_timelines(&res.events, TimelineType::ToolResult).is_empty(),
        "expected tool result; events: {}",
        live::summarize_live_events(&res.events)
    );
    assert!(
        !target_path.exists(),
        "expected delete-target.txt to be deleted; events: {}",
        live::summarize_live_events(&res.events)
    );
    let transcript = assistant_transcript(&collect_timelines(
        &res.events,
        TimelineType::AssistantMessage,
    ));
    assert!(
        transcript.contains("DELETE_OK"),
        "unexpected assistant reply: {:?}; events: {}",
        transcript,
        live::summarize_live_events(&res.events)
    );
    assert!(
        failed_messages(&res.events).is_empty(),
        "unexpected failures: {:?}; events: {}",
        failed_messages(&res.events),
        live::summarize_live_events(&res.events)
    );
}

#[tokio::test]
async fn delete_flow_pi() {
    let Some(provider) = recorded_provider_or_return("pi", "live_e2e", "delete_flow_pi") else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let target_path = temp.path().join("delete-target.txt");
    fs::write(&target_path, "delete me\n").unwrap();

    let res = run_live_turn(
        LiveTurnSpec::new(
            provider.clone(),
            temp.path().to_path_buf(),
            live_delete_prompt(provider.name(), temp.path(), &target_path),
        )
        .recorded("live_e2e", "delete_flow_pi"),
    )
    .await
    .unwrap();

    assert!(
        !collect_timelines(&res.events, TimelineType::ToolCall).is_empty(),
        "expected tool call; events: {}",
        live::summarize_live_events(&res.events)
    );
    assert!(
        !collect_timelines(&res.events, TimelineType::ToolResult).is_empty(),
        "expected tool result; events: {}",
        live::summarize_live_events(&res.events)
    );
    assert!(
        !target_path.exists(),
        "expected delete-target.txt to be deleted; events: {}",
        live::summarize_live_events(&res.events)
    );
    let transcript = assistant_transcript(&collect_timelines(
        &res.events,
        TimelineType::AssistantMessage,
    ));
    assert!(
        transcript.contains("DELETE_OK"),
        "unexpected assistant reply: {:?}; events: {}",
        transcript,
        live::summarize_live_events(&res.events)
    );
    assert!(
        failed_messages(&res.events).is_empty(),
        "unexpected failures: {:?}; events: {}",
        failed_messages(&res.events),
        live::summarize_live_events(&res.events)
    );
}

#[tokio::test]
async fn gemini_rejected_and_cancelled_flow() {
    let Some(provider) =
        recorded_provider_or_return("gemini", "live_e2e", "gemini_rejected_and_cancelled_flow")
    else {
        return;
    };
    let temp = workspace_with_readme("live interrupt workspace\n");

    let res = run_live_turn_with_hooks(
        LiveTurnSpec::new(
            provider,
            temp.path().to_path_buf(),
            "Use tools, do not simulate. Use a shell or terminal command to run `sleep 30` in the current working directory. Do not recover or switch tools.",
        )
        .recorded("live_e2e", "gemini_rejected_and_cancelled_flow"),
        LiveTurnHooks {
            permission_response: Some(Arc::new(|_req| {
                Some(PermissionResponse::from_decision(Decision::Deny))
            })),
            interrupt_on_event: None,
        },
    )
    .await
    .unwrap();

    assert!(
        !find_events_by_kind(&res.events, Kind::PermissionRequest).is_empty(),
        "expected permission request; events: {}",
        live::summarize_live_events(&res.events)
    );
    assert!(turn_completed(&res.events).is_some(), "turn completed");
    let transcript = assistant_transcript(&collect_timelines(
        &res.events,
        TimelineType::AssistantMessage,
    ));
    assert!(
        {
            let lower = transcript.to_lowercase();
            lower.contains("canceled")
                || lower.contains("cancelled")
                || lower.contains("cannot proceed")
        },
        "unexpected assistant reply after reject: {:?}; events: {}",
        transcript,
        live::summarize_live_events(&res.events)
    );
}

#[tokio::test]
async fn gemini_cancelled_flow() {
    let Some(provider) = recorded_provider_or_return("gemini", "live_e2e", "gemini_cancelled_flow")
    else {
        return;
    };
    let temp = workspace_with_readme("live cancel workspace\n");

    let res = run_live_turn_with_hooks(
        LiveTurnSpec::new(
            provider,
            temp.path().to_path_buf(),
            "Use tools, do not simulate. Use a shell or terminal command to run `sleep 30` in the current working directory. Do not recover or switch tools.",
        )
        .recorded("live_e2e", "gemini_cancelled_flow"),
        LiveTurnHooks {
            permission_response: Some(Arc::new(|_req| {
                Some(PermissionResponse::from_decision(Decision::Allow))
            })),
            interrupt_on_event: Some(Arc::new(|event| {
                matches!(
                    &event.payload,
                    Payload::Timeline(timeline)
                        if timeline.item.ty == TimelineType::ToolCall
                            && timeline
                                .item
                                .tool_call
                                .as_ref()
                                .is_some_and(|tool| {
                                    (tool.call.name == "shell"
                                        && tool
                                            .call
                                            .input
                                            .get("command")
                                            .and_then(serde_json::Value::as_str)
                                            .is_some_and(|command| command.contains("sleep 30")))
                                        || (tool.call.name == "execute"
                                            && tool
                                                .call
                                                .input
                                                .get("title")
                                                .and_then(serde_json::Value::as_str)
                                                .is_some_and(|title| title.contains("sleep 30")))
                                })
                )
            })),
        },
    )
    .await
    .unwrap();

    assert!(
        !find_events_by_kind(&res.events, Kind::PermissionRequest).is_empty(),
        "expected permission request; events: {}",
        live::summarize_live_events(&res.events)
    );
    let failed = turn_failed(&res.events).expect("turn failed");
    assert!(
        failed.code == "cancelled" || failed.code == "aborted",
        "unexpected terminal failure after interrupt: {failed:?}"
    );
}

#[tokio::test]
async fn tool_failure_flow_claude() {
    let Some(provider) =
        recorded_provider_or_return("claude", "live_e2e", "tool_failure_flow_claude")
    else {
        return;
    };
    let temp = workspace_with_readme("live failure workspace\n");

    let res = run_live_turn(
        LiveTurnSpec::new(
            provider,
            temp.path().to_path_buf(),
            live_failure_prompt("claude"),
        )
        .recorded("live_e2e", "tool_failure_flow_claude"),
    )
    .await
    .unwrap();

    let results = collect_timelines(&res.events, TimelineType::ToolResult);
    assert!(
        !results.is_empty(),
        "expected tool result; events: {}",
        live::summarize_live_events(&res.events)
    );
    assert!(
        results.iter().any(|item| item
            .tool_result
            .as_ref()
            .is_some_and(|result| !result.result.error.is_empty())),
        "expected at least one error tool result; events: {}",
        live::summarize_live_events(&res.events)
    );
    let transcript = assistant_transcript(&collect_timelines(
        &res.events,
        TimelineType::AssistantMessage,
    ));
    assert!(
        transcript.contains("FAIL_OK"),
        "unexpected assistant reply: {:?}; events: {}",
        transcript,
        live::summarize_live_events(&res.events)
    );
    assert!(
        failed_messages(&res.events).is_empty(),
        "unexpected failures: {:?}; events: {}",
        failed_messages(&res.events),
        live::summarize_live_events(&res.events)
    );
}

#[tokio::test]
async fn tool_failure_flow_codex() {
    let Some(provider) =
        recorded_provider_or_return("codex", "live_e2e", "tool_failure_flow_codex")
    else {
        return;
    };
    let temp = workspace_with_readme("live failure workspace\n");

    let res = run_live_turn(
        LiveTurnSpec::new(
            provider,
            temp.path().to_path_buf(),
            live_failure_prompt("codex"),
        )
        .recorded("live_e2e", "tool_failure_flow_codex"),
    )
    .await
    .unwrap();

    let results = collect_timelines(&res.events, TimelineType::ToolResult);
    assert!(
        !results.is_empty(),
        "expected tool result; events: {}",
        live::summarize_live_events(&res.events)
    );
    assert!(
        results.iter().any(|item| {
            item.tool_result.as_ref().is_some_and(|result| {
                !result.result.error.is_empty()
                    || result.result.output.contains("No such file")
                    || result.result.output.contains("missing-file.txt")
            })
        }),
        "expected at least one error tool result; events: {}",
        live::summarize_live_events(&res.events)
    );
    let transcript = assistant_transcript(&collect_timelines(
        &res.events,
        TimelineType::AssistantMessage,
    ));
    assert!(
        transcript.contains("FAIL_OK"),
        "unexpected assistant reply: {:?}; events: {}",
        transcript,
        live::summarize_live_events(&res.events)
    );
    assert!(
        failed_messages(&res.events).is_empty(),
        "unexpected failures: {:?}; events: {}",
        failed_messages(&res.events),
        live::summarize_live_events(&res.events)
    );
}

#[tokio::test]
async fn tool_failure_flow_pi() {
    let Some(provider) = recorded_provider_or_return("pi", "live_e2e", "tool_failure_flow_pi")
    else {
        return;
    };
    let temp = workspace_with_readme("live pi failure workspace\n");

    let res = run_live_turn(
        LiveTurnSpec::new(
            provider,
            temp.path().to_path_buf(),
            live_failure_prompt("pi"),
        )
        .recorded("live_e2e", "tool_failure_flow_pi"),
    )
    .await
    .unwrap();

    let results = collect_timelines(&res.events, TimelineType::ToolResult);
    assert!(
        !results.is_empty(),
        "expected tool result; events: {}",
        live::summarize_live_events(&res.events)
    );
    assert!(
        results.iter().any(|item| {
            item.tool_result.as_ref().is_some_and(|result| {
                !result.result.error.is_empty()
                    || result.result.output.contains("No such file")
                    || result.result.output.contains("missing")
            })
        }),
        "expected at least one error tool result; events: {}",
        live::summarize_live_events(&res.events)
    );
    let transcript = assistant_transcript(&collect_timelines(
        &res.events,
        TimelineType::AssistantMessage,
    ));
    assert!(
        transcript.contains("FAIL_OK"),
        "unexpected assistant reply: {:?}; events: {}",
        transcript,
        live::summarize_live_events(&res.events)
    );
    assert!(
        failed_messages(&res.events).is_empty(),
        "unexpected failures: {:?}; events: {}",
        failed_messages(&res.events),
        live::summarize_live_events(&res.events)
    );
}

#[tokio::test]
async fn codex_reject_flow() {
    let Some(provider) = recorded_provider_or_return("codex", "live_e2e", "codex_reject_flow")
    else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let target_path = temp.path().join("delete-target.txt");
    fs::write(&target_path, "delete me\n").unwrap();

    let res = run_live_turn_with_hooks(
        LiveTurnSpec::new(
            provider,
            temp.path().to_path_buf(),
            live_delete_prompt("codex", temp.path(), &target_path),
        )
        .recorded("live_e2e", "codex_reject_flow"),
        LiveTurnHooks {
            permission_response: Some(Arc::new(|_req| {
                Some(PermissionResponse::from_decision(Decision::Deny))
            })),
            interrupt_on_event: None,
        },
    )
    .await
    .unwrap();

    assert!(
        !find_events_by_kind(&res.events, Kind::PermissionRequest).is_empty(),
        "expected permission request; events: {}",
        live::summarize_live_events(&res.events)
    );
    let results = collect_timelines(&res.events, TimelineType::ToolResult);
    assert!(
        !results.is_empty(),
        "expected tool result; events: {}",
        live::summarize_live_events(&res.events)
    );
    assert!(target_path.exists(), "expected delete-target.txt to remain");
    let transcript = assistant_transcript(&collect_timelines(
        &res.events,
        TimelineType::AssistantMessage,
    ));
    assert!(
        !transcript.trim().is_empty(),
        "expected assistant acknowledgement; events: {}",
        live::summarize_live_events(&res.events)
    );
    assert!(
        failed_messages(&res.events).is_empty(),
        "unexpected failures: {:?}; events: {}",
        failed_messages(&res.events),
        live::summarize_live_events(&res.events)
    );
}

#[tokio::test]
async fn reject_flow_pi() {
    let Some(provider) = recorded_provider_or_return("pi", "live_e2e", "reject_flow_pi") else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    // Write to a path outside the workspace to trigger Pi permission check.
    let external = tempfile::tempdir().unwrap();
    let external_path = external.path().join("reject-target.txt");

    let res = run_live_turn_with_hooks(
        LiveTurnSpec::new(
            provider,
            temp.path().to_path_buf(),
            format!(
                "Use tools, do not simulate. Write exactly 'reject-test' to {} using the Write tool. Do not use shell or bash. After attempting the write, reply with exactly WRITE_DONE.",
                external_path.display()
            ),
        )
        .recorded("live_e2e", "reject_flow_pi"),
        LiveTurnHooks {
            permission_response: Some(Arc::new(|_req| {
                Some(PermissionResponse::from_decision(Decision::Deny))
            })),
            interrupt_on_event: None,
        },
    )
    .await
    .unwrap();

    // This replay must exercise the deny path, then complete without writing.
    assert!(
        turn_completed(&res.events).is_some(),
        "expected turn to complete"
    );
    assert!(
        !find_events_by_kind(&res.events, Kind::PermissionRequest).is_empty(),
        "expected Pi permission request; events: {}",
        live::summarize_live_events(&res.events)
    );
    assert!(
        failed_messages(&res.events).is_empty(),
        "unexpected failures: {:?}; events: {}",
        failed_messages(&res.events),
        live::summarize_live_events(&res.events)
    );
    let transcript = assistant_transcript(&collect_timelines(
        &res.events,
        TimelineType::AssistantMessage,
    ));
    assert!(
        !transcript.contains("WRITE_DONE"),
        "denied Pi write still reported completion; events: {}",
        live::summarize_live_events(&res.events)
    );
    assert!(
        !external_path.exists()
            || fs::read_to_string(&external_path)
                .unwrap()
                .trim()
                .is_empty(),
        "denied Pi write created content at {}",
        external_path.display()
    );
}

#[tokio::test]
async fn codex_interrupt_flow() {
    let Some(provider) = recorded_provider_or_return("codex", "live_e2e", "codex_interrupt_flow")
    else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();

    let res = run_live_turn_with_hooks(
        LiveTurnSpec::new(
            provider,
            temp.path().to_path_buf(),
            "Use tools, do not simulate. Run a shell command to execute `sleep 30` in the current working directory. Do not recover or switch tools.",
        )
        .recorded("live_e2e", "codex_interrupt_flow"),
        LiveTurnHooks {
            permission_response: None,
            interrupt_on_event: Some(Arc::new(|event| {
                matches!(
                    &event.payload,
                    Payload::Timeline(timeline)
                        if timeline.item.ty == TimelineType::ToolCall
                            && timeline
                                .item
                                .tool_call
                                .as_ref()
                                .is_some_and(|tool| {
                                    tool.call.name == "shell"
                                        && tool
                                            .call
                                            .input
                                            .get("command")
                                            .and_then(serde_json::Value::as_str)
                                            .is_some_and(|command| command.contains("sleep 30"))
                                })
                )
            })),
        },
    )
    .await
    .unwrap();

    let failed = failed_messages(&res.events);
    assert!(
        !failed.is_empty(),
        "expected turn failure from interrupt; events: {}",
        live::summarize_live_events(&res.events)
    );
    assert!(
        contains_string(&failed, "aborted")
            || contains_string(&failed, "cancelled")
            || contains_string(&failed, "interrupted"),
        "unexpected interrupt failures: {:?}; events: {}",
        failed,
        live::summarize_live_events(&res.events)
    );
}

#[tokio::test]
async fn interrupt_flow_pi() {
    let Some(provider) = recorded_provider_or_return("pi", "live_e2e", "interrupt_flow_pi") else {
        return;
    };
    let temp = workspace_with_readme("live interrupt workspace\n");

    let res = run_live_turn_with_hooks(
        LiveTurnSpec::new(
            provider,
            temp.path().to_path_buf(),
            "Start a long-running response, but do not use tools. Continue until interrupted.",
        )
        .recorded("live_e2e", "interrupt_flow_pi"),
        LiveTurnHooks {
            permission_response: None,
            interrupt_on_event: Some(Arc::new(|event| {
                matches!(&event.payload, Payload::TurnStarted(_))
            })),
        },
    )
    .await
    .unwrap();

    assert!(
        turn_failed(&res.events).is_some() || turn_completed(&res.events).is_some(),
        "expected terminal turn event after interrupt; events: {}",
        live::summarize_live_events(&res.events)
    );
}
