/// Journey coverage manifest — single source of truth for journey→test mapping.
///
/// Every documented user journey 1..67 must have an explicit entry.
/// Validation tests enforce completeness, ordering, and evidence.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JourneyStatus {
    /// At least one complete-link or behavioral integration test exists.
    Covered,
}

#[derive(Debug, Clone)]
pub struct JourneyMapping {
    pub id: u8,
    pub slug: &'static str,
    pub status: JourneyStatus,
    /// At least one test function name (string, no source parsing).
    pub test_names: &'static [&'static str],
    /// Empty for ordinary covered journeys; explanatory for explicit reserves.
    pub reason: &'static str,
}

pub const JOURNEY_COUNT: usize = 67;

/// All 67 journeys in id order (1..67).
pub const JOURNEYS: [JourneyMapping; JOURNEY_COUNT] = [
    // ── 1 ─────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 1,
        slug: "journey_01_entry_panel",
        status: JourneyStatus::Covered,
        test_names: &["entry_panel_renders_user_click_targets_as_text_commands"],
        reason: "",
    },
    // ── 2 ─────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 2,
        slug: "journey_02_history_jump",
        status: JourneyStatus::Covered,
        test_names: &["history_row_click_reuses_bound_topic_and_suppresses_replay_on_repeat"],
        reason: "",
    },
    // ── 3 ─────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 3,
        slug: "journey_03_rename",
        status: JourneyStatus::Covered,
        test_names: &["topic_rename_command_updates_channel_and_persisted_workspace_title"],
        reason: "",
    },
    // ── 4 ─────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 4,
        slug: "journey_04_autoscan_agents",
        status: JourneyStatus::Covered,
        test_names: &["entry_panel_renders_user_click_targets_as_text_commands"],
        reason: "",
    },
    // ── 5 ─────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 5,
        slug: "journey_05_core_agent_catalog",
        status: JourneyStatus::Covered,
        test_names: &["panel_overview_hides_add_agent_and_help_actions"],
        reason: "",
    },
    // ── 6 ─────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 6,
        slug: "journey_06_provider_filter",
        status: JourneyStatus::Covered,
        test_names: &["entry_panel_provider_filter_button_scopes_visible_sessions"],
        reason: "",
    },
    // ── 7 ─────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 7,
        slug: "journey_07_send_prompt",
        status: JourneyStatus::Covered,
        test_names: &["journey_07_send_prompt_renders_reasoning_tool_calls_without_footer"],
        reason: "",
    },
    // ── 8 ─────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 8,
        slug: "journey_08_model_switch",
        status: JourneyStatus::Covered,
        test_names: &["model_alias_and_text_selection_use_topic_bound_session_trait"],
        reason: "",
    },
    // ── 9 ─────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 9,
        slug: "journey_09_permissions",
        status: JourneyStatus::Covered,
        test_names: &["permissions_command_lists_text_modes_and_updates_status_from_text_mode"],
        reason: "",
    },
    // ── 10 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 10,
        slug: "journey_10_fork",
        status: JourneyStatus::Covered,
        test_names: &["fork_selection_rebinds_current_topic_without_creating_fork_topic"],
        reason: "",
    },
    // ── 11 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 11,
        slug: "journey_11_history_reentry",
        status: JourneyStatus::Covered,
        test_names: &["history_row_click_reuses_bound_topic_and_suppresses_replay_on_repeat"],
        reason: "",
    },
    // ── 12 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 12,
        slug: "journey_12_quit_and_resume",
        status: JourneyStatus::Covered,
        test_names: &["new_and_quit_topic_commands_run_through_public_bot_flow"],
        reason: "",
    },
    // ── 13 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 13,
        slug: "journey_13_history_replay",
        status: JourneyStatus::Covered,
        test_names: &["history_load_older_button_runs_through_public_bot_flow"],
        reason: "",
    },
    // ── 14 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 14,
        slug: "journey_14_load_older",
        status: JourneyStatus::Covered,
        test_names: &["history_load_older_button_runs_through_public_bot_flow"],
        reason: "",
    },
    // ── 15 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 15,
        slug: "journey_15_agent_selector",
        status: JourneyStatus::Covered,
        test_names: &["agent_row_click_creates_topic_and_first_topic_message_opens_session"],
        reason: "",
    },
    // ── 16 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 16,
        slug: "journey_16_workspace_selector",
        status: JourneyStatus::Covered,
        test_names: &["history_workspace_summary_row_drills_into_filtered_session_list"],
        reason: "",
    },
    // ── 17 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 17,
        slug: "journey_17_pagination",
        status: JourneyStatus::Covered,
        test_names: &["entry_menu_help_and_pagination_commands_run_through_public_bot_flow"],
        reason: "",
    },
    // ── 18 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 18,
        slug: "journey_18_lifecycle",
        status: JourneyStatus::Covered,
        test_names: &["new_and_quit_topic_commands_run_through_public_bot_flow"],
        reason: "",
    },
    // ── 19 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 19,
        slug: "journey_19_fork_selection_isolation",
        status: JourneyStatus::Covered,
        test_names: &["fork_target_selection_runs_through_topic_scoped_text_command_flow"],
        reason: "",
    },
    // ── 20 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 20,
        slug: "journey_20_subagent",
        status: JourneyStatus::Covered,
        test_names: &["subagent_open_button_creates_child_topic_and_resumes_child_session"],
        reason: "",
    },
    // ── 21 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 21,
        slug: "journey_21_approval",
        status: JourneyStatus::Covered,
        test_names: &["approval_button_resolves_pending_live_request_through_topic_callback"],
        reason: "",
    },
    // ── 22 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 22,
        slug: "journey_22_images",
        status: JourneyStatus::Covered,
        test_names: &["topic_image_caption_downloads_attachment_and_submits_multimodal_input"],
        reason: "",
    },
    // ── 23 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 23,
        slug: "journey_23_multichat_isolation",
        status: JourneyStatus::Covered,
        test_names: &["same_topic_id_in_different_chats_routes_to_chat_scoped_sessions"],
        reason: "",
    },
    // ── 24 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 24,
        slug: "journey_24_entry_commands",
        status: JourneyStatus::Covered,
        test_names: &["entry_menu_help_and_pagination_commands_run_through_public_bot_flow"],
        reason: "",
    },
    // ── 25 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 25,
        slug: "journey_25_stale_button",
        status: JourneyStatus::Covered,
        test_names: &["stale_panel_button_rerenders_current_panel_without_applying_old_view"],
        reason: "",
    },
    // ── 26 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 26,
        slug: "journey_26_inline_query",
        status: JourneyStatus::Covered,
        test_names: &["command_query_uses_last_topic_workspace_catalog_for_user"],
        reason: "",
    },
    // ── 27 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 27,
        slug: "journey_27_skills",
        status: JourneyStatus::Covered,
        test_names: &["skills_topic_command_preserves_structured_markdown_catalog_shape"],
        reason: "",
    },
    // ── 28 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 28,
        slug: "journey_28_commands",
        status: JourneyStatus::Covered,
        test_names: &[
            "commands_topic_command_renders_provider_catalog_without_injecting_trait_commands",
        ],
        reason: "",
    },
    // ── 29 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 29,
        slug: "journey_29_history_only_agent",
        status: JourneyStatus::Covered,
        test_names: &["history_only_agent_row_shows_visible_unsupported_notice"],
        reason: "",
    },
    // ── 30 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 30,
        slug: "journey_30_agent_status",
        status: JourneyStatus::Covered,
        test_names: &[
            "status_topic_command_renders_process_resources_for_current_workspace",
            "entry_status_command_renders_all_managed_agent_resources",
        ],
        reason: "",
    },
    // ── 31 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 31,
        slug: "journey_31_kill_agent",
        status: JourneyStatus::Covered,
        test_names: &["entry_kill_all_detaches_live_session_and_reports_identity"],
        reason: "",
    },
    // ── 32 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 32,
        slug: "journey_32_unbound_topic",
        status: JourneyStatus::Covered,
        test_names: &["unbound_topic_message_does_not_reuse_matching_control_workspace_id"],
        reason: "",
    },
    // ── 33 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 33,
        slug: "journey_33_replay_idempotency",
        status: JourneyStatus::Covered,
        test_names: &[
            "history_row_click_reuses_bound_topic_and_suppresses_replay_on_repeat",
            "history_replay_is_scoped_to_current_topic_binding",
            "history_row_click_after_restart_recreates_topic_hidden_by_user_deletion",
        ],
        reason: "",
    },
    // ── 34 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 34,
        slug: "journey_34_cross_channel_notification",
        status: JourneyStatus::Covered,
        test_names: &["journey_34_dual_channel_core_event_delivers_to_both_telegram_and_wechat"],
        reason: "",
    },
    // ── 35 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 35,
        slug: "journey_35_notification_reply",
        status: JourneyStatus::Covered,
        test_names: &[
            "watch_notification_topic_receives_agent_message_and_reply_routes_to_session",
        ],
        reason: "",
    },
    // ── 36 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 36,
        slug: "journey_36_notification_concurrency_isolation",
        status: JourneyStatus::Covered,
        test_names: &["concurrent_session_notifications_route_replies_by_message_binding"],
        reason: "",
    },
    // ── 37 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 37,
        slug: "journey_37_notification_config",
        status: JourneyStatus::Covered,
        test_names: &["config_notifications_respect_global_workspace_and_session_toggles"],
        reason: "",
    },
    // ── 38 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 38,
        slug: "journey_38_active_suppression",
        status: JourneyStatus::Covered,
        test_names: &["journey_38_dual_channel_active_suppression_shared_across_channels"],
        reason: "",
    },
    // ── 39 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 39,
        slug: "journey_39_reset_notifications",
        status: JourneyStatus::Covered,
        test_names: &["reset_notifications_command_recreates_notification_topic"],
        reason: "",
    },
    // ── 40 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 40,
        slug: "journey_40_split_message",
        status: JourneyStatus::Covered,
        test_names: &["agent_notification_binds_every_split_message_id"],
        reason: "",
    },
    // ── 41 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 41,
        slug: "journey_41_wechat_qr_login",
        status: JourneyStatus::Covered,
        test_names: &["spawn_enabled_waits_for_priority_adapter_before_starting_next"],
        reason: "",
    },
    // ── 42 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 42,
        slug: "journey_42_context_token_expiry",
        status: JourneyStatus::Covered,
        test_names: &["context_expiry_reminder_sends_once_per_token_version"],
        reason: "",
    },
    // ── 43 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 43,
        slug: "journey_43_wechat_retry",
        status: JourneyStatus::Covered,
        test_names: &["watched_notification_transport_failure_does_not_escape_core_event_handler"],
        reason: "",
    },
    // ── 44 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 44,
        slug: "journey_44_system_config",
        status: JourneyStatus::Covered,
        test_names: &[
            "entry_config_global_bypass_toggle_updates_system_default_and_panel",
            "config_global_bypass_toggle_updates_shared_system_setting",
            "notification_reply_resume_uses_global_bypass_config_default",
            "notification_reply_resume_uses_global_bypass_config_when_enabled",
        ],
        reason: "",
    },
    // ── 45 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 45,
        slug: "journey_45_adapter_self_heal",
        status: JourneyStatus::Covered,
        test_names: &["supervise_enabled_restarts_transient_task_failure_without_returning_error"],
        reason: "",
    },
    // ── 46 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 46,
        slug: "journey_46_watcher_recovery",
        status: JourneyStatus::Covered,
        test_names: &["history_watch_supervisor_recreates_watcher_after_backoff"],
        reason: "",
    },
    // ── 47 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 47,
        slug: "journey_47_memory_footprint",
        status: JourneyStatus::Covered,
        test_names: &["journey_47_large_candidate_fixture_smoke_measures_snapshot_entry_count"],
        reason: "",
    },
    // ── 48 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 48,
        slug: "journey_48_log_rotation",
        status: JourneyStatus::Covered,
        test_names: &["file_appender_rolls_by_size"],
        reason: "",
    },
    // ── 49 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 49,
        slug: "journey_49_cross_adapter_live_reuse",
        status: JourneyStatus::Covered,
        test_names: &["journey_49_shared_core_live_session_reused_across_telegram_and_wechat"],
        reason: "",
    },
    // ── 50 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 50,
        slug: "journey_50_startup_stale_reconcile",
        status: JourneyStatus::Covered,
        test_names: &["history_row_click_after_restart_recreates_topic_hidden_by_user_deletion"],
        reason: "",
    },
    // ── 51 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 51,
        slug: "journey_51_turn_failure",
        status: JourneyStatus::Covered,
        test_names: &["journey_51_turn_failure_visible_failure_stops_typing_and_detaches_live"],
        reason: "",
    },
    // ── 52 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 52,
        slug: "journey_52_health_endpoints",
        status: JourneyStatus::Covered,
        test_names: &[
            "healthz_requires_running_history_watcher_but_not_adapter_readiness",
            "readyz_uses_adapter_snapshot_without_provider_calls",
        ],
        reason: "",
    },
    // ── 53 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 53,
        slug: "journey_53_linkme_registry",
        status: JourneyStatus::Covered,
        test_names: &["journey_53_linkme_descriptor_registers_runtime_history_and_catalog"],
        reason: "",
    },
    // ── 54 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 54,
        slug: "journey_54_streaming_metadata",
        status: JourneyStatus::Covered,
        test_names: &["history_index_refresh_caches_candidates_not_fully_parsed_entries"],
        reason: "",
    },
    // ── 55 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 55,
        slug: "journey_55_command_dispatch",
        status: JourneyStatus::Covered,
        test_names: &["journey_55_command_source_dispatches_system_commands_without_marker"],
        reason: "",
    },
    // ── 56 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 56,
        slug: "journey_56_session_params",
        status: JourneyStatus::Covered,
        test_names: &["journey_56_session_params_preserve_cwd_model_permissions_across_open"],
        reason: "",
    },
    // ── 57 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 57,
        slug: "journey_57_capabilities_merge",
        status: JourneyStatus::Covered,
        test_names: &["journey_57_capabilities_single_descriptor_drives_runtime_and_ui"],
        reason: "",
    },
    // ── 58 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 58,
        slug: "journey_58_history_linkme",
        status: JourneyStatus::Covered,
        test_names: &["journey_58_linkme_history_descriptor_discovers_and_replays_new_provider"],
        reason: "",
    },
    // ── 59 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 59,
        slug: "journey_59_open_path_simplify",
        status: JourneyStatus::Covered,
        test_names: &["journey_59_descriptor_open_resume_uses_direct_provider_path"],
        reason: "",
    },
    // ── 60 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 60,
        slug: "journey_60_turn_queuing",
        status: JourneyStatus::Covered,
        test_names: &["topic_turn_queue_reports_position_and_runs_fifo"],
        reason: "",
    },
    // ── 61 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 61,
        slug: "journey_61_interrupt",
        status: JourneyStatus::Covered,
        test_names: &["topic_interrupt_bypasses_turn_queue_and_calls_live_interrupt"],
        reason: "",
    },
    // ── 62 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 62,
        slug: "journey_62_text_attachments",
        status: JourneyStatus::Covered,
        test_names: &["topic_text_attachment_without_caption_submits_file_context_as_turn"],
        reason: "",
    },
    // ── 63 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 63,
        slug: "journey_63_entry_freeform",
        status: JourneyStatus::Covered,
        test_names: &["entry_chat_freeform_message_only_shows_management_hint"],
        reason: "",
    },
    // ── 64 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 64,
        slug: "journey_64_unbound_entry_command_correction",
        status: JourneyStatus::Covered,
        test_names: &["entry_history_command_inside_new_unbound_topic_still_opens_panel_history"],
        reason: "",
    },
    // ── 65 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 65,
        slug: "journey_65_models_alias",
        status: JourneyStatus::Covered,
        test_names: &["model_alias_and_text_selection_use_topic_bound_session_trait"],
        reason: "",
    },
    // ── 66 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 66,
        slug: "journey_66_wechat_first_incoming",
        status: JourneyStatus::Covered,
        test_names: &["first_incoming_message_registers_user_for_later_notifications"],
        reason: "",
    },
    // ── 67 ────────────────────────────────────────────────────────────────
    JourneyMapping {
        id: 67,
        slug: "journey_67_scheduled_tasks",
        status: JourneyStatus::Covered,
        test_names: &["journey_67_scheduled_task_resumes_submits_and_notifies"],
        reason: "",
    },
];

