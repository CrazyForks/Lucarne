#[test]
fn journey_53_linkme_descriptor_registers_runtime_history_and_catalog() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let history = std::fs::read_to_string(manifest.join("src/history/mod.rs")).expect("history");
    let production_history = history
        .split("#[cfg(test)]")
        .next()
        .expect("production history");
    let runtime_module =
        std::fs::read_to_string(manifest.join("src/agent_runtime/mod.rs")).expect("runtime mod");

    for agent_type in [
        "agent_sessions::Claude",
        "agent_sessions::Codex",
        "agent_sessions::Copilot",
        "agent_sessions::Gemini",
        "agent_sessions::Pi",
    ] {
        assert!(
            !production_history.contains(agent_type),
            "lucarne history must consume provider descriptors, not concrete {agent_type}"
        );
    }
    for forbidden in [
        "DEFAULT_AGENT_PROVIDERS",
        "default_provider_ids",
        "known_provider",
        "provider_label",
    ] {
        assert!(
            !runtime_module.contains(forbidden),
            "agent_runtime must not expose public provider catalog helper {forbidden}"
        );
    }

    let ids = lucarne::history::history_providers()
        .into_iter()
        .map(|provider| provider.id())
        .collect::<Vec<_>>();
    assert!(ids.contains(&"codex"));
    assert!(ids.contains(&"pi"));
}

#[test]
fn journey_67_scheduled_tasks_are_implemented_end_to_end() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest.join("../..");
    let journey_doc = std::fs::read_to_string(
        repo.join("docs/superpowers/specs/2026-05-13-user-behavior-journeys.md"),
    )
    .expect("journey spec");
    let core_service =
        std::fs::read_to_string(manifest.join("src/core_service/service.rs")).expect("service");
    let control_plane =
        std::fs::read_to_string(manifest.join("src/control_plane/state.rs")).expect("state");

    assert!(journey_doc.contains("旅程 67：定时触发 agent 任务"));
    assert!(!journey_doc.contains("旅程 67：定时触发 agent 任务（PRD 预留）"));
    assert!(journey_doc.contains("| 67 | 定时触发 agent 任务 | 工作会话 | ✅ 已实现 |"));
    assert!(core_service.contains("run_due_scheduled_tasks"));
    assert!(control_plane.contains("mark_scheduled_task_triggered"));
}
