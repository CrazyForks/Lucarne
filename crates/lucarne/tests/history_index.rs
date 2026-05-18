use std::sync::{Mutex, OnceLock};

use lucarne::history::history_provider;
use lucarne::history::index::HistoryIndex;

#[test]
fn history_index_cache_is_read_optimized() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/history/index.rs"),
    )
    .expect("read history index source");

    assert!(
        source.contains("cache: RwLock<HistoryCache>"),
        "history index cache should allow concurrent page reads"
    );
    assert!(
        !source.contains("cache: Mutex<HistoryCache>"),
        "history index cache should not serialize all page reads through a mutex"
    );
}

#[test]
fn history_index_session_pages_stream_bounded_candidates_not_full_entries() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/history/index.rs"),
    )
    .expect("read history index source");
    let list_page_section = source
        .split("pub fn list_page")
        .nth(1)
        .expect("list_page should exist")
        .split("pub fn entry_at")
        .next()
        .expect("list_page should precede entry_at");

    assert!(
        source.contains("ranked_candidate_page_for_providers"),
        "history index session pages should build bounded ranked candidate pages"
    );
    assert!(
        !source.contains("list_all_for_providers"),
        "history index refresh must not fully parse every history file before serving page 1"
    );
    assert!(
        !source.contains("Option<Arc<Vec<HistoryCandidate>>>"),
        "history index should not retain the full candidate set after serving page 1"
    );
    assert!(
        list_page_section.contains("paged_entries_from_ranked_candidates"),
        "default session pages should lazily parse only enough candidate metadata for the requested page"
    );
    assert!(
        list_page_section.contains("cache_available_provider_ids_from_page"),
        "overview should reuse the session candidate scan for provider availability instead of running discovery twice"
    );
    assert!(
        !list_page_section.contains("default_snapshot"),
        "page 1 must not build a full metadata snapshot before pagination"
    );
    assert!(
        source.contains("metadata: HashMap<HistoryCandidateKey, Option<HistorySessionMeta>>"),
        "history index should cache per-candidate metadata across page reads"
    );
    assert!(
        !source.contains("parse_ranked_candidate_page(candidates.as_slice(), offset, limit)"),
        "default session pages must not full-parse transcript entries when the snapshot is empty"
    );
}

#[test]
fn history_index_session_pages_do_not_cache_full_candidate_vectors() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/history/index.rs"),
    )
    .expect("read history index source");

    assert!(
        !source.contains("Option<Arc<Vec<HistoryCandidate>>>"),
        "overview/session pages must not retain the full provider candidate set"
    );
    assert!(
        !source.contains("list_ranked_candidates_for_history_providers(&self.providers)"),
        "overview/session pages should stream provider discovery into a bounded ranked page"
    );
    assert!(
        source.contains("ranked_candidate_page_for_providers"),
        "history index should build bounded ranked candidate pages for session views"
    );
}

#[test]
fn workspace_pages_do_not_cache_full_session_snapshots() {
    let index_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/history/index.rs"),
    )
    .expect("read history index source");
    let history_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/history/mod.rs"),
    )
    .expect("read history source");

    assert!(
        !index_source.contains("snapshot: Option<Arc<HistorySnapshot>>"),
        "workspace pages should cache workspace aggregates, not full session snapshots"
    );
    let session_meta_section = history_source
        .split("pub(crate) struct HistorySessionMeta")
        .nth(1)
        .expect("session metadata struct should exist")
        .split("impl HistorySessionMeta")
        .next()
        .expect("session metadata struct should precede impl");
    assert!(
        !session_meta_section.contains("HistoryCandidate"),
        "cached session metadata should not clone provider source payloads"
    );
    assert!(
        index_source.contains("workspace_snapshots: HashMap"),
        "workspace pages should keep a dedicated lightweight aggregate cache"
    );
}

