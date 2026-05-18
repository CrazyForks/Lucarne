#[test]
fn legacy_generic_raw_api_is_not_exported_from_crate_root() {
    let lib_rs = include_str!("../src/lib.rs");

    for removed_export in [
        concat!("pub use agent::{", "Provider", "Parser"),
        "pub use agent::{CandidateEntry, CandidateRole, CandidateSource, DiscoverableProvider}",
        "pub use input::{Bundle",
        "pub use session::Session",
    ] {
        assert!(
            !lib_rs.contains(removed_export),
            "crate root must not re-export legacy raw API: {removed_export}"
        );
    }

    assert!(
        !lib_rs.contains("pub use agent::CandidateEntry;"),
        "CandidateEntry must stay behind the provider descriptor boundary"
    );
    assert!(
        lib_rs.contains("pub use input::InputMetadata;"),
        "reader metadata remains a small provider parse carrier"
    );
}

#[test]
fn legacy_generic_raw_types_are_removed_or_crate_private_only() {
    let agent_rs = include_str!("../src/agent.rs");
    let input_rs = include_str!("../src/input.rs");

    for forbidden in [
        concat!("pub trait ", "Provider", "Parser"),
        concat!(
            "pub struct ",
            "Provider",
            "Parsed",
            "<A: ",
            "Provider",
            "Parser",
            ">"
        ),
        "pub trait DiscoverableProvider",
        "pub struct CandidateSource<A: DiscoverableProvider>",
        "pub struct Bundle",
        "pub struct BundleEntry",
        concat!("pub struct Session<A: ", "Provider", "Parser", ">"),
        concat!("pub(crate) trait ", "Provider", "Parser"),
        concat!(
            "pub(crate) struct ",
            "Provider",
            "Parsed",
            "<A: ",
            "Provider",
            "Parser",
            ">"
        ),
        "pub(crate) struct Bundle",
        "pub(crate) struct BundleEntry",
        concat!("pub(crate) struct Session<A: ", "Provider", "Parser", ">"),
    ] {
        assert!(
            !agent_rs.contains(forbidden) && !input_rs.contains(forbidden),
            "legacy raw type must not be public: {forbidden}"
        );
    }

    for internal in [
        "pub(crate) trait DiscoverableProvider",
        "pub(crate) struct AgentProviderSourceEntry",
    ] {
        assert!(
            agent_rs.contains(internal) || input_rs.contains(internal),
            "remaining transition internals must be explicitly crate-private: {internal}"
        );
    }
}

#[test]
fn provider_descriptor_accepts_semantic_bytes_not_legacy_bundle() {
    let descriptor_rs = include_str!("../src/providers/descriptor.rs");
    let transcript_rs = include_str!("../../crates/lucarne/src/history/transcript.rs");

    assert!(
        !descriptor_rs.contains("parse_agent_session: fn(Bundle")
            && !descriptor_rs
                .contains("pub fn parse_agent_session(\n        self,\n        bundle: Bundle"),
        "provider descriptor must not expose Bundle-based parsing"
    );
    assert!(
        descriptor_rs.contains("parse_agent_session_bytes")
            && descriptor_rs.contains("bytes: Vec<u8>"),
        "provider descriptor should expose owned byte-window semantic parsing"
    );
    assert!(
        !transcript_rs.contains("Bundle::single"),
        "lucarne history must pass bounded byte windows directly, without rebuilding Bundle"
    );
}

#[test]
fn candidate_sources_are_opaque_descriptor_payloads() {
    let lib_rs = include_str!("../src/lib.rs");
    let agent_rs = include_str!("../src/agent.rs");
    let descriptor_rs = include_str!("../src/providers/descriptor.rs");

    assert!(
        !lib_rs.contains("CandidateEntry"),
        "CandidateEntry must not appear in crate root"
    );
    assert!(
        agent_rs.contains("pub(crate) struct AgentProviderSourceEntry"),
        "AgentProviderSourceEntry should remain crate-private provider discovery data"
    );
    for forbidden in [
        "pub path: PathBuf",
        "pub entries: Vec<AgentProviderSourceEntry>",
        "pub fn parse_meta(\n        self,\n        entries: &[AgentProviderSourceEntry]",
        "parse_meta: fn(&[AgentProviderSourceEntry])",
    ] {
        assert!(
            !descriptor_rs.contains(forbidden),
            "descriptor must not expose candidate internals: {forbidden}"
        );
    }
    for required in [
        "pub fn path(&self) -> &Path",
        "pub fn last_modified_unix(&self) -> i64",
        "pub fn parse_source_meta",
    ] {
        assert!(
            descriptor_rs.contains(required),
            "opaque source API missing: {required}"
        );
    }
    for forbidden in ["pub fn single_file", "pub fn from_path"] {
        assert!(
            !descriptor_rs.contains(forbidden),
            "test-only source constructors must not be public: {forbidden}"
        );
    }
}

#[test]
fn provider_modules_do_not_reexport_raw_type_trees() {
    for module in [
        include_str!("../src/providers/codex/mod.rs"),
        include_str!("../src/providers/claude/mod.rs"),
        include_str!("../src/providers/copilot/mod.rs"),
        include_str!("../src/providers/cursor/mod.rs"),
        include_str!("../src/providers/gemini/mod.rs"),
        include_str!("../src/providers/pi/mod.rs"),
    ] {
        assert!(
            !module.contains("pub use types::*;"),
            "provider raw Body/Entry trees must not be re-exported as public API"
        );
        assert!(
            module.contains("pub(crate) use types::*;"),
            "provider raw type trees may remain provider-private parse internals"
        );
    }
}