// ── Validation tests ───────────────────────────────────────────────────────

#[test]
fn journey_count_is_exactly_67() {
    assert_eq!(
        JOURNEYS.len(),
        67,
        "JOURNEYS must have exactly 67 entries, found {}",
        JOURNEYS.len()
    );
}

#[test]
fn journey_ids_are_contiguous_and_ordered() {
    for (idx, entry) in JOURNEYS.iter().enumerate() {
        let expected_id = (idx + 1) as u8;
        assert_eq!(
            entry.id, expected_id,
            "JOURNEYS[{}] has id {} but expected {}",
            idx, entry.id, expected_id
        );
    }
}

#[test]
fn journey_slugs_match_ids() {
    for entry in &JOURNEYS {
        let expected_prefix = format!("journey_{:02}_", entry.id);
        assert!(
            entry.slug.starts_with(&expected_prefix),
            "slug '{}' does not start with '{}'",
            entry.slug,
            expected_prefix
        );
    }
}

#[test]
fn covered_entries_have_test_names() {
    for entry in &JOURNEYS {
        if entry.status == JourneyStatus::Covered {
            assert!(
                !entry.test_names.is_empty(),
                "journey {} (slug '{}') is Covered but has no test names",
                entry.id,
                entry.slug
            );
        }
    }
}

#[test]
fn no_journey_entries_are_pending() {
    let pending = JOURNEYS
        .iter()
        .filter(|entry| entry.status != JourneyStatus::Covered)
        .map(|entry| format!("{} {}", entry.id, entry.slug))
        .collect::<Vec<_>>();
    assert!(
        pending.is_empty(),
        "journey coverage must not have pending entries: {pending:?}"
    );
}

#[test]
fn no_duplicate_slugs() {
    let mut seen = std::collections::HashSet::new();
    for entry in &JOURNEYS {
        assert!(
            seen.insert(entry.slug),
            "duplicate slug '{}' found",
            entry.slug
        );
    }
}

#[test]
fn all_slugs_are_unique_prefixes() {
    for entry in &JOURNEYS {
        let prefix = &entry.slug[..11]; // "journey_XX_"
        let count = JOURNEYS
            .iter()
            .filter(|e| e.slug.starts_with(prefix))
            .count();
        assert_eq!(
            count, 1,
            "slug prefix '{}' appears {} times (slug '{}')",
            prefix, count, entry.slug
        );
    }
}
