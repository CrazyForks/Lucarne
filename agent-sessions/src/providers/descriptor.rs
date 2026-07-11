use std::{
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

use smol_str::SmolStr;

use crate::{
    DiscoverableProvider, InputMetadata, ParseSelection, Result, agent::AgentProviderSourceEntry,
    agent_session::SessionMeta,
};

#[cfg(feature = "watch")]
type WatchEventTransform = fn(
    Box<[crate::watch::WatchEvent]>,
    &mut crate::watch::state::ProviderWatchState,
) -> Box<[crate::watch::WatchEvent]>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionFileFormat {
    LineDelimitedJson,
    JsonDocument,
}

#[derive(Debug, Clone)]
pub struct AgentProviderSource {
    path: PathBuf,
    entries: Vec<AgentProviderSourceEntry>,
}

impl AgentProviderSource {
    #[must_use]
    pub(crate) fn new(path: PathBuf, entries: Vec<AgentProviderSourceEntry>) -> Self {
        Self { path, entries }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn last_modified_unix(&self) -> i64 {
        self.entries
            .iter()
            .map(|entry| file_mtime_unix(entry.path()))
            .max()
            .unwrap_or(0)
    }
}

fn file_mtime_unix(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|mtime| mtime.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy)]
pub struct AgentProviderDescriptor {
    id: &'static str,
    display_name: &'static str,
    session_file_format: SessionFileFormat,
    default_roots: fn() -> Vec<PathBuf>,
    discover_sources: fn(&mut dyn FnMut(AgentProviderSource)) -> Result<()>,
    parse_source_meta: fn(&AgentProviderSource) -> Result<SessionMeta>,
    parse_agent_session_bytes: fn(Vec<u8>, ParseSelection) -> Result<crate::agent_session::Session>,
    visible_transcript_user_offsets: fn(&[u8], u64) -> Vec<u64>,
    is_transcript_user_text_visible: fn(&str) -> bool,
    #[cfg(feature = "watch")]
    parse_watch_reader: fn(
        &Path,
        &mut dyn std::io::BufRead,
        ParseSelection,
    ) -> Result<crate::watch::provider::ParsedWatchSession>,
    #[cfg(feature = "watch")]
    parse_watch_metadata_reader:
        fn(&Path, &mut dyn std::io::BufRead) -> Result<crate::watch::provider::ParsedWatchSession>,
    #[cfg(feature = "watch")]
    supports_incremental_watch_events: fn() -> bool,
    #[cfg(feature = "watch")]
    needs_watch_state_seed: fn() -> bool,
    #[cfg(feature = "watch")]
    seed_watch_state: fn(&[crate::watch::WatchEvent]) -> crate::watch::state::ProviderWatchState,
    #[cfg(feature = "watch")]
    dedupe_watch_events: WatchEventTransform,
    #[cfg(feature = "watch")]
    normalize_watch_events: WatchEventTransform,
    #[cfg(feature = "watch")]
    includes_candidate_in_history: fn(&Path, &Path) -> bool,
    #[cfg(feature = "watch")]
    discover_session_files_into: fn(&Path, &mut dyn FnMut(&Path) -> bool, &mut dyn FnMut(PathBuf)),
    #[cfg(feature = "watch")]
    initial_watch_directory_depth: fn() -> Option<usize>,
    #[cfg(feature = "watch")]
    changed_watch_directory_depth: fn() -> Option<usize>,
    #[cfg(feature = "watch")]
    includes_watch_directory: fn(&Path, &Path, bool) -> bool,
}

impl PartialEq for AgentProviderDescriptor {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for AgentProviderDescriptor {}

impl Hash for AgentProviderDescriptor {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl std::fmt::Display for AgentProviderDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id)
    }
}

