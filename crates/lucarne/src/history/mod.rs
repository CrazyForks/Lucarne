//! History listing backed by `agent-sessions` discovery.
//!
//! We enumerate candidate session files across supported providers,
//! keep lightweight metadata snapshots for list views, and replay
//! bounded transcript windows only when a session is opened.

use agent_sessions::ParseSelection;
pub mod index;
mod provider;
mod render;
mod transcript;

pub use index::{HistoryIndex, IndexedHistoryPage, IndexedHistoryWorkspacePage};
pub use provider::{
    history_provider, history_providers, history_providers_for_ids, HistoryProviderDescriptor,
};
use provider::{history_provider as find_history_provider, HistoryProviderSource};
use render::transcript_from_session_window;

use std::{
    cmp::{Ordering, Reverse},
    collections::{BTreeMap, BinaryHeap, HashSet},
    path::{Path, PathBuf},
    time::SystemTime,
};
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    /// Provider id (`claude`, `codex`, ...).
    pub provider_id: &'static str,
    /// Provider-reported session id (used to resume).
    pub session_id: String,
    /// Path to the raw session file — used to re-read the history entry.
    pub session_path: PathBuf,
    /// Project cwd reported by the agent, if any.
    pub cwd: Option<String>,
    /// First user message (truncated, single-line).
    pub summary: String,
    /// Last-active time as unix seconds.
    pub last_active_unix: i64,
    /// Last-active timestamp for display (`MM-DD HH:MM:SS` in local time).
    pub last_active_display: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryWorkspace {
    pub cwd: PathBuf,
    pub display_name: String,
    pub provider_ids: Vec<&'static str>,
    pub session_count: usize,
    pub last_active_unix: i64,
    pub last_active_display: String,
}

#[derive(Debug, Clone)]
pub(crate) struct HistorySessionMeta {
    pub(crate) session_id: String,
    pub(crate) title: Option<String>,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) created_at: Option<String>,
    pub(crate) updated_at: Option<String>,
}

impl HistorySessionMeta {
    pub(crate) fn matches_cwd(&self, cwd: Option<&Path>) -> bool {
        let Some(cwd) = cwd else {
            return true;
        };
        self.cwd
            .as_deref()
            .is_some_and(|session_cwd| session_cwd == cwd)
    }

    pub(crate) fn to_entry(&self, candidate: &HistoryCandidate) -> HistoryEntry {
        let (last_active_unix, last_active_display) = self.last_active(candidate);
        HistoryEntry {
            provider_id: candidate.provider_id,
            session_id: self.session_id.clone(),
            session_path: candidate.path.clone(),
            cwd: self.cwd.as_ref().map(|cwd| cwd.display().to_string()),
            summary: self.summary(candidate),
            last_active_unix,
            last_active_display,
        }
    }

    fn summary(&self, candidate: &HistoryCandidate) -> String {
        if let Some(title) = self
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
        {
            return first_line_snippet(title, 80);
        }
        candidate
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(|stem| first_line_snippet(stem, 80))
            .filter(|summary| !summary.is_empty())
            .unwrap_or_else(|| "(session metadata)".into())
    }