#[test]
fn history_index_uses_lightweight_metadata_cache_and_workspace_aggregates() {
    let index_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/history/index.rs"),
    )
    .expect("read history index source");
    let history_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/history/mod.rs"),
    )
    .expect("read history source");

    assert!(
        !index_source.contains("snapshot: Option<Arc<HistorySnapshot>>"),
        "history index must not cache full parsed metadata snapshots"
    );
    assert!(
        index_source.contains("workspace_snapshot_for_providers"),
        "workspace aggregate views should use a dedicated aggregate path"
    );
    assert!(
        index_source.contains("workspace_snapshots: HashMap"),
        "workspace views should cache only workspace aggregates"
    );
    assert!(
        index_source.contains("record_workspace_meta"),
        "workspace views should aggregate directly from provider metadata"
    );
    let filtered_page_section = index_source
        .split("pub fn list_page_filtered")
        .nth(1)
        .expect("filtered page should exist")
        .split("pub fn entry_at_filtered")
        .next()
        .expect("filtered page should precede filtered entry lookup");
    assert!(
        filtered_page_section.contains("paged_entries_from_ranked_candidates"),
        "filtered session pages should use the same lazy metadata page path as default pages"
    );
    assert!(
        !filtered_page_section.contains("workspace_snapshot_for_providers"),
        "filtered session pages must not build full provider snapshots before pagination"
    );
    assert!(
        !index_source.contains("metadata_snapshot_from_ranked_candidates"),
        "filtered sessions must not rebuild full metadata snapshots"
    );
    assert!(
        history_source.contains("try_parse_candidate_meta"),
        "filtered session metadata discovery should ask provider parsers for unified meta"
    );
    assert!(
        history_source.contains("candidate.provider.parse_source_meta"),
        "metadata parsing should use provider-owned lightweight metadata probes"
    );
    assert!(
        !index_source.contains("raw_files: HashMap"),
        "history index must not retain raw session file contents across panel renders"
    );
    assert!(
        history_source.contains("HistorySessionMeta"),
        "history index should cache lightweight session metadata, not full entries"
    );
    assert!(
        history_source.contains(".to_entry(candidate)"),
        "session pages should render entries by combining metadata with the current candidate"
    );
    assert!(
        !history_source.contains("source: HistorySource"),
        "history metadata must not retain raw transcript sources"
    );
    assert!(
        !history_source.contains("read_candidate_cwd_prefix"),
        "history must not parse provider cwd formats above agent-sessions"
    );
    assert!(
        !history_source.contains("pub entries: Vec<HistoryEntry>"),
        "history metadata must not eagerly parse/cache full HistoryEntry values"
    );
}

#[test]
fn provider_session_lookup_does_not_bypass_history_index_cache() {
    let service_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/core_service/service.rs"),
    )
    .expect("read core service source");
    let lookup_section = service_source
        .split("pub fn history_entry_for_provider_session")
        .nth(1)
        .expect("provider session history lookup should exist")
        .split("pub fn list_history_workspaces")
        .next()
        .expect("lookup should precede workspace listing");

    assert!(
        lookup_section
            .contains(".entry_for_provider_session(provider_id, session_id, project_path)"),
        "provider-session lookup should reuse HistoryIndex instead of constructing a fresh scan"
    );
    assert!(
        !lookup_section.contains("list_ranked_candidates_for_providers"),
        "provider-session lookup must not rediscover history candidates on the hot path"
    );
    assert!(
        !lookup_section.contains("metadata_snapshot_from_ranked_candidates"),
        "provider-session lookup must not rebuild metadata snapshots outside HistoryIndex"
    );
}

#[test]
fn history_transcript_hot_paths_have_no_legacy_cursor_fallbacks() {
    let history_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/history/transcript.rs"),
    )
    .expect("read history source");

    assert!(
        !history_source.contains(r#"starts_with("before:")"#),
        "legacy before: cursors must not route transcript hot paths to full forward parsing"
    );
    assert!(
        !history_source.contains("decode_skip_cursor"),
        "legacy skip cursors must not remain accepted in transcript hot paths"
    );
    assert!(
        !history_source.contains("fn transcript_entry<A>"),
        "history transcript hot paths should not keep a generic full-file parser fallback"
    );
    assert!(
        !history_source.contains("parse_agent_session_forward_selected::<A>"),
        "history transcript hot paths must not parse a whole session forward through a fallback"
    );
}

#[test]
fn provider_session_lookup_uses_lazy_candidate_metadata() {
    let index_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/history/index.rs"),
    )
    .expect("read history index source");
    let lookup_section = index_source
        .split("pub fn entry_for_provider_session")
        .nth(1)
        .expect("provider session history lookup should exist")
        .split("pub fn list_workspaces_page")
        .next()
        .expect("lookup should precede workspace listing");

    assert!(
        lookup_section.contains("collect_candidates"),
        "provider/session lookup should stream provider-owned candidates, not use a full provider snapshot"
    );
    assert!(
        lookup_section.contains("cached_candidate_meta"),
        "provider/session lookup should reuse the per-candidate metadata cache"
    );
    assert!(
        !lookup_section.contains("snapshot_for_providers"),
        "provider/session lookup must not build a full provider snapshot on cache miss"
    );
}