impl AgentProviderDescriptor {
    #[must_use]
    pub fn id(self) -> &'static str {
        self.id
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.id
    }

    #[must_use]
    pub fn display_name(self) -> &'static str {
        self.display_name
    }

    #[must_use]
    pub fn session_file_format(self) -> SessionFileFormat {
        self.session_file_format
    }

    #[must_use]
    pub fn default_roots(self) -> Vec<PathBuf> {
        (self.default_roots)()
    }

    pub fn discover_sources_into(self, emit: &mut dyn FnMut(AgentProviderSource)) -> Result<()> {
        (self.discover_sources)(emit)
    }

    pub fn parse_source_meta(self, source: &AgentProviderSource) -> Result<SessionMeta> {
        (self.parse_source_meta)(source)
    }

    pub fn parse_file_meta(self, path: PathBuf) -> Result<SessionMeta> {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(SmolStr::from);
        let mut entry = AgentProviderSourceEntry::new(path.clone());
        if let Some(name) = name {
            entry = entry.named(name);
        }
        (self.parse_source_meta)(&AgentProviderSource::new(path, vec![entry]))
    }

    pub fn parse_agent_session_bytes(
        self,
        bytes: Vec<u8>,
        selection: ParseSelection,
    ) -> Result<crate::agent_session::Session> {
        (self.parse_agent_session_bytes)(bytes, selection)
    }

    #[must_use]
    pub fn visible_transcript_user_offsets(self, bytes: &[u8], base_offset: u64) -> Vec<u64> {
        (self.visible_transcript_user_offsets)(bytes, base_offset)
    }

    #[must_use]
    pub fn is_transcript_user_text_visible(self, text: &str) -> bool {
        (self.is_transcript_user_text_visible)(text)
    }

    #[cfg(feature = "watch")]
    pub(crate) fn parse_watch_reader(
        self,
        path: &Path,
        reader: &mut dyn std::io::BufRead,
        selection: ParseSelection,
    ) -> Result<crate::watch::provider::ParsedWatchSession> {
        (self.parse_watch_reader)(path, reader, selection)
    }

    #[cfg(feature = "watch")]
    pub(crate) fn parse_watch_metadata_reader(
        self,
        path: &Path,
        reader: &mut dyn std::io::BufRead,
    ) -> Result<crate::watch::provider::ParsedWatchSession> {
        (self.parse_watch_metadata_reader)(path, reader)
    }

    #[cfg(feature = "watch")]
    pub(crate) fn supports_incremental_watch_events(self) -> bool {
        (self.supports_incremental_watch_events)()
    }

    #[cfg(feature = "watch")]
    pub(crate) fn needs_watch_state_seed(self) -> bool {
        (self.needs_watch_state_seed)()
    }

    #[cfg(feature = "watch")]
    pub(crate) fn seed_watch_state(
        self,
        events: &[crate::watch::WatchEvent],
    ) -> crate::watch::state::ProviderWatchState {
        (self.seed_watch_state)(events)
    }

    #[cfg(feature = "watch")]
    pub(crate) fn dedupe_watch_events(
        self,
        events: Box<[crate::watch::WatchEvent]>,
        state: &mut crate::watch::state::ProviderWatchState,
    ) -> Box<[crate::watch::WatchEvent]> {
        (self.dedupe_watch_events)(events, state)
    }

    #[cfg(feature = "watch")]
    pub(crate) fn normalize_watch_events(
        self,
        events: Box<[crate::watch::WatchEvent]>,
        state: &mut crate::watch::state::ProviderWatchState,
    ) -> Box<[crate::watch::WatchEvent]> {
        (self.normalize_watch_events)(events, state)
    }

    #[cfg(feature = "watch")]
    pub(crate) fn includes_candidate_in_history(self, root: &Path, path: &Path) -> bool {
        (self.includes_candidate_in_history)(root, path)
    }

    #[cfg(feature = "watch")]
    pub(crate) fn discover_session_files_into(
        self,
        root: &Path,
        is_recent: &mut dyn FnMut(&Path) -> bool,
        emit: &mut dyn FnMut(PathBuf),
    ) {
        (self.discover_session_files_into)(root, is_recent, emit)
    }

    #[cfg(feature = "watch")]
    pub(crate) fn initial_watch_directory_depth(self) -> Option<usize> {
        (self.initial_watch_directory_depth)()
    }

    #[cfg(feature = "watch")]
    pub(crate) fn changed_watch_directory_depth(self) -> Option<usize> {
        (self.changed_watch_directory_depth)()
    }

    #[cfg(feature = "watch")]
    pub(crate) fn includes_watch_directory(
        self,
        root: &Path,
        path: &Path,
        is_recent: bool,
    ) -> bool {
        (self.includes_watch_directory)(root, path, is_recent)
    }
}

#[must_use]
pub fn agent_providers() -> Vec<AgentProviderDescriptor> {
    vec![
        #[cfg(feature = "codex")]
        descriptor_for::<super::codex::Codex>("Codex", SessionFileFormat::LineDelimitedJson),
        #[cfg(feature = "claude")]
        descriptor_for::<super::claude::Claude>("CC", SessionFileFormat::LineDelimitedJson),
        #[cfg(feature = "gemini")]
        descriptor_for::<super::gemini::Gemini>("Gemini", SessionFileFormat::JsonDocument),
        #[cfg(feature = "copilot")]
        descriptor_for::<super::copilot::Copilot>("Copilot", SessionFileFormat::LineDelimitedJson),
        #[cfg(feature = "pi")]
        descriptor_for::<super::pi::Pi>("Pi", SessionFileFormat::LineDelimitedJson),
        #[cfg(feature = "cursor")]
        descriptor_for::<super::cursor::Cursor>("Cursor", SessionFileFormat::LineDelimitedJson),
        #[cfg(feature = "grok")]
        descriptor_for::<super::grok::Grok>("Grok Build", SessionFileFormat::LineDelimitedJson),
    ]
}

#[must_use]
pub fn agent_provider(provider_id: &str) -> Option<AgentProviderDescriptor> {
    agent_providers()
        .into_iter()
        .find(|provider| provider.id() == provider_id)
}