#[test]
fn provider_raw_body_entry_trees_are_crate_private() {
    for types_rs in [
        include_str!("../src/providers/codex/types.rs"),
        include_str!("../src/providers/claude/types.rs"),
        include_str!("../src/providers/copilot/types.rs"),
        include_str!("../src/providers/cursor/types.rs"),
        include_str!("../src/providers/gemini/types.rs"),
        include_str!("../src/providers/pi/types.rs"),
    ] {
        for forbidden in ["pub struct ", "pub enum "] {
            assert!(
                !types_rs.contains(forbidden),
                "provider raw type tree must be crate-private: {forbidden}"
            );
        }
    }
}

#[test]
fn parse_selection_has_no_public_state_dimension() {
    let parse_selection_rs = include_str!("../src/parse_selection.rs");

    for forbidden in [
        "state: bool",
        "pub const fn with_state",
        "pub const fn includes_state",
    ] {
        assert!(
            !parse_selection_rs.contains(forbidden),
            "ParseSelection state must not be public selection API: {forbidden}"
        );
    }
}

#[test]
fn custom_input_streaming_surface_stays_removed() {
    let input_rs = include_str!("../src/input.rs");
    let lib_rs = include_str!("../src/lib.rs");

    for forbidden in [
        "pub struct InputStream",
        "pub fn stream",
        "pub trait Input",
        "impl Input for",
    ] {
        assert!(
            !input_rs.contains(forbidden) && !lib_rs.contains(forbidden),
            "fake/custom input compatibility surface must stay removed: {forbidden}"
        );
    }
}

#[test]
fn watch_events_are_semantic_and_clone_cheap() {
    let watch_event_rs = include_str!("../src/watch/event.rs");
    let watch_state_rs = include_str!("../src/watch/state.rs");

    assert!(watch_event_rs.contains("pub enum WatchEvent"));
    assert!(watch_event_rs.contains("pub events: Box<[WatchEvent]>"));
    assert!(watch_event_rs.contains("use smol_str::SmolStr;"));
    assert!(watch_state_rs.contains("last_prompt_timestamp: Option<SmolStr>"));

    for field in [
        "pub text: Option<SmolStr>",
        "pub model: Option<SmolStr>",
        "pub phase: Option<SmolStr>",
        "pub call_id: Option<SmolStr>",
        "pub name: SmolStr",
        "pub input_json: Option<SmolStr>",
        "pub command: Option<SmolStr>",
        "pub file_path: Option<SmolStr>",
        "pub output_json: Option<SmolStr>",
        "pub speed: Option<SmolStr>",
        "pub last_agent_message: Option<SmolStr>",
        "pub reason: Option<SmolStr>",
        "pub kind: SmolStr",
        "pub value_json: Option<SmolStr>",
        "pub raw_json: SmolStr",
    ] {
        assert!(
            watch_event_rs.contains(field),
            "watch event payload field should use SmolStr: {field}"
        );
    }
}

#[test]
fn agent_session_core_labels_and_metadata_are_clone_cheap() {
    let agent_session_rs = include_str!("../src/agent_session/mod.rs");

    for field in [
        "pub struct AgentKind(pub SmolStr)",
        "pub struct VersionKind(pub SmolStr)",
        "Other(SmolStr)",
        "Custom(SmolStr)",
        "pub session_id: Option<SmolStr>",
        "pub thread_id: Option<SmolStr>",
        "pub created_at: Option<SmolStr>",
        "pub updated_at: Option<SmolStr>",
        "pub source_kind: Option<SmolStr>",
        "pub model: SmolStr",
        "pub model: Option<SmolStr>",
        "pub phase: Option<SmolStr>",
        "pub name: SmolStr",
        "pub id: Option<SmolStr>",
        "pub tool_use_id: Option<SmolStr>",
    ] {
        assert!(
            agent_session_rs.contains(field),
            "agent_session field should use SmolStr: {field}"
        );
    }
}

#[test]
fn watch_delta_parse_path_uses_reader_dispatch_without_bytes_wrapper() {
    let watch_mod_rs = include_str!("../src/watch/mod.rs");
    let watch_provider_rs = include_str!("../src/watch/provider.rs");
    let descriptor_rs = include_str!("../src/providers/descriptor.rs");

    assert!(
        !watch_provider_rs.contains("parse_provider_bytes"),
        "watch provider layer must not keep byte-wrapper dispatch"
    );
    assert!(
        !descriptor_rs.contains("parse_watch_bytes"),
        "provider descriptor watch delta seam must be reader-based"
    );
    assert!(
        watch_mod_rs.contains("parse_watch_reader"),
        "session watcher should dispatch deltas through provider reader seam"
    );
}

#[test]
fn watch_parse_seam_does_not_use_agent_session_projection_bridge() {
    let provider_rs = include_str!("../src/watch/provider.rs");

    assert!(
        provider_rs.contains("fn parse_watch_reader"),
        "watch providers should own their watch parse seam"
    );
    for forbidden in [
        "trait ProviderWatchEvents: IntoAgentSession",
        "IntoAgentSession",
        "parse_agent_session_reader",
        "watch_events_from_agent_events",
        "WatchEvent::from_agent_event",
    ] {
        assert!(
            !provider_rs.contains(forbidden),
            "watch layer must not route through shared agent_session projection: {forbidden}"
        );
    }
}