#[test]
fn history_session_pages_do_not_use_full_snapshots() {
    let history_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/history/mod.rs"),
    )
    .expect("read history source");
    assert!(
        !history_source.contains("pub(crate) fn sessions_page"),
        "session pages should not live on a full metadata snapshot type"
    );
    assert!(
        history_source.contains("entries_page_from_candidates"),
        "session pages should count and collect only the requested page in one pass"
    );
    assert!(
        !history_source.contains("pub(crate) fn entry_at"),
        "entry_at should use lazy page lookup instead of a separate snapshot scan helper"
    );
}

#[test]
fn history_index_refresh_only_invalidates_bounded_caches() {
    let index_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/history/index.rs"),
    )
    .expect("read history index source");
    let refresh_section = index_source
        .split("pub fn refresh_if_stale")
        .nth(1)
        .expect("refresh_if_stale should exist")
        .split("pub fn list_page")
        .next()
        .expect("refresh_if_stale should precede list_page");

    assert!(
        refresh_section.contains("let mut cache = self.cache.write()"),
        "stale refresh should re-check and update under the write lock"
    );
    assert!(
        !refresh_section.contains("if self.is_stale()"),
        "refresh_if_stale must not have a read-lock/write-lock race that duplicates discovery"
    );
    assert!(
        refresh_section.contains("self.refresh_locked(&mut cache)"),
        "stale refresh should invalidate under the write lock"
    );
    assert!(
        !index_source.contains("fn default_snapshot"),
        "history index should not build default full metadata snapshots"
    );
}

#[test]
fn provider_snapshot_keys_are_canonicalized_by_supported_provider_order() {
    let index_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/history/index.rs"),
    )
    .expect("read history index source");
    let normalize_section = index_source
        .split("fn normalize_provider_ids")
        .nth(1)
        .expect("normalize_provider_ids should exist")
        .split("#[cfg(test)]")
        .next()
        .expect("normalize_provider_ids should precede module tests");

    assert!(
        normalize_section.contains("for supported_provider_id in &self.provider_ids"),
        "provider snapshot cache keys should follow the configured provider order"
    );
    assert!(
        !normalize_section.contains("for provider_id in provider_ids"),
        "provider snapshot cache keys should not preserve caller order"
    );
}

#[test]
fn session_pages_reuse_bounded_ranked_candidate_cache_until_refresh() {
    let _guard = env_lock().lock().unwrap();
    let tmp = tempfile::TempDir::new().expect("history fixture temp dir");
    let codex_home = tmp.path().join("codex");
    let session_dir = codex_home.join("sessions/2026/05/15");
    std::fs::create_dir_all(&session_dir).expect("create codex sessions dir");
    let older_path = session_dir.join("rollout-2026-05-15T00-00-00-cache-older.jsonl");
    std::fs::write(
        &older_path,
        concat!(
            r#"{"timestamp":"2026-05-15T00:00:00.000Z","type":"session_meta","payload":{"session_id":"cache-older","cwd":"/tmp/cache-project","originator":"codex-cli","model":"gpt-5.5"}}"#,
            "\n",
            r#"{"timestamp":"2026-05-15T00:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"older prompt"}]}}"#,
            "\n",
        ),
    )
    .expect("write older codex session");
    std::thread::sleep(std::time::Duration::from_secs(1));
    let newer_path = session_dir.join("rollout-2026-05-15T00-00-01-cache-newer.jsonl");
    std::fs::write(
        &newer_path,
        concat!(
            r#"{"timestamp":"2026-05-15T00:00:02.000Z","type":"session_meta","payload":{"session_id":"cache-newer","cwd":"/tmp/cache-project","originator":"codex-cli","model":"gpt-5.5"}}"#,
            "\n",
            r#"{"timestamp":"2026-05-15T00:00:03.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"newer prompt"}]}}"#,
            "\n",
        ),
    )
    .expect("write newer codex session");
    let _env = EnvGuard::set(&[
        ("CODEX_HOME", codex_home.as_os_str().to_os_string()),
        ("HOME", tmp.path().as_os_str().to_os_string()),
    ]);
    let index = HistoryIndex::new(
        vec![history_provider("codex").expect("codex provider")],
        std::time::Duration::from_secs(60),
    );

    let first = index.list_page(0, 1);
    std::fs::remove_file(&newer_path).expect("remove top session after page cache");
    let cached = index.list_page(0, 1);
    index.refresh();
    let refreshed = index.list_page(0, 1);

    assert_eq!(first.total, 2);
    assert_eq!(first.entries[0].session_id, "cache-newer");
    assert_eq!(cached.total, 2);
    assert_eq!(cached.entries[0].session_id, "cache-newer");
    assert_eq!(refreshed.total, 1);
    assert_eq!(refreshed.entries[0].session_id, "cache-older");
}