#[cfg(feature = "watch")]
fn descriptor_for<A>(
    display_name: &'static str,
    session_file_format: SessionFileFormat,
) -> AgentProviderDescriptor
where
    A: DiscoverableProvider + crate::watch::provider::ProviderWatchEvents,
{
    AgentProviderDescriptor {
        id: A::name(),
        display_name,
        session_file_format,
        default_roots: A::default_roots,
        discover_sources: discover_sources::<A>,
        parse_source_meta: parse_source_meta::<A>,
        parse_agent_session_bytes: parse_agent_session_bytes::<A>,
        visible_transcript_user_offsets: A::visible_transcript_user_offsets,
        is_transcript_user_text_visible: A::is_transcript_user_text_visible,
        #[cfg(feature = "watch")]
        parse_watch_reader: parse_watch_reader::<A>,
        #[cfg(feature = "watch")]
        parse_watch_metadata_reader: parse_watch_metadata_reader::<A>,
        #[cfg(feature = "watch")]
        supports_incremental_watch_events: A::supports_incremental_watch_events,
        #[cfg(feature = "watch")]
        needs_watch_state_seed: A::needs_watch_state_seed,
        #[cfg(feature = "watch")]
        seed_watch_state: A::seed_watch_state,
        #[cfg(feature = "watch")]
        dedupe_watch_events: A::dedupe_watch_events,
        #[cfg(feature = "watch")]
        normalize_watch_events: A::normalize_watch_events,
        #[cfg(feature = "watch")]
        includes_candidate_in_history: A::includes_candidate_in_history,
        #[cfg(feature = "watch")]
        discover_session_files_into: discover_session_files_into::<A>,
        #[cfg(feature = "watch")]
        initial_watch_directory_depth: A::initial_watch_directory_depth,
        #[cfg(feature = "watch")]
        changed_watch_directory_depth: A::changed_watch_directory_depth,
        #[cfg(feature = "watch")]
        includes_watch_directory: A::includes_watch_directory,
    }
}

#[cfg(not(feature = "watch"))]
fn descriptor_for<A>(
    display_name: &'static str,
    session_file_format: SessionFileFormat,
) -> AgentProviderDescriptor
where
    A: DiscoverableProvider,
{
    AgentProviderDescriptor {
        id: A::name(),
        display_name,
        session_file_format,
        default_roots: A::default_roots,
        discover_sources: discover_sources::<A>,
        parse_source_meta: parse_source_meta::<A>,
        parse_agent_session_bytes: parse_agent_session_bytes::<A>,
        visible_transcript_user_offsets: A::visible_transcript_user_offsets,
        is_transcript_user_text_visible: A::is_transcript_user_text_visible,
    }
}

fn discover_sources<A>(emit: &mut dyn FnMut(AgentProviderSource)) -> Result<()>
where
    A: DiscoverableProvider,
{
    A::discover_in(A::default_roots(), emit)
}

fn parse_source_meta<A>(source: &AgentProviderSource) -> Result<SessionMeta>
where
    A: DiscoverableProvider,
{
    A::parse_candidate_entries_agent_session_meta(&source.entries)
}

fn parse_agent_session_bytes<A>(
    bytes: Vec<u8>,
    selection: ParseSelection,
) -> Result<crate::agent_session::Session>
where
    A: DiscoverableProvider,
{
    let mut reader = std::io::Cursor::new(bytes);
    A::parse_direct_agent_session_reader_selected(&mut reader, InputMetadata::new(), selection)?
        .ok_or(crate::Error::UnsupportedInput {
            agent: A::name(),
            details: "agent_session byte parsing requires a direct semantic reader",
        })
}

#[cfg(feature = "watch")]
fn parse_watch_reader<A>(
    path: &Path,
    reader: &mut dyn std::io::BufRead,
    selection: ParseSelection,
) -> Result<crate::watch::provider::ParsedWatchSession>
where
    A: crate::watch::provider::ProviderWatchEvents,
{
    A::parse_watch_reader(path, reader, selection)
}

#[cfg(feature = "watch")]
fn parse_watch_metadata_reader<A>(
    path: &Path,
    reader: &mut dyn std::io::BufRead,
) -> Result<crate::watch::provider::ParsedWatchSession>
where
    A: crate::watch::provider::ProviderWatchEvents,
{
    A::parse_watch_metadata_reader(path, reader)
}

#[cfg(feature = "watch")]
fn discover_session_files_into<A>(
    root: &Path,
    is_recent: &mut dyn FnMut(&Path) -> bool,
    emit: &mut dyn FnMut(PathBuf),
) where
    A: DiscoverableProvider,
{
    let mut emit_source = |source: AgentProviderSource| emit(source.path().to_path_buf());
    let _ = A::discover_recent_in([root.to_path_buf()], is_recent, &mut emit_source);
}