    pub(crate) fn last_active(&self, candidate: &HistoryCandidate) -> (i64, String) {
        if let Some(ts) = self
            .updated_at
            .as_deref()
            .filter(|ts| !ts.trim().is_empty())
        {
            if let Some(unix) = parse_rfc3339_unix(ts) {
                return (unix, format_unix(unix));
            }
        }
        if candidate.last_modified_unix > 0 {
            let unix = candidate.last_modified_unix;
            return (unix, format_unix(unix));
        }
        if let Some(ts) = self
            .created_at
            .as_deref()
            .filter(|ts| !ts.trim().is_empty())
        {
            if let Some(unix) = parse_rfc3339_unix(ts) {
                return (unix, format_unix(unix));
            }
        }
        (0, format_unix(0))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryCursor(String);

impl HistoryCursor {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryImage {
    pub image_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryMessage {
    pub id: String,
    pub text: String,
    pub timestamp: Option<String>,
    pub images: Vec<HistoryImage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryTurn {
    pub id: String,
    pub user: HistoryMessage,
    pub assistant: Option<HistoryMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryTranscript {
    pub provider_id: &'static str,
    pub session_id: String,
    pub session_path: PathBuf,
    pub turns: Vec<HistoryTurn>,
    pub has_older: bool,
    pub older_cursor: Option<HistoryCursor>,
}

#[derive(Debug, thiserror::Error)]
pub enum HistoryTranscriptError {
    #[error("unsupported history provider: {provider_id}")]
    UnsupportedProvider { provider_id: String },
    #[error("failed to read history transcript {path}: {message}")]
    Read { path: PathBuf, message: String },
    #[error("failed to parse history transcript {path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("invalid history cursor {cursor}: {message}")]
    InvalidCursor { cursor: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct HistoryCandidateKey {
    provider_id: &'static str,
    path: PathBuf,
    last_modified_unix: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct HistoryCandidate {
    provider_id: &'static str,
    provider: HistoryProviderDescriptor,
    path: PathBuf,
    source: Option<HistoryProviderSource>,
    last_modified_unix: i64,
}

impl HistoryCandidate {
    #[cfg(test)]
    pub(crate) fn new(provider_id: &'static str, path: PathBuf, last_modified_unix: i64) -> Self {
        let provider = find_history_provider(provider_id).expect("history provider");
        Self {
            provider_id,
            provider,
            path,
            source: None,
            last_modified_unix,
        }
    }

    fn from_source(provider: HistoryProviderDescriptor, source: HistoryProviderSource) -> Self {
        let path = source.path().to_path_buf();
        let last_modified_unix = source.last_modified_unix();
        Self {
            provider_id: provider.id(),
            provider,
            path,
            source: Some(source),
            last_modified_unix,
        }
    }

    pub(crate) fn provider_id(&self) -> &'static str {
        self.provider_id
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn last_modified_unix(&self) -> i64 {
        self.last_modified_unix
    }

    pub(crate) fn key(&self) -> HistoryCandidateKey {
        HistoryCandidateKey {
            provider_id: self.provider_id,
            path: self.path.clone(),
            last_modified_unix: self.last_modified_unix,
        }
    }
}

/// Enumerate all sessions, newest first (no truncation).
pub fn list_all() -> Vec<HistoryEntry> {
    list_all_for_history_providers(&history_providers())
}

pub fn list_all_for_providers(provider_ids: &[&str]) -> Vec<HistoryEntry> {
    let providers = normalize_history_providers(provider_ids);
    list_all_for_history_providers(&providers)
}

pub fn list_all_for_history_providers(
    providers: &[HistoryProviderDescriptor],
) -> Vec<HistoryEntry> {
    let mut all: Vec<HistoryEntry> = Vec::new();
    for provider in providers {
        all.extend(collect(*provider));
    }
    all.sort_by_key(|e| Reverse(e.last_active_unix));
    debug!(
        target: "lucarne::history",
        total = all.len(),
        "assembled history entries"
    );
    all
}

/// Enumerate recent sessions across all configured providers, newest
/// first, limited to `max` entries.
pub fn list_recent(max: usize) -> Vec<HistoryEntry> {
    list_page(0, max).0
}

pub fn list_recent_for_providers(provider_ids: &[&str], max: usize) -> Vec<HistoryEntry> {
    list_page_for_providers(provider_ids, 0, max).0
}

/// Pagination helper: returns `(page_entries, total_available)`.
pub fn list_page(offset: usize, limit: usize) -> (Vec<HistoryEntry>, usize) {
    list_page_for_history_providers(&history_providers(), offset, limit)
}

pub fn list_page_for_providers(
    provider_ids: &[&str],
    offset: usize,
    limit: usize,
) -> (Vec<HistoryEntry>, usize) {
    let providers = normalize_history_providers(provider_ids);
    list_page_for_history_providers(&providers, offset, limit)
}

pub fn list_page_for_history_providers(
    providers: &[HistoryProviderDescriptor],
    offset: usize,
    limit: usize,
) -> (Vec<HistoryEntry>, usize) {
    let page = ranked_candidate_page_for_providers(providers, offset.saturating_add(limit));
    entries_page_from_candidates(
        &page.candidates,
        None,
        offset,
        limit,
        |candidate| parse_candidate_meta(candidate).ok(),
        page.total,
    )
}

pub fn entry_at(index: usize) -> Option<HistoryEntry> {
    entry_at_for_history_providers(&history_providers(), index)
}

pub fn entry_at_for_providers(provider_ids: &[&str], index: usize) -> Option<HistoryEntry> {
    let providers = normalize_history_providers(provider_ids);
    entry_at_for_history_providers(&providers, index)
}

pub fn entry_at_for_history_providers(
    providers: &[HistoryProviderDescriptor],
    index: usize,
) -> Option<HistoryEntry> {
    list_page_for_history_providers(providers, index, 1)
        .0
        .into_iter()
        .next()
}

pub fn history_transcript_for_entry(
    entry: &HistoryEntry,
    limit: usize,
    cursor: Option<&HistoryCursor>,
) -> Result<HistoryTranscript, HistoryTranscriptError> {
    let provider = find_history_provider(entry.provider_id).ok_or_else(|| {
        HistoryTranscriptError::UnsupportedProvider {
            provider_id: entry.provider_id.to_string(),
        }
    })?;
    transcript_entry(provider, entry, limit, cursor)
}

fn transcript_entry(
    provider: HistoryProviderDescriptor,
    entry: &HistoryEntry,
    limit: usize,
    cursor: Option<&HistoryCursor>,
) -> Result<HistoryTranscript, HistoryTranscriptError> {
    let transcript = provider.parse_transcript(
        &entry.session_path,
        cursor.map(HistoryCursor::as_str),
        history_transcript_selection(),
        limit,
    )?;
    transcript_from_session_window(
        provider,
        entry.provider_id,
        &entry.session_id,
        entry.session_path.clone(),
        transcript.session(),
        &transcript,
        limit,
    )
}

fn collect(provider: HistoryProviderDescriptor) -> Vec<HistoryEntry> {
    let provider_id = provider.id();
    let roots = provider.default_roots();
    let roots_len = roots.len();
    let roots_log = format_roots(&roots);
    let mut sources_len = 0usize;
    let mut out = Vec::new();
    let mut stats = ScanStats::new(roots_len, 0);
    let mut failure_details_logged = 0usize;
    let discovered = provider.discover_sources_into(&mut |src| {
        sources_len += 1;
        stats.candidates += 1;
        let candidate = HistoryCandidate::from_source(provider, src);
        match try_parse_candidate_meta(&candidate) {
            Ok(meta) => {
                stats.record_parsed();
                out.push(meta.to_entry(&candidate));
            }
            Err(err) => {
                stats.record_failure(&err);
                if failure_details_logged < 5 {
                    debug!(
                        target: "lucarne::history",
                        provider = provider_id,
                        path = %candidate.path.display(),
                        error_kind = err.kind(),
                        error = %err.message(),
                        "session candidate skipped"
                    );
                    failure_details_logged += 1;
                }
            }
        }
    });
    match discovered {
        Ok(()) => {}
        Err(err) => {
            warn!(
                target: "lucarne::history_discovery",
                provider = provider_id,
                roots = roots_len,
                root_paths = %roots_log,
                error = %err,
                "history discovery failed"
            );
            return Vec::new();
        }
    }
    if stats.skipped() > 0 {
        warn!(
            target: "lucarne::history",
            provider = provider_id,
            roots = stats.roots,
            root_paths = %roots_log,
            sources = sources_len,
            candidates = stats.candidates,
            parsed = stats.parsed,
            read_failures = stats.read_failures,
            parse_failures = stats.parse_failures,
            skipped = stats.skipped(),
            failure_details_logged,
            "session scan skipped candidates"
        );
    }
    info!(
        target: "lucarne::history",
        provider = provider_id,
        roots = stats.roots,
        root_paths = %roots_log,
        sources = sources_len,
        candidates = stats.candidates,
        parsed = stats.parsed,
        read_failures = stats.read_failures,
        parse_failures = stats.parse_failures,
        "session scan summary"
    );
    out
}

#[cfg(test)]
pub(crate) fn test_ranked_candidates_for_provider_ids(
    provider_ids: &[&str],
) -> Vec<HistoryCandidate> {
    let providers = normalize_history_providers(provider_ids);
    ranked_candidate_page_for_providers(&providers, usize::MAX).candidates
}

#[derive(Debug, Clone)]
pub(crate) struct HistoryCandidatePage {
    pub(crate) candidates: Vec<HistoryCandidate>,
    pub(crate) total: usize,
    pub(crate) provider_ids_by_activity: Vec<&'static str>,
}

pub(crate) fn ranked_candidate_page_for_providers(
    providers: &[HistoryProviderDescriptor],
    retain_limit: usize,
) -> HistoryCandidatePage {
    let mut heap = BinaryHeap::<Reverse<RankedHistoryCandidate>>::new();
    let mut latest_by_provider = BTreeMap::<&'static str, (usize, i64)>::new();
    let mut total = 0usize;
    let mut sequence = 0usize;
    for (provider_order, provider) in providers.iter().enumerate() {
        collect_candidates(*provider, |candidate| {
            total = total.saturating_add(1);
            latest_by_provider
                .entry(candidate.provider_id())
                .and_modify(|(_, last_active)| {
                    *last_active = (*last_active).max(candidate.last_modified_unix())
                })
                .or_insert_with(|| (provider_order, candidate.last_modified_unix()));
            if retain_limit == 0 {
                return;
            }
            sequence = sequence.saturating_add(1);
            let ranked = RankedHistoryCandidate {
                last_modified_unix: candidate.last_modified_unix,
                sequence,
                candidate,
            };
            if heap.len() < retain_limit {
                heap.push(Reverse(ranked));
            } else if heap
                .peek()
                .is_some_and(|oldest| ranked.cmp(&oldest.0).is_gt())
            {
                heap.pop();
                heap.push(Reverse(ranked));
            }
        });
    }
    let mut candidates = heap
        .into_iter()
        .map(|ranked| ranked.0.candidate)
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .last_modified_unix
            .cmp(&left.last_modified_unix)
            .then_with(|| left.path.cmp(&right.path))
    });
    let mut provider_ids_by_activity = latest_by_provider.into_iter().collect::<Vec<_>>();
    provider_ids_by_activity.sort_by(
        |(_, (left_order, left_last_active)), (_, (right_order, right_last_active))| {
            right_last_active
                .cmp(left_last_active)
                .then_with(|| left_order.cmp(right_order))
        },
    );
    let provider_ids_by_activity = provider_ids_by_activity
        .into_iter()
        .map(|(provider_id, _)| provider_id)
        .collect::<Vec<_>>();
    debug!(
        target: "lucarne::history",
        total,
        retained = candidates.len(),
        "assembled ranked history candidates"
    );
    HistoryCandidatePage {
        candidates,
        total,
        provider_ids_by_activity,
    }
}

#[derive(Debug)]
struct RankedHistoryCandidate {
    last_modified_unix: i64,
    sequence: usize,
    candidate: HistoryCandidate,
}

impl PartialEq for RankedHistoryCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.last_modified_unix == other.last_modified_unix && self.sequence == other.sequence
    }
}

impl Eq for RankedHistoryCandidate {}

impl PartialOrd for RankedHistoryCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RankedHistoryCandidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.last_modified_unix
            .cmp(&other.last_modified_unix)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

pub(crate) fn entries_page_from_candidates(
    candidates: &[HistoryCandidate],
    cwd: Option<&Path>,
    offset: usize,
    limit: usize,
    mut parse_meta: impl FnMut(&HistoryCandidate) -> Option<HistorySessionMeta>,
    candidate_total: usize,
) -> (Vec<HistoryEntry>, usize) {
    if limit == 0 {
        return (Vec::new(), candidate_total);
    }

    let mut entries = Vec::new();
    let mut parsed_sessions = 0usize;
    let mut exhausted = true;
    for candidate in candidates {
        let Some(meta) = parse_meta(candidate) else {
            continue;
        };
        if !meta.matches_cwd(cwd) {
            continue;
        }
        if parsed_sessions >= offset {
            entries.push(meta.to_entry(candidate));
            if entries.len() == limit {
                exhausted = false;
                break;
            }
        }
        parsed_sessions = parsed_sessions.saturating_add(1);
    }
    let total = if exhausted {
        parsed_sessions
    } else {
        candidate_total
    };
    (entries, total)
}

pub(crate) fn collect_candidates(
    provider: HistoryProviderDescriptor,
    mut emit: impl FnMut(HistoryCandidate),
) -> usize {
    let provider_id = provider.id();
    let roots = provider.default_roots();
    let roots_len = roots.len();
    let roots_log = format_roots(&roots);
    let mut sources_len = 0usize;
    let discovered = provider.discover_sources_into(&mut |src| {
        sources_len = sources_len.saturating_add(1);
        emit(HistoryCandidate::from_source(provider, src));
    });
    match discovered {
        Ok(()) => {}
        Err(err) => {
            warn!(
                target: "lucarne::history_discovery",
                provider = provider_id,
                roots = roots_len,
                root_paths = %roots_log,
                error = %err,
                "history candidate discovery failed"
            );
            return 0;
        }
    }
    info!(
        target: "lucarne::history",
        provider = provider_id,
        roots = roots_len,
        root_paths = %roots_log,
        sources = sources_len,
        candidates = sources_len,
        "history candidate summary"
    );
    sources_len
}

pub(crate) struct WorkspaceAccumulator {
    cwd: PathBuf,
    provider_ids: Vec<&'static str>,
    session_count: usize,
    last_active: Option<SystemTime>,
}

impl WorkspaceAccumulator {
    fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            provider_ids: Vec::new(),
            session_count: 0,
            last_active: None,
        }
    }

    fn record(
        &mut self,
        provider_id: &'static str,
        session_count: usize,
        last_active: Option<SystemTime>,
    ) {
        if !self.provider_ids.contains(&provider_id) {
            self.provider_ids.push(provider_id);
        }
        self.session_count += session_count;
        self.last_active = max_system_time(self.last_active, last_active);
    }

    fn finish(self) -> HistoryWorkspace {
        let last_active_unix = self
            .last_active
            .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);
        HistoryWorkspace {
            display_name: workspace_display_name(&self.cwd),
            cwd: self.cwd,
            provider_ids: self.provider_ids,
            session_count: self.session_count,
            last_active_unix,
            last_active_display: format_unix(last_active_unix),
        }
    }
}

fn max_system_time(left: Option<SystemTime>, right: Option<SystemTime>) -> Option<SystemTime> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

fn workspace_display_name(cwd: &Path) -> String {
    cwd.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map_or_else(|| cwd.display().to_string(), str::to_string)
}

fn normalize_history_providers(provider_ids: &[&str]) -> Vec<HistoryProviderDescriptor> {
    let mut seen = HashSet::new();
    let mut providers = Vec::new();
    for id in provider_ids {
        let Some(provider) = find_history_provider(*id) else {
            continue;
        };
        if seen.insert(provider.id()) {
            providers.push(provider);
        }
    }
    providers
}

pub(crate) fn parse_candidate_meta(
    candidate: &HistoryCandidate,
) -> Result<HistorySessionMeta, EntryParseError> {
    try_parse_candidate_meta(candidate)
}

const WORKSPACE_SESSION_DENSITY_WEIGHT_SECONDS: i64 = 6 * 60 * 60;

pub(crate) fn finish_workspace_accumulators(
    workspaces: BTreeMap<PathBuf, WorkspaceAccumulator>,
) -> Vec<HistoryWorkspace> {
    let mut entries = workspaces
        .into_values()
        .map(WorkspaceAccumulator::finish)
        .collect::<Vec<_>>();
    sort_workspaces_by_activity_and_density(&mut entries);
    entries
}

pub(crate) fn record_workspace_meta(
    workspaces: &mut BTreeMap<PathBuf, WorkspaceAccumulator>,
    candidate: &HistoryCandidate,
    meta: &HistorySessionMeta,
) {
    let Some(cwd) = meta.cwd.clone() else {
        return;
    };
    let last_active = (candidate.last_modified_unix > 0).then(|| {
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(candidate.last_modified_unix as u64)
    });
    workspaces
        .entry(cwd.clone())
        .or_insert_with(|| WorkspaceAccumulator::new(cwd))
        .record(candidate.provider_id, 1, last_active);
}

fn sort_workspaces_by_activity_and_density(entries: &mut [HistoryWorkspace]) {
    entries.sort_by(|a, b| {
        workspace_rank_score(b)
            .cmp(&workspace_rank_score(a))
            .then_with(|| b.last_active_unix.cmp(&a.last_active_unix))
            .then_with(|| b.session_count.cmp(&a.session_count))
            .then_with(|| a.display_name.cmp(&b.display_name))
            .then_with(|| a.cwd.cmp(&b.cwd))
    });
}

fn workspace_rank_score(workspace: &HistoryWorkspace) -> i64 {
    workspace.last_active_unix.saturating_add(
        session_density_units(workspace.session_count)
            .saturating_mul(WORKSPACE_SESSION_DENSITY_WEIGHT_SECONDS),
    )
}

fn session_density_units(session_count: usize) -> i64 {
    let mut value = session_count.saturating_add(1);
    let mut units = 0;
    while value > 1 {
        value >>= 1;
        units += 1;
    }
    units
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScanStats {
    roots: usize,
    candidates: usize,
    parsed: usize,
    read_failures: usize,
    parse_failures: usize,
}

impl ScanStats {
    fn new(roots: usize, candidates: usize) -> Self {
        Self {
            roots,
            candidates,
            parsed: 0,
            read_failures: 0,
            parse_failures: 0,
        }
    }

    fn record_parsed(&mut self) {
        self.parsed += 1;
    }

    fn record_parse_failure(&mut self) {
        self.parse_failures += 1;
    }

    fn record_failure(&mut self, err: &EntryParseError) {
        match err {
            EntryParseError::Parse { .. } => self.record_parse_failure(),
        }
    }

    fn skipped(&self) -> usize {
        self.read_failures + self.parse_failures
    }
}

#[derive(Debug)]

pub(crate) enum EntryParseError {
    Parse { message: String },
}

impl EntryParseError {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            EntryParseError::Parse { .. } => "parse",
        }
    }

    pub(crate) fn message(&self) -> &str {
        match self {
            EntryParseError::Parse { message } => message,
        }
    }
}

fn try_parse_candidate_meta(
    candidate: &HistoryCandidate,
) -> Result<HistorySessionMeta, EntryParseError> {
    let parsed_meta = match &candidate.source {
        Some(source) => candidate.provider.parse_source_meta(source),
        #[cfg(test)]
        None => candidate.provider.parse_file_meta(candidate.path.clone()),
        #[cfg(not(test))]
        None => unreachable!("test-only history candidates are not constructed in production"),
    };
    let meta = match parsed_meta {
        Ok(meta) => meta,
        Err(err) => {
            return Err(EntryParseError::Parse {
                message: err.to_string(),
            });
        }
    };
    let cwd = meta
        .cwd
        .as_deref()
        .filter(|cwd| !cwd.trim().is_empty())
        .map(PathBuf::from);
    let session_id = meta.session_id.as_deref().unwrap_or("").to_string();
    let title = meta
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string);
    let created_at = meta.created_at.as_deref().map(str::to_string);
    let updated_at = meta.updated_at.as_deref().map(str::to_string);
    Ok(HistorySessionMeta {
        session_id,
        title,
        cwd,
        created_at,
        updated_at,
    })
}