#[test]
fn provider_session_lookup_reflects_removed_files_without_full_candidate_cache() {
    let _guard = env_lock().lock().unwrap();
    let tmp = tempfile::TempDir::new().expect("history fixture temp dir");
    let codex_home = tmp.path().join("codex");
    let session_dir = codex_home.join("sessions/2026/05/15");
    std::fs::create_dir_all(&session_dir).expect("create codex sessions dir");
    let session_path = session_dir.join("rollout-2026-05-15T00-00-00-cache-thread.jsonl");
    std::fs::write(
        &session_path,
        concat!(
            r#"{"timestamp":"2026-05-15T00:00:00.000Z","type":"session_meta","payload":{"session_id":"cache-thread","cwd":"/tmp/cache-project","originator":"codex-cli","model":"gpt-5.5"}}"#,
            "\n",
            r#"{"timestamp":"2026-05-15T00:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"cached prompt"}]}}"#,
            "\n",
        ),
    )
    .expect("write codex session");
    let _env = EnvGuard::set(&[
        ("CODEX_HOME", codex_home.as_os_str().to_os_string()),
        ("HOME", tmp.path().as_os_str().to_os_string()),
    ]);
    let index = HistoryIndex::new(
        vec![history_provider("codex").expect("codex provider")],
        std::time::Duration::from_secs(60),
    );

    let first = index
        .entry_for_provider_session("codex", "cache-thread", None)
        .expect("initial lookup");
    std::fs::remove_file(&session_path).expect("remove session after snapshot");
    let cached = index.entry_for_provider_session("codex", "cache-thread", None);

    assert_eq!(first.session_id, "cache-thread");
    assert!(
        cached.is_none(),
        "lookup should reflect provider discovery after the source file disappears"
    );
}

#[test]
fn history_transcript_dispatch_keeps_provider_responsibility_out_of_common_layer() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let history_mod_source = std::fs::read_to_string(manifest.join("src/history/mod.rs"))
        .expect("read history mod source");
    let provider_source = std::fs::read_to_string(manifest.join("src/history/provider.rs"))
        .expect("read history provider source");
    let transcript_source = std::fs::read_to_string(manifest.join("src/history/transcript.rs"))
        .expect("read history transcript source");
    let render_source = std::fs::read_to_string(manifest.join("src/history/render.rs"))
        .expect("read history render source");
    let history_source = [
        history_mod_source.as_str(),
        provider_source.as_str(),
        transcript_source.as_str(),
        render_source.as_str(),
    ]
    .join("\n");
    let version_source = std::fs::read_to_string(manifest.join("src/adapters/version.rs"))
        .expect("read version source");
    let adapters_source = std::fs::read_to_string(manifest.join("src/adapters/mod.rs"))
        .expect("read adapters source");
    let production_source = history_mod_source
        .split("\n#[cfg(test)]\nmod tests")
        .next()
        .expect("history production source should precede tests");

    assert!(
        !history_source.contains("agent_sessions::Gemini")
            && !history_source.contains("agent_sessions::Codex"),
        "lucarne history modules must consume descriptors, not concrete provider types"
    );
    assert!(
        !history_source.contains("codex_transcript_entry"),
        "common history layer must not expose provider-specific transcript entry points"
    );
    assert!(
        !history_source.contains("gemini_transcript_entry"),
        "common history layer must not expose provider-specific transcript entry points"
    );
    assert!(
        transcript_source.contains("HISTORY_TAIL_CURSOR_PREFIX"),
        "lucarne::history should own bounded byte cursor semantics"
    );
    assert!(
        !production_source.contains("is_parseable_session_file"),
        "common history layer must not hard-code provider candidate file-format selection"
    );
    assert!(
        !production_source.contains("\"sessionId\"") && !production_source.contains("\"messages\""),
        "common history layer must not build provider-specific JSON transcript envelopes"
    );
    assert!(
        !production_source.contains("is_agent_instruction_preamble")
            && !production_source.contains("is_turn_aborted_control_marker"),
        "common history layer must not own provider-specific transcript visibility filters"
    );
    assert!(
        !production_source.contains("media_type_for_path")
            && !production_source.contains("input_metadata_for_path"),
        "common history layer must not guess provider transcript input metadata from paths"
    );
    assert!(
        provider_source.contains("SessionFileFormat::JsonDocument")
            && provider_source.contains("UnsupportedProvider"),
        "JSON document providers should fail closed through the history provider wrapper"
    );
    assert!(
        transcript_source.contains(".parse_agent_session_bytes("),
        "bounded transcript replay should parse through provider descriptors"
    );
    assert!(
        render_source.contains("is_transcript_user_text_visible"),
        "rendering should ask provider descriptor visibility before exposing user text"
    );
    assert!(
        !production_source.contains("IntoAgentSession"),
        "history transcript loading should use direct semantic readers, not the raw projection trait"
    );
    assert!(
        !production_source.contains("parse_agent_session_forward_selected::<A>"),
        "history transcript hot paths must not keep a whole-file forward parse fallback"
    );
    assert!(
        !version_source.contains("MIN_VERSIONS"),
        "shared version helper must not own provider minimum-version policy"
    );
    assert!(
        !adapters_source.contains("resumable_session_field"),
        "shared adapter layer must not own provider resume handle field semantics"
    );
}

#[test]
fn history_metadata_collection_is_not_bound_to_projection_trait() {
    let history_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/history/mod.rs"),
    )
    .expect("read history source");
    let collect_section = history_source
        .split("fn collect(provider: HistoryProviderDescriptor)")
        .nth(1)
        .expect("generic history entry collector should exist")
        .split("fn collect_candidates(provider")
        .next()
        .expect("history entry collector should precede candidate collector");
    let meta_section = history_source
        .split("fn try_parse_candidate_meta(")
        .nth(1)
        .expect("candidate metadata parser should exist")
        .split("fn history_transcript_selection")
        .next()
        .expect("candidate metadata parser should precede timestamp helper");

    assert!(
        !collect_section.contains("IntoAgentSession"),
        "history metadata collection should not require raw projection support"
    );
    assert!(
        !meta_section.contains("IntoAgentSession"),
        "history candidate metadata parsing should use direct provider metadata probes"
    );
    assert!(
        meta_section.contains("candidate.provider.parse_source_meta"),
        "history metadata parsing should call provider-owned metadata probes"
    );
}

#[test]
fn journey_47_large_candidate_fixture_smoke_measures_snapshot_entry_count() {
    let _guard = env_lock().lock().unwrap();
    let tmp = tempfile::TempDir::new().expect("history fixture temp dir");
    let codex_home = tmp.path().join("codex");
    let session_dir = codex_home.join("sessions/2026/05/05");
    std::fs::create_dir_all(&session_dir).expect("create codex sessions dir");
    for idx in 0..500 {
        let session_id = format!("bulk-{idx:04}");
        let path = session_dir.join(format!(
            "rollout-2026-05-05T00-{:02}-00-{session_id}.jsonl",
            idx % 60
        ));
        std::fs::write(
            path,
            format!(
                "{}\n{}\n",
                format_args!(
                    r#"{{"timestamp":"2026-05-05T00:00:00.000Z","type":"session_meta","payload":{{"session_id":"{session_id}","cwd":"/tmp/bulk-{idx}","originator":"codex-cli","model":"gpt-5.5"}}}}"#
                ),
                format_args!(
                    r#"{{"timestamp":"2026-05-05T00:{:02}:00.000Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"bulk prompt {idx}"}}]}}}}"#,
                    idx % 60
                )
            ),
        )
        .expect("write codex session");
    }
    let _env = EnvGuard::set(&[
        ("CODEX_HOME", codex_home.as_os_str().to_os_string()),
        ("HOME", tmp.path().as_os_str().to_os_string()),
    ]);
    let index = HistoryIndex::new(
        vec![history_provider("codex").expect("codex provider")],
        std::time::Duration::from_secs(30),
    );

    let page = index.list_page(0, 10);

    assert_eq!(page.total, 500);
    assert_eq!(page.entries.len(), 10);
    assert!(page
        .entries
        .iter()
        .all(|entry| entry.provider_id == "codex" && entry.summary.starts_with("bulk prompt")));
}

#[test]
fn history_index_filters_to_configured_providers_and_pages_cached_entries() {
    let index = HistoryIndex::new(
        vec![history_provider("codex").expect("codex provider")],
        std::time::Duration::from_secs(30),
    );
    let page = index.list_page(0, 5);

    assert!(page.total >= page.entries.len());
    assert!(page
        .entries
        .iter()
        .all(|entry| entry.provider_id == "codex"));
    assert!(!index.is_stale());
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct EnvGuard {
    old: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    fn set(values: &[(&'static str, std::ffi::OsString)]) -> Self {
        let old = values
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect::<Vec<_>>();
        for (key, value) in values {
            std::env::set_var(key, value);
        }
        Self { old }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.old.drain(..) {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}