fn history_transcript_selection() -> ParseSelection {
    ParseSelection::empty().with_meta().with_messages()
}

fn format_roots(roots: &[PathBuf]) -> String {
    if roots.is_empty() {
        return "(none)".into();
    }
    roots
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn first_line_snippet(text: &str, max: usize) -> String {
    let first = text.lines().next().unwrap_or(text).trim();
    if first.chars().count() > max {
        let mut it = first.chars();
        let truncated: String = it.by_ref().take(max).collect();
        format!("{truncated}…")
    } else {
        first.to_string()
    }
}

fn parse_rfc3339_unix(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp())
}

fn format_unix(unix: i64) -> String {
    crate::time_display::format_last_active_display(unix)
}

#[cfg(test)]
mod tests {
    use super::render::visible_turns;
    use super::*;
    use agent_sessions::agent_session::{Actor, Body, Session as UniSession};
    use smol_str::SmolStr;
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};

    fn test_entries_page(
        candidates: &[HistoryCandidate],
        cwd: Option<&Path>,
        offset: usize,
        limit: usize,
    ) -> (Vec<HistoryEntry>, usize) {
        entries_page_from_candidates(
            candidates,
            cwd,
            offset,
            limit,
            |candidate| parse_candidate_meta(candidate).ok(),
            candidates.len(),
        )
    }

    #[test]
    fn scan_stats_counts_candidates_successes_and_failures() {
        let mut stats = ScanStats::new(2, 5);
        stats.record_parsed();
        stats.record_parse_failure();

        assert_eq!(stats.roots, 2);
        assert_eq!(stats.candidates, 5);
        assert_eq!(stats.parsed, 1);
        assert_eq!(stats.read_failures, 0);
        assert_eq!(stats.parse_failures, 1);
        assert_eq!(stats.skipped(), 1);
    }

    #[test]
    fn history_session_meta_formats_last_active_display_short_local() {
        let timestamp = "2026-05-18T05:43:59.985Z";
        let unix = parse_rfc3339_unix(timestamp).expect("timestamp");
        let candidate = HistoryCandidate::new("codex", PathBuf::from("/tmp/session.jsonl"), 0);
        let meta = HistorySessionMeta {
            session_id: "session".into(),
            title: None,
            cwd: None,
            created_at: None,
            updated_at: Some(timestamp.into()),
        };

        let (last_active_unix, display) = meta.last_active(&candidate);

        assert_eq!(last_active_unix, unix);
        assert_eq!(
            display,
            crate::time_display::format_last_active_display(unix)
        );
        assert!(!display.contains("2026"));
        assert!(!display.contains('T'));
        assert!(!display.ends_with('Z'));
    }

    #[test]
    fn scan_stats_formats_roots_for_one_log_line() {
        let roots = vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")];

        assert_eq!(format_roots(&roots), "/tmp/a, /tmp/b");
    }

    #[test]
    fn ranked_candidates_include_pi_sessions() {
        let home = test_temp_dir("history-pi-home");
        let session_dir = home.join(".pi/agent/sessions/--tmp-pi-project--");
        std::fs::create_dir_all(&session_dir).unwrap();
        let session_path = session_dir.join("2026-05-11T09-47-45-778Z_pi-session.jsonl");
        std::fs::write(
            &session_path,
            r#"{"type":"session","version":3,"id":"pi-session","timestamp":"2026-05-11T09:47:45.778Z","cwd":"/tmp/pi-project"}"#,
        )
        .unwrap();

        {
            let _guard = env_lock().lock().unwrap();
            let _env = EnvGuard::set(&[("HOME", home.as_os_str().to_os_string())]);
            let candidates = test_ranked_candidates_for_provider_ids(&["pi"]);

            assert_eq!(candidates.len(), 1);
            assert_eq!(candidates[0].provider_id(), "pi");
            assert_eq!(candidates[0].path, session_path);
        }

        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn ranked_candidates_exclude_pi_delegated_subagent_fork_sessions() {
        let home = test_temp_dir("history-pi-subagent-fork-home");
        let session_dir = home.join(".pi/agent/sessions/--tmp-pi-project--");
        std::fs::create_dir_all(&session_dir).unwrap();
        let primary = session_dir.join("2026-05-10T11-59-22-505Z_parent.jsonl");
        std::fs::write(&primary, "{}\n").unwrap();
        let subagent = session_dir.join("2026-05-11T00-49-47-936Z_child.jsonl");
        std::fs::write(
            &subagent,
            [
                r#"{"type":"session","version":3,"id":"child","timestamp":"2026-05-11T00:49:47.936Z","cwd":"/tmp/pi-project","parentSession":"/tmp/parent.jsonl"}"#,
                r#"{"type":"message","id":"old","timestamp":"2026-05-10T11:59:25.404Z","message":{"role":"user","content":[{"type":"text","text":"parent title should not leak"}]}}"#,
                r#"{"type":"message","id":"new","timestamp":"2026-05-11T00:49:51.117Z","message":{"role":"user","content":[{"type":"text","text":"Task: You are a delegated subagent running from a fork of the parent session. Treat the inherited conversation as reference-only context, not a live thread to continue.\n\nTask:\nImplement the child task."}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        {
            let _guard = env_lock().lock().unwrap();
            let _env = EnvGuard::set(&[("HOME", home.as_os_str().to_os_string())]);
            let candidates = test_ranked_candidates_for_provider_ids(&["pi"]);

            assert!(candidates.iter().any(|candidate| candidate.path == primary));
            assert!(!candidates
                .iter()
                .any(|candidate| candidate.path == subagent));
        }

        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn metadata_page_uses_pi_first_user_message_as_summary() {
        let root = test_temp_dir("history-pi-summary");
        let path = root.join("pi.jsonl");
        std::fs::write(
            &path,
            [
                r#"{"type":"session","version":3,"id":"pi-summary","timestamp":"2026-05-11T09:47:45.778Z","cwd":"/tmp/pi-project"}"#,
                r#"{"type":"message","id":"u1","timestamp":"2026-05-11T09:47:46.778Z","message":{"role":"user","content":[{"type":"text","text":"pi title prompt\nsecond line"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let candidates = vec![HistoryCandidate::new("pi", path, 30)];
        let (page, total) = test_entries_page(&candidates, None, 0, 1);

        assert_eq!(total, 1);
        assert_eq!(page[0].summary, "pi title prompt");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn metadata_page_uses_pi_fork_user_message_after_session_start() {
        let root = test_temp_dir("history-pi-fork-summary");
        let path = root.join("pi-fork.jsonl");
        let mut lines = vec![
            r#"{"type":"session","version":3,"id":"pi-fork","timestamp":"2026-05-11T00:49:47.936Z","cwd":"/tmp/pi-project","parentSession":"/tmp/parent.jsonl"}"#,
            r#"{"type":"message","id":"old","timestamp":"2026-05-10T11:59:25.404Z","message":{"role":"user","content":[{"type":"text","text":"parent title should not leak"}]}}"#,
        ];
        lines.extend(
            std::iter::repeat(
                r#"{"type":"message","id":"old-assistant","timestamp":"2026-05-10T12:00:00.000Z","message":{"role":"assistant","content":[{"type":"text","text":"inherited"}]}}"#,
            )
            .take(70),
        );
        lines.push(
            r#"{"type":"message","id":"new","timestamp":"2026-05-11T00:49:51.117Z","message":{"role":"user","content":[{"type":"text","text":"fork title prompt"}]}}"#,
        );
        std::fs::write(&path, lines.join("\n")).unwrap();
        let candidates = vec![HistoryCandidate::new("pi", path, 30)];
        let (page, total) = test_entries_page(&candidates, None, 0, 1);

        assert_eq!(total, 1);
        assert_eq!(page[0].summary, "fork title prompt");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn metadata_page_limits_and_offsets() {
        let root1 = test_temp_dir("history-page-1");
        let root2 = test_temp_dir("history-page-2");
        let path1 = root1.join("codex.jsonl");
        let path2 = root2.join("codex.jsonl");
        std::fs::write(
            &path1,
            r#"{"timestamp":"2026-04-16T00:00:00.000Z","type":"session_meta","payload":{"session_id":"sess-a","cwd":"/tmp/project"}}"#,
        ).unwrap();
        std::fs::write(
            &path2,
            r#"{"timestamp":"2026-04-16T00:00:01.000Z","type":"session_meta","payload":{"session_id":"sess-b","cwd":"/tmp/project"}}"#,
        ).unwrap();
        let candidates = vec![
            HistoryCandidate::new("codex", path1, 30),
            HistoryCandidate::new("codex", path2, 20),
        ];
        let (page, total) = test_entries_page(&candidates, None, 0, 1);
        assert_eq!(total, 2);
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].session_id, "sess-a");

        std::fs::remove_dir_all(root1).unwrap();
        std::fs::remove_dir_all(root2).unwrap();
    }

    #[test]
    fn metadata_page_parses_history_entries_with_streaming_metadata() {
        let root = test_temp_dir("history-selected");
        let path = root.join("codex.jsonl");
        std::fs::write(
            &path,
            [
                r#"{"timestamp":"2026-04-16T00:00:00.000Z","type":"session_meta","payload":{"session_id":"sess-selected","cwd":"/tmp/project"}}"#,
                r#"{"timestamp":"2026-04-16T00:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello selected history"}]}}"#,
                r#"{"timestamp":"2026-04-16T00:00:02.000Z","type":"response_item","payload":{"type":"function_call_output","output":"missing call id would fail if full-parsed"}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let candidates = vec![HistoryCandidate::new("codex", path, 30)];
        let (page, total) = test_entries_page(&candidates, None, 0, 1);

        assert_eq!(total, 1);
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].session_id, "sess-selected");
        // Malformed events should not crash the streaming metadata parser
        assert!(!page[0].summary.is_empty());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_order_blends_recent_activity_with_session_density() {
        let now = 1_700_000_000;
        let mut workspaces = BTreeMap::<PathBuf, WorkspaceAccumulator>::new();
        record_workspace_session(&mut workspaces, "codex", "/tmp/recent-sparse", now);
        for _ in 0..64 {
            record_workspace_session(
                &mut workspaces,
                "codex",
                "/tmp/dense-slightly-older",
                now - 10 * 3_600,
            );
        }
        for _ in 0..512 {
            record_workspace_session(
                &mut workspaces,
                "codex",
                "/tmp/stale-dense",
                now - 30 * 86_400,
            );
        }

        let mut workspaces = workspaces
            .into_values()
            .map(WorkspaceAccumulator::finish)
            .collect::<Vec<_>>();
        sort_workspaces_by_activity_and_density(&mut workspaces);

        assert_eq!(
            workspaces
                .iter()
                .map(|workspace| workspace.cwd.as_path())
                .collect::<Vec<_>>(),
            vec![
                Path::new("/tmp/dense-slightly-older"),
                Path::new("/tmp/recent-sparse"),
                Path::new("/tmp/stale-dense"),
            ]
        );
        assert_eq!(workspaces[0].session_count, 64);
        assert_eq!(workspaces[1].session_count, 1);
        assert_eq!(workspaces[2].session_count, 512);
    }

    #[test]
    fn metadata_pages_without_full_entry_parse() {
        let root = test_temp_dir("history-metadata-offset");
        let bad_path = root.join("bad.jsonl");
        let good_path = root.join("good.jsonl");
        std::fs::write(
            &bad_path,
            [
                r#"{"timestamp":"2026-04-16T00:00:00.000Z","type":"session_meta","payload":{"session_id":"sess-bad","cwd":"/tmp/project","timestamp":"2026-04-16T00:00:00.000Z"}}"#,
                "not-json",
            ]
            .join("\n"),
        )
        .unwrap();
        std::fs::write(
            &good_path,
            [
                r#"{"timestamp":"2026-04-16T00:00:00.000Z","type":"session_meta","payload":{"session_id":"sess-good","cwd":"/tmp/project"}}"#,
                r#"{"timestamp":"2026-04-16T00:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"good history"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let candidates = vec![
            HistoryCandidate::new("codex", bad_path, 30),
            HistoryCandidate::new("codex", good_path, 20),
        ];
        let (first_page, total) =
            test_entries_page(&candidates, Some(Path::new("/tmp/project")), 0, 1);
        let (second_page, _) =
            test_entries_page(&candidates, Some(Path::new("/tmp/project")), 1, 1);

        assert_eq!(total, 2);
        assert_eq!(first_page.len(), 1);
        assert_eq!(first_page[0].session_id, "sess-bad");
        assert_eq!(first_page[0].last_active_unix, 30);
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].session_id, "sess-good");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn requested_history_providers_are_deduped_and_filtered() {
        let providers =
            normalize_history_providers(&["codex", "unknown", "claude", "codex", "copilot", "pi"]);

        assert_eq!(
            providers
                .iter()
                .map(|provider| provider.id())
                .collect::<Vec<_>>(),
            vec!["codex", "claude", "copilot", "pi"]
        );
    }

    #[test]
    fn history_provider_registry_is_provider_owned() {
        let providers = history_providers();

        assert!(providers.iter().any(|provider| provider.id() == "codex"));
        assert!(providers.iter().any(|provider| provider.id() == "pi"));
    }

    #[test]
    fn codex_provider_owns_transcript_user_visibility_rules() {
        let provider = find_history_provider("codex").expect("codex provider");
        assert!(!provider.is_transcript_user_text_visible(
            "# AGENTS.md instructions for /tmp/project\n\n<INSTRUCTIONS>\n..."
        ));
        assert!(!provider.is_transcript_user_text_visible(
            "<turn_aborted>\nThe user interrupted the previous turn on purpose.\n</turn_aborted>"
        ));
        assert!(provider
            .is_transcript_user_text_visible("每次启动bot都应该把commands发给telegram同步一下"));
    }

    #[test]
    fn transcript_recent_batch_filters_preamble_pairs_turns_and_keeps_orphan_user() {
        let session = test_session(vec![
            user_prompt_at(
                "preamble",
                "# AGENTS.md instructions for /tmp/project\n\n<INSTRUCTIONS>\n...",
                Some("2026-05-04T00:00:00Z".into()),
            ),
            user_prompt_at("u1", "first", Some("2026-05-04T00:00:01Z".into())),
            assistant_response_at("a1", "first partial", Some("2026-05-04T00:00:02Z".into())),
            assistant_response_at("a2", "first final", Some("2026-05-04T00:00:03Z".into())),
            user_prompt_at("u2", "second", Some("2026-05-04T00:00:04Z".into())),
            assistant_response_at("a3", "second answer", Some("2026-05-04T00:00:05Z".into())),
            user_prompt_at("u3", "third orphan", Some("2026-05-04T00:00:06Z".into())),
        ]);

        let transcript = codex_test_transcript(&session, 10);

        assert_eq!(transcript.turns.len(), 3);
        assert_eq!(transcript.turns[0].user.text, "first");
        assert_eq!(
            transcript.turns[0].assistant.as_ref().unwrap().text,
            "first final"
        );
        assert_eq!(transcript.turns[1].user.text, "second");
        assert_eq!(transcript.turns[2].user.text, "third orphan");
        assert!(transcript.turns[2].assistant.is_none());
        assert!(!transcript.has_older);
    }

    #[test]
    fn transcript_filters_turn_aborted_control_markers() {
        let session = test_session(vec![
            user_prompt_at(
                "aborted",
                "<turn_aborted>\nThe user interrupted the previous turn on purpose.\n</turn_aborted>",
                Some("2026-05-04T00:00:00Z".into()),
            ),
            user_prompt_at(
                "u1",
                "real user prompt",
                Some("2026-05-04T00:00:01Z".into()),
            ),
            assistant_response_at("a1", "real answer", Some("2026-05-04T00:00:02Z".into())),
        ]);

        let transcript = codex_test_transcript(&session, 10);

        assert_eq!(transcript.turns.len(), 1);
        assert_eq!(transcript.turns[0].user.id, "u1");
        assert_eq!(transcript.turns[0].user.text, "real user prompt");
    }

    #[test]
    fn transcript_recent_batch_returns_newest_limit_oldest_to_newest_with_older_cursor() {
        let session = test_session(
            (1..=12)
                .flat_map(|idx| {
                    vec![
                        user_prompt_at(
                            format!("u{idx}"),
                            format!("user {idx}"),
                            Some(format!("2026-05-04T00:{idx:02}:00Z")),
                        ),
                        assistant_response_at(
                            format!("a{idx}"),
                            format!("assistant {idx}"),
                            Some(format!("2026-05-04T00:{idx:02}:01Z")),
                        ),
                    ]
                })
                .collect(),
        );

        let transcript = codex_test_transcript(&session, 10);

        assert_eq!(transcript.turns.first().unwrap().user.text, "user 3");
        assert_eq!(transcript.turns.last().unwrap().user.text, "user 12");
        assert!(transcript.has_older);
        assert!(transcript.older_cursor.is_some());
    }

    #[test]
    fn transcript_dedupes_adjacent_unanswered_duplicate_user_carriers() {
        let session = test_session(vec![
            user_prompt_at("u1-response", "hello", Some("2026-05-04T00:00:00Z".into())),
            user_prompt_at("u1-event", "hello", Some("2026-05-04T00:00:00.001Z".into())),
            assistant_response_at("a1", "answer", Some("2026-05-04T00:00:01Z".into())),
        ]);

        let transcript = codex_test_transcript(&session, 10);

        assert_eq!(transcript.turns.len(), 1);
        assert_eq!(transcript.turns[0].user.id, "u1-response");
        assert_eq!(transcript.turns[0].user.text, "hello");
        assert_eq!(
            transcript.turns[0]
                .assistant
                .as_ref()
                .map(|msg| msg.text.as_str()),
            Some("answer")
        );
    }

    #[test]
    fn transcript_system_prompt_does_not_reassign_followup_assistant_to_previous_user() {
        let session = test_session(vec![
            user_prompt_at("u1", "/fork", Some("2026-05-05T07:20:25Z".into())),
            assistant_response_at(
                "a1",
                "cannot execute session fork",
                Some("2026-05-05T07:20:36Z".into()),
            ),
            system_prompt_at(
                "dev1",
                "Continue working toward the active thread goal.",
                Some("2026-05-05T07:31:11Z".into()),
            ),
            assistant_response_at(
                "a2",
                "project structure audit complete",
                Some("2026-05-05T07:31:42Z".into()),
            ),
        ]);

        let transcript = codex_test_transcript(&session, 10);

        assert_eq!(transcript.turns.len(), 1);
        assert_eq!(transcript.turns[0].user.text, "/fork");
        assert_eq!(
            transcript.turns[0]
                .assistant
                .as_ref()
                .map(|msg| msg.text.as_str()),
            Some("cannot execute session fork")
        );
    }

    #[test]
    fn transcript_codex_current_dedupes_response_item_and_user_message_event() {
        let root = test_temp_dir("history-codex-duplicate-user-carrier");
        let path = root.join("codex.jsonl");
        std::fs::write(
            &path,
            [
                r#"{"timestamp":"2026-05-04T00:00:00.000Z","type":"session_meta","payload":{"id":"sess-duplicate","cwd":"/tmp/project","originator":"lucarne"}}"#,
                r#"{"timestamp":"2026-05-04T00:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello duplicate carrier"}]}}"#,
                r#"{"timestamp":"2026-05-04T00:00:01.001Z","type":"event_msg","payload":{"type":"user_message","message":"hello duplicate carrier","images":[],"local_images":[],"text_elements":[]}}"#,
                r#"{"timestamp":"2026-05-04T00:00:02.000Z","type":"event_msg","payload":{"type":"agent_message","message":"answer","phase":"final_answer","memory_citation":null}}"#,
                r#"{"timestamp":"2026-05-04T00:00:02.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"answer"}],"phase":"final_answer"}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let entry = HistoryEntry {
            provider_id: "codex",
            session_id: "sess-duplicate".into(),
            session_path: path,
            cwd: None,
            summary: String::new(),
            last_active_unix: 0,
            last_active_display: String::new(),
        };

        let transcript = history_transcript_for_entry(&entry, 10, None).expect("transcript");

        assert_eq!(transcript.turns.len(), 1);
        assert_eq!(transcript.turns[0].user.text, "hello duplicate carrier");
        assert_eq!(
            transcript.turns[0]
                .assistant
                .as_ref()
                .map(|msg| msg.text.as_str()),
            Some("answer")
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transcript_codex_user_message_raw_images_are_exposed() {
        let root = test_temp_dir("history-codex-user-message-images");
        let path = root.join("codex.jsonl");
        std::fs::write(
            &path,
            [
                r#"{"timestamp":"2026-05-04T00:00:00.000Z","type":"session_meta","payload":{"id":"sess-images","cwd":"/tmp/project","originator":"lucarne"}}"#,
                r#"{"timestamp":"2026-05-04T00:00:01.000Z","type":"event_msg","payload":{"type":"user_message","message":"see image","images":["data:image/png;base64,AQID"],"local_images":["/tmp/local-history-image.png"],"text_elements":[]}}"#,
                r#"{"timestamp":"2026-05-04T00:00:02.000Z","type":"event_msg","payload":{"type":"agent_message","message":"answer","phase":"final_answer","memory_citation":null}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let entry = HistoryEntry {
            provider_id: "codex",
            session_id: "sess-images".into(),
            session_path: path,
            cwd: None,
            summary: String::new(),
            last_active_unix: 0,
            last_active_display: String::new(),
        };

        let transcript = history_transcript_for_entry(&entry, 10, None).expect("transcript");

        assert_eq!(
            transcript.turns[0]
                .user
                .images
                .iter()
                .map(|image| image.image_url.as_str())
                .collect::<Vec<_>>(),
            vec!["data:image/png;base64,AQID", "/tmp/local-history-image.png"]
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transcript_pi_dispatch_reads_visible_turns() {
        let root = test_temp_dir("history-pi-transcript");
        let path = root.join("pi.jsonl");
        std::fs::write(
            &path,
            [
                r#"{"type":"session","version":3,"id":"pi-transcript","timestamp":"2026-05-11T09:47:45.778Z","cwd":"/tmp/pi-project"}"#,
                r#"{"type":"message","id":"u1","timestamp":"2026-05-11T09:47:46.778Z","message":{"role":"user","content":[{"type":"text","text":"还好吗"}]}}"#,
                r#"{"type":"message","id":"a1","parentId":"u1","timestamp":"2026-05-11T09:47:47.778Z","message":{"role":"assistant","model":"deepseek-v4-pro","stopReason":"stop","content":[{"type":"thinking","thinking":"internal"},{"type":"text","text":"当前状态没问题。"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let entry = HistoryEntry {
            provider_id: "pi",
            session_id: "pi-transcript".into(),
            session_path: path,
            cwd: None,
            summary: String::new(),
            last_active_unix: 0,
            last_active_display: String::new(),
        };

        let transcript = history_transcript_for_entry(&entry, 10, None).expect("pi transcript");

        assert_eq!(transcript.provider_id, "pi");
        assert_eq!(transcript.session_id, "pi-transcript");
        assert_eq!(transcript.turns.len(), 1);
        assert_eq!(transcript.turns[0].user.text, "还好吗");
        assert_eq!(
            transcript.turns[0]
                .assistant
                .as_ref()
                .map(|msg| msg.text.as_str()),
            Some("当前状态没问题。")
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transcript_pi_tail_reads_past_large_tool_result_to_reach_visible_limit() {
        let root = test_temp_dir("history-pi-large-tool-gap");
        let path = root.join("pi.jsonl");
        let large_tool_output = "x".repeat(600 * 1024);
        std::fs::write(
            &path,
            [
                r#"{"type":"session","version":3,"id":"pi-large-gap","timestamp":"2026-05-11T09:47:45.778Z","cwd":"/tmp/pi-project"}"#.to_string(),
                r#"{"type":"message","id":"u1","timestamp":"2026-05-11T09:47:46.778Z","message":{"role":"user","content":[{"type":"text","text":"older user"}]}}"#.to_string(),
                r#"{"type":"message","id":"a1","parentId":"u1","timestamp":"2026-05-11T09:47:47.778Z","message":{"role":"assistant","content":[{"type":"text","text":"older assistant"}]}}"#.to_string(),
                format!(
                    r#"{{"type":"message","id":"tool1","timestamp":"2026-05-11T09:48:00.000Z","message":{{"role":"toolResult","toolName":"read","content":[{{"type":"text","text":"{large_tool_output}"}}],"isError":false}}}}"#
                ),
                r#"{"type":"message","id":"u2","timestamp":"2026-05-11T09:49:46.778Z","message":{"role":"user","content":[{"type":"text","text":"latest user"}]}}"#.to_string(),
            ]
            .join("\n"),
        )
        .unwrap();
        let entry = HistoryEntry {
            provider_id: "pi",
            session_id: "pi-large-gap".into(),
            session_path: path,
            cwd: None,
            summary: String::new(),
            last_active_unix: 0,
            last_active_display: String::new(),
        };

        let transcript = history_transcript_for_entry(&entry, 10, None).expect("pi transcript");

        assert_eq!(
            transcript
                .turns
                .iter()
                .map(|turn| turn.user.text.as_str())
                .collect::<Vec<_>>(),
            vec!["older user", "latest user"]
        );
        assert!(!transcript.has_older);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transcript_provider_dispatch_reads_visible_turns_from_current_fixtures() {
        let cases = [
            ProviderFixtureCase {
                provider_id: "codex",
                fixture: "codex/codex_current_sample.jsonl",
                session_id: "019d5294-7fd5-7e21-bcca-32362218c185",
                user_text: "hello",
                assistant_text: "Hi there.",
            },
            ProviderFixtureCase {
                provider_id: "claude",
                fixture: "claude/claude_current_sample.jsonl",
                session_id: "63679569-7045-45ba-bfef-cad8b1045769",
                user_text: "hello",
                assistant_text: "Hey! What can I help you with today?",
            },
            ProviderFixtureCase {
                provider_id: "copilot",
                fixture: "copilot/events.jsonl",
                session_id: "copilot-fixture",
                user_text: "Fix the bug in main.rs",
                assistant_text: "I will inspect main.rs first.",
            },
        ];

        for case in cases {
            let entry = HistoryEntry {
                provider_id: case.provider_id,
                session_id: case.session_id.into(),
                session_path: fixture_path(case.fixture),
                cwd: None,
                summary: String::new(),
                last_active_unix: 0,
                last_active_display: String::new(),
            };

            let transcript =
                history_transcript_for_entry(&entry, 10, None).expect(case.provider_id);

            assert_eq!(transcript.provider_id, case.provider_id);
            assert_eq!(transcript.session_id, case.session_id);
            assert_eq!(transcript.turns.len(), 1, "{case:?}");
            assert_eq!(transcript.turns[0].user.text, case.user_text, "{case:?}");
            assert_eq!(
                transcript.turns[0]
                    .assistant
                    .as_ref()
                    .map(|msg| msg.text.as_str()),
                Some(case.assistant_text),
                "{case:?}"
            );
            assert!(!transcript.has_older, "{case:?}");
        }
    }

    #[test]
    fn transcript_json_document_provider_is_not_replayed_by_bounded_jsonl_hot_path() {
        let entry = HistoryEntry {
            provider_id: "gemini",
            session_id: "e1c7ee9f-90f8-41d5-815c-5d9cf9f83963".into(),
            session_path: fixture_path("gemini/session-sample.json"),
            cwd: None,
            summary: String::new(),
            last_active_unix: 0,
            last_active_display: String::new(),
        };

        let err = history_transcript_for_entry(&entry, 10, None)
            .expect_err("json document history replay should be unsupported");

        assert!(matches!(
            err,
            HistoryTranscriptError::UnsupportedProvider { provider_id } if provider_id == "gemini"
        ));
    }

    #[derive(Debug, Clone, Copy)]
    struct ProviderFixtureCase {
        provider_id: &'static str,
        fixture: &'static str,
        session_id: &'static str,
        user_text: &'static str,
        assistant_text: &'static str,
    }

    fn record_workspace_session(
        workspaces: &mut BTreeMap<PathBuf, WorkspaceAccumulator>,
        provider_id: &'static str,
        cwd: &'static str,
        last_active_unix: i64,
    ) {
        let cwd = PathBuf::from(cwd);
        let last_active =
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(last_active_unix as u64);
        workspaces
            .entry(cwd.clone())
            .or_insert_with(|| WorkspaceAccumulator::new(cwd))
            .record(provider_id, 1, Some(last_active));
    }

    fn user_prompt_at(
        id: impl Into<String>,
        text: impl Into<String>,
        timestamp: Option<String>,
    ) -> agent_sessions::agent_session::Event {
        let mut event = agent_sessions::agent_session::event(
            Actor::User,
            Body::Prompt(agent_sessions::agent_session::Prompt {
                text: Some(text.into().into()),
                blocks: Box::new([]),
            }),
            timestamp.map(SmolStr::from),
        );
        event.id = Some(id.into().into());
        event
    }

    fn system_prompt_at(
        id: impl Into<String>,
        text: impl Into<String>,
        timestamp: Option<String>,
    ) -> agent_sessions::agent_session::Event {
        let mut event = agent_sessions::agent_session::event(
            Actor::System,
            Body::Prompt(agent_sessions::agent_session::Prompt {
                text: Some(text.into().into()),
                blocks: Box::new([]),
            }),
            timestamp.map(SmolStr::from),
        );
        event.id = Some(id.into().into());
        event
    }

    fn assistant_response_at(
        id: impl Into<String>,
        text: impl Into<String>,
        timestamp: Option<String>,
    ) -> agent_sessions::agent_session::Event {
        let mut event = agent_sessions::agent_session::event(
            Actor::Assistant,
            Body::Response(agent_sessions::agent_session::Response {
                text: Some(text.into().into()),
                blocks: Box::new([]),
                ..Default::default()
            }),
            timestamp.map(SmolStr::from),
        );
        event.id = Some(id.into().into());
        event
    }

    fn test_session(events: Vec<agent_sessions::agent_session::Event>) -> UniSession {
        UniSession {
            agent: agent_sessions::agent_session::AgentKind::new("test"),
            version: agent_sessions::agent_session::VersionKind::new("test-v1"),
            meta: agent_sessions::agent_session::SessionMeta::default(),
            events: events.into_boxed_slice(),
        }
    }

    fn codex_test_transcript(session: &UniSession, limit: usize) -> HistoryTranscript {
        let provider = find_history_provider("codex").expect("codex provider");
        let turns = visible_turns(provider, session);
        let end = turns.len();
        let start = end.saturating_sub(limit);
        let page = if limit == 0 {
            Vec::new()
        } else {
            turns[start..end].to_vec()
        };
        let older_cursor = (start > 0).then(|| HistoryCursor::new("test-history-cursor"));
        HistoryTranscript {
            provider_id: "codex",
            session_id: "sess".into(),
            session_path: PathBuf::from("/tmp/sess.jsonl"),
            turns: page,
            has_older: older_cursor.is_some(),
            older_cursor,
        }
    }

    fn fixture_path(relative: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../agent-sessions/tests/fixtures")
            .join(relative)
    }

    fn test_temp_dir(label: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("lucarne-{label}-{suffix}"));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    struct EnvGuard(Vec<(&'static str, Option<OsString>)>);

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    impl EnvGuard {
        fn set(vars: &[(&'static str, OsString)]) -> Self {
            let old = vars
                .iter()
                .map(|(key, _)| (*key, std::env::var_os(key)))
                .collect();
            for (key, value) in vars {
                std::env::set_var(key, value);
            }
            Self(old)
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.0.drain(..) {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}
