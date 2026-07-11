mod config;
mod event;
#[cfg(target_os = "macos")]
mod macos_fsevents;
pub(crate) mod provider;
mod raw;
pub(crate) mod state;

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::fs;
use std::future::Future;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime};

use futures::Stream;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

pub use config::{WatchConfig, WatchProvider};

struct WatchProviderIds<'a>(&'a [WatchProvider]);

impl fmt::Debug for WatchProviderIds<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_list()
            .entries(self.0.iter().map(|provider| provider.id()))
            .finish()
    }
}

#[cfg(any(feature = "codex", feature = "claude"))]
pub(crate) use event::watch_smol;
#[cfg(any(
    feature = "codex",
    feature = "claude",
    feature = "copilot",
    feature = "gemini",
    feature = "grok",
    feature = "pi"
))]
pub(crate) use event::watch_smol_opt;
pub use event::{
    WatchAssistantMessage, WatchAttachment, WatchChange, WatchError, WatchEvent, WatchEventMeta,
    WatchMessage, WatchOther, WatchSnapshot, WatchState, WatchToolCall, WatchToolResult,
    WatchTurnCompleted, WatchTurnFailed, WatchUnknown, WatchUpdate, WatchUsage,
};
#[cfg(target_os = "macos")]
use macos_fsevents::MacRecursiveWatcher;

use provider::{
    ParsedWatchSession, dedupe_provider_watch_events, discover_provider_session_files_into,
    includes_provider_candidate_in_history, is_session_like_path, normalize_provider_watch_events,
    parse_provider_metadata_reader, provider_needs_watch_state_seed,
    provider_supports_incremental_watch_events, seed_provider_watch_state,
};
use raw::RawWatchEvent;
use state::{FileSnapshot, ProviderWatchState, drop_leading_partial_line, split_complete_lines};

const MAX_WATCH_METADATA_READ_BYTES: u64 = 2 * 1024;
const MAX_WATCH_READ_BYTES: u64 = MAX_WATCH_METADATA_READ_BYTES;

pub struct SessionWatcher {
    config: WatchConfig,
    raw_rx: mpsc::UnboundedReceiver<RawWatchEvent>,
    _watcher: Option<RecommendedWatcher>,
    #[cfg(target_os = "macos")]
    _recursive_watcher: Option<MacRecursiveWatcher>,
    watched_paths: HashSet<PathBuf>,
    baselines: HashMap<PathBuf, FileSnapshot>,
    providers_by_path: Vec<(PathBuf, WatchProvider)>,
    pending_paths: HashSet<PathBuf>,
    pending_updates: VecDeque<Box<[WatchUpdate]>>,
    quiet_until: Option<Instant>,
    quiet_sleep: Option<Pin<Box<tokio::time::Sleep>>>,
    retention_scan_at: Option<Instant>,
    retention_sleep: Option<Pin<Box<tokio::time::Sleep>>>,
    disconnected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WatchTarget {
    path: PathBuf,
    recursive_mode: RecursiveMode,
}

#[derive(Debug)]
struct JsonlTail {
    complete: Vec<u8>,
    pending_partial: Vec<u8>,
}

impl WatchTarget {
    fn non_recursive(path: PathBuf) -> Self {
        Self {
            path,
            recursive_mode: RecursiveMode::NonRecursive,
        }
    }
}

impl SessionWatcher {
    pub fn start(config: WatchConfig) -> std::result::Result<Self, WatchError> {
        if config.providers.is_empty() {
            return Err(WatchError::NoProviders);
        }

        crate::memory_profile_snapshot!("agent_sessions.watch.start");
        debug!(
            target: "agent_sessions::watch",
            provider_ids = ?WatchProviderIds(&config.providers),
            debounce_ms = config.debounce.as_millis(),
            recent_window_secs = config.recent_window.as_secs(),
            selection = ?config.selection,
            "starting session watcher"
        );

        let (raw_tx, raw_rx) = mpsc::unbounded_channel();
        let notify_raw_tx = raw_tx.clone();
        let watcher = notify::recommended_watcher(move |event| {
            let _ = notify_raw_tx.send(RawWatchEvent::from_notify_result(event));
        })?;
        crate::memory_profile_snapshot!("agent_sessions.watch.after_recommended_watcher");
        #[cfg(target_os = "macos")]
        let recursive_watcher = MacRecursiveWatcher::new(raw_tx.clone());
        #[cfg(not(target_os = "macos"))]
        let _ = raw_tx;

        let mut roots = Vec::new();
        let mut providers_by_path = Vec::new();
        for provider in &config.providers {
            let provider_roots = config.roots_for_provider(*provider);
            for root in provider_roots {
                if !root.exists() {
                    trace!(
                        target: "agent_sessions::watch",
                        provider = provider.as_str(),
                        root = %root.display(),
                        "skipping missing watch root"
                    );
                    continue;
                }
                let root = fs::canonicalize(&root).unwrap_or(root);
                debug!(
                    target: "agent_sessions::watch",
                    provider = provider.as_str(),
                    root = %root.display(),
                    "registered session root"
                );
                providers_by_path.push((root.clone(), *provider));
                roots.push(root);
            }
        }

        if roots.is_empty() {
            return Err(WatchError::NoRoots);
        }
        crate::memory_profile_snapshot!("agent_sessions.watch.after_roots");

        let retention_scan_at = Some(Instant::now() + retention_scan_interval(&config));
        let mut this = Self {
            config,
            raw_rx,
            _watcher: Some(watcher),
            #[cfg(target_os = "macos")]
            _recursive_watcher: Some(recursive_watcher),
            watched_paths: HashSet::new(),
            baselines: HashMap::new(),
            providers_by_path,
            pending_paths: HashSet::new(),
            pending_updates: VecDeque::new(),
            quiet_until: None,
            quiet_sleep: None,
            retention_scan_at,
            retention_sleep: None,
            disconnected: false,
        };
        this.initialize_baselines();
        crate::memory_profile_snapshot!("agent_sessions.watch.after_initialize_baselines");
        let initial_targets = this.initial_watch_targets(&roots);
        trace!(
            target: "agent_sessions::watch",
            targets = initial_targets.len(),
            "selected initial watch targets"
        );
        for target in initial_targets {
            trace!(
                target: "agent_sessions::watch",
                watch_path = %target.path.display(),
                recursive = matches!(target.recursive_mode, RecursiveMode::Recursive),
                has_baseline = this.baselines.contains_key(&target.path),
                recent_session = this.is_recent_session_path(&target.path),
                hot_session = this.is_hot_session_path(&target.path),
                "initial watch target selected"
            );
            this.watch_target(target)?;
        }
        crate::memory_profile_snapshot!("agent_sessions.watch.after_initialize_baselines");
        debug!(
            target: "agent_sessions::watch",
            baselines = this.baselines.len(),
            roots = roots.len(),
            "session watcher started"
        );
        Ok(this)
    }

    fn accept_raw_event(&mut self, event: RawWatchEvent) {
        match event {
            RawWatchEvent::Paths(paths) => {
                trace!(
                    target: "agent_sessions::watch",
                    paths = paths.len(),
                    "received raw watch paths"
                );
                for path in paths {
                    self.accept_raw_path(path);
                }
                if !self.pending_paths.is_empty() {
                    self.quiet_until = Some(Instant::now() + self.config.debounce);
                    self.quiet_sleep = None;
                    trace!(
                        target: "agent_sessions::watch",
                        pending_paths = self.pending_paths.len(),
                        debounce_ms = self.config.debounce.as_millis(),
                        "debounce quiet deadline reset"
                    );
                }
            }
            RawWatchEvent::Error(error) => {
                warn!(
                    target: "agent_sessions::watch",
                    error = %error,
                    "raw file watcher error"
                );
            }
        }
    }

    fn accept_raw_path(&mut self, path: PathBuf) {
        let path = fs::canonicalize(&path).unwrap_or(path);
        let Some((root, provider)) = self.provider_root_for_path(&path) else {
            trace!(
                target: "agent_sessions::watch",
                path = %path.display(),
                "ignoring watch path outside configured roots"
            );
            return;
        };
        if !includes_provider_candidate_in_history(provider, root, &path) {
            trace!(
                target: "agent_sessions::watch",
                provider = provider.as_str(),
                path = %path.display(),
                "ignoring provider-classified non-history watch path"
            );
            return;
        }
        if is_session_like_path(&path) {
            trace!(
                target: "agent_sessions::watch",
                provider = provider.as_str(),
                path = %path.display(),
                "queued changed session path"
            );
            self.pending_paths.insert(path);
            return;
        }
        if path.is_dir() {
            if !self.has_recursive_root_for_path(&path) {
                self.watch_existing_directories_under(&path);
            }
            let discovered = self.discover_session_files_under(&path);
            trace!(
                target: "agent_sessions::watch",
                provider = provider.as_str(),
                path = %path.display(),
                discovered = discovered.len(),
                "expanded changed directory to session files"
            );
            for session_path in discovered {
                self.pending_paths.insert(session_path);
            }
        } else {
            trace!(
                target: "agent_sessions::watch",
                provider = provider.as_str(),
                path = %path.display(),
                "ignoring provider history watch path that is neither session-like nor directory"
            );
        }
    }

    fn process_pending_paths(&mut self) -> Vec<WatchUpdate> {
        self.quiet_until = None;
        self.quiet_sleep = None;
        let paths = self.pending_paths.drain().collect::<Vec<_>>();
        debug!(
            target: "agent_sessions::watch",
            pending_paths = paths.len(),
            "processing debounced session paths"
        );
        let mut updates = Vec::new();
        for path in paths {
            let Some(provider) = self.provider_for_path(&path) else {
                continue;
            };
            if path.exists() {
                updates.extend(self.process_existing_path(provider, path));
            } else if let Some(old) = self.baselines.remove(&path) {
                debug!(
                    target: "agent_sessions::watch",
                    provider = provider.as_str(),
                    path = %path.display(),
                    "session file deleted"
                );
                updates.push(WatchUpdate {
                    provider,
                    path,
                    session_id: old.session_id,
                    cwd: old.cwd,
                    title: old.title,
                    change: WatchChange::Deleted,
                    events: Vec::new().into_boxed_slice(),
                    error: None,
                });
            }
        }
        debug!(
            target: "agent_sessions::watch",
            updates = updates.len(),
            "processed debounced session paths"
        );
        updates
    }

    fn downgrade_stale_session_targets(&mut self) {
        let paths = self.baselines.keys().cloned().collect::<Vec<_>>();
        let mut downgraded = 0usize;
        for path in paths {
            if self.downgrade_session_target_if_stale(path) {
                downgraded += 1;
            }
        }
        if downgraded > 0 {
            debug!(
                target: "agent_sessions::watch",
                downgraded,
                baselines = self.baselines.len(),
                watched_paths = self.watched_paths.len(),
                "downgraded stale session watch targets"
            );
        }
    }

    fn downgrade_session_target_if_stale(&mut self, path: PathBuf) -> bool {
        if self.pending_paths.contains(&path)
            || self.is_explicit_session_file_root(&path)
            || self.should_watch_session_file_target(&path)
        {
            return false;
        }
        if self
            .baselines
            .get(&path)
            .is_some_and(|snapshot| !snapshot.pending_partial.is_empty())
        {
            trace!(
                target: "agent_sessions::watch",
                path = %path.display(),
                "retaining stale session baseline with pending partial record"
            );
            return false;
        }
        let removed_file = if self.session_has_parent_or_root_coverage(&path) {
            self.unwatch_session_file_target(&path)
        } else {
            false
        };
        let removed_parent = self.unwatch_stale_parent_directory_target(&path);
        if removed_file || removed_parent {
            trace!(
                target: "agent_sessions::watch",
                path = %path.display(),
                removed_file,
                removed_parent,
                "downgraded stale session target to parent/root watch"
            );
        }
        removed_file || removed_parent
    }

    fn is_explicit_session_file_root(&self, path: &Path) -> bool {
        self.providers_by_path
            .iter()
            .any(|(root, _provider)| root == path && root.is_file())
    }

    fn unwatch_session_file_target(&mut self, path: &Path) -> bool {
        let watch_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if !self.watched_paths.remove(&watch_path) {
            return false;
        }
        let Some(watcher) = self._watcher.as_mut() else {
            return true;
        };
        if let Err(error) = watcher.unwatch(&watch_path) {
            warn!(
                target: "agent_sessions::watch",
                watch_path = %watch_path.display(),
                error = %error,
                "failed to unwatch stale session file"
            );
        }
        true
    }

    fn session_has_parent_or_root_coverage(&self, path: &Path) -> bool {
        self.has_recursive_root_for_path(path)
            || path.parent().is_some_and(|parent| {
                let parent = fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
                self.watched_paths.contains(&parent)
            })
    }

    fn unwatch_stale_parent_directory_target(&mut self, path: &Path) -> bool {
        if self.has_recursive_root_for_path(path) {
            return false;
        }
        let Some(parent) = path.parent() else {
            return false;
        };
        let parent = fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
        if !self.watched_paths.contains(&parent)
            || self.should_watch_directory_for_current_state(&parent)
            || self.parent_directory_has_retained_session_target(&parent)
        {
            return false;
        }
        if !self.watched_paths.remove(&parent) {
            return false;
        }
        let Some(watcher) = self._watcher.as_mut() else {
            return true;
        };
        if let Err(error) = watcher.unwatch(&parent) {
            warn!(
                target: "agent_sessions::watch",
                watch_path = %parent.display(),
                error = %error,
                "failed to unwatch stale session directory"
            );
        }
        true
    }

    fn parent_directory_has_retained_session_target(&self, parent: &Path) -> bool {
        self.baselines.keys().any(|candidate| {
            session_parent_matches(candidate, parent)
                && self.session_path_requires_direct_target(candidate)
        }) || self
            .pending_paths
            .iter()
            .any(|candidate| session_parent_matches(candidate, parent))
    }

    fn session_path_requires_direct_target(&self, path: &Path) -> bool {
        self.pending_paths.contains(path)
            || self.is_explicit_session_file_root(path)
            || self.should_watch_session_file_target(path)
            || self
                .baselines
                .get(path)
                .is_some_and(|snapshot| !snapshot.pending_partial.is_empty())
    }

    fn should_watch_directory_for_current_state(&self, path: &Path) -> bool {
        self.provider_root_for_path(path)
            .is_some_and(|(root, provider)| self.should_watch_directory(provider, root, path))
    }

    fn schedule_next_retention_scan(&mut self) {
        self.retention_scan_at = Some(Instant::now() + retention_scan_interval(&self.config));
        self.retention_sleep = None;
    }

    fn watch_target(&mut self, target: WatchTarget) -> std::result::Result<(), WatchError> {
        let path = fs::canonicalize(&target.path).unwrap_or(target.path);
        if !self.watched_paths.insert(path.clone()) {
            return Ok(());
        }
        if matches!(target.recursive_mode, RecursiveMode::Recursive) {
            self.watch_recursive_path(&path)?;
        } else {
            self.watch_non_recursive_path(&path)?;
        }
        debug!(
            target: "agent_sessions::watch",
            watch_path = %path.display(),
            recursive = matches!(target.recursive_mode, RecursiveMode::Recursive),
            "watching session path"
        );
        Ok(())
    }

    fn watch_non_recursive_path(&mut self, path: &Path) -> std::result::Result<(), WatchError> {
        let Some(watcher) = self._watcher.as_mut() else {
            return Err(WatchError::Notify(notify::Error::generic(
                "non-recursive watcher is not initialized",
            )));
        };
        if let Err(error) = watcher.watch(path, RecursiveMode::NonRecursive) {
            self.watched_paths.remove(path);
            return Err(error.into());
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn watch_recursive_path(&mut self, path: &Path) -> std::result::Result<(), WatchError> {
        let Some(watcher) = self._recursive_watcher.as_mut() else {
            return Err(WatchError::Notify(notify::Error::generic(
                "recursive watcher is not initialized",
            )));
        };
        if let Err(error) = watcher.watch(path) {
            self.watched_paths.remove(path);
            return Err(error);
        }
        Ok(())
    }

    #[cfg(any(windows, target_os = "linux"))]
    fn watch_recursive_path(&mut self, path: &Path) -> std::result::Result<(), WatchError> {
        let Some(watcher) = self._watcher.as_mut() else {
            return Err(WatchError::Notify(notify::Error::generic(
                "recursive watcher is not initialized",
            )));
        };
        if let Err(error) = watcher.watch(path, RecursiveMode::Recursive) {
            self.watched_paths.remove(path);
            return Err(error.into());
        }
        Ok(())
    }

    #[cfg(all(not(target_os = "macos"), not(windows), not(target_os = "linux")))]
    fn watch_recursive_path(&mut self, path: &Path) -> std::result::Result<(), WatchError> {
        self.watch_non_recursive_path(path)
    }

    fn watch_existing_directories_under(&mut self, path: &Path) {
        let watch_paths = self.discover_changed_watch_directories_under(path);
        let Some(watcher) = self._watcher.as_mut() else {
            return;
        };
        for watch_path in watch_paths {
            let watch_path = fs::canonicalize(&watch_path).unwrap_or(watch_path);
            if !self.watched_paths.insert(watch_path.clone()) {
                continue;
            }
            if let Err(error) = watcher.watch(&watch_path, RecursiveMode::NonRecursive) {
                self.watched_paths.remove(&watch_path);
                warn!(
                    target: "agent_sessions::watch",
                    watch_path = %watch_path.display(),
                    error = %error,
                    "failed to watch new session directory"
                );
                continue;
            }
            debug!(
                target: "agent_sessions::watch",
                watch_path = %watch_path.display(),
                "watching new session directory"
            );
        }
    }

    fn watch_existing_path(&mut self, path: &Path) {
        let watch_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if self.existing_directory_watch_covers_child_file(&watch_path) {
            trace!(
                target: "agent_sessions::watch",
                watch_path = %watch_path.display(),
                "skipping session file watch covered by parent directory"
            );
            return;
        }
        let Some(watcher) = self._watcher.as_mut() else {
            return;
        };
        if !self.watched_paths.insert(watch_path.clone()) {
            return;
        }
        if let Err(error) = watcher.watch(&watch_path, RecursiveMode::NonRecursive) {
            self.watched_paths.remove(&watch_path);
            warn!(
                target: "agent_sessions::watch",
                watch_path = %watch_path.display(),
                error = %error,
                "failed to watch new session file"
            );
            return;
        }
        debug!(
            target: "agent_sessions::watch",
            watch_path = %watch_path.display(),
            "watching new session file"
        );
    }

    fn existing_directory_watch_covers_child_file(&self, path: &Path) -> bool {
        directory_watch_covers_child_file_events()
            && path.parent().is_some_and(|parent| {
                let parent = fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
                self.watched_paths.contains(&parent)
            })
    }

    fn process_existing_path(
        &mut self,
        provider: WatchProvider,
        path: PathBuf,
    ) -> Option<WatchUpdate> {
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                warn!(
                    target: "agent_sessions::watch",
                    provider = provider.as_str(),
                    path = %path.display(),
                    error = %error,
                    "failed to stat changed session path"
                );
                return Some(WatchUpdate {
                    provider,
                    path,
                    session_id: None,
                    cwd: None,
                    title: None,
                    change: WatchChange::ParseError,
                    events: Vec::new().into_boxed_slice(),
                    error: Some(error.to_string().into()),
                });
            }
        };
        let len = metadata.len();
        let old = self.baselines.get(&path).cloned();

        let Some(old) = old else {
            return self.process_created_path(provider, path, len);
        };

        if !provider_supports_incremental_watch_events(provider) {
            self.baselines.insert(
                path.clone(),
                FileSnapshot {
                    len,
                    has_subscriber: old.has_subscriber,
                    session_id: old.session_id,
                    cwd: old.cwd,
                    title: old.title,
                    watch_state: old.watch_state,
                    pending_partial: Vec::new(),
                },
            );
            trace!(
                target: "agent_sessions::watch",
                provider = provider.as_str(),
                path = %path.display(),
                len,
                "non-incremental watch target changed; advancing baseline without full reparse"
            );
            return None;
        }

        if len < old.len {
            self.baselines.insert(
                path.clone(),
                FileSnapshot {
                    len,
                    has_subscriber: old.has_subscriber,
                    session_id: old.session_id.clone(),
                    cwd: old.cwd.clone(),
                    title: old.title.clone(),
                    watch_state: ProviderWatchState::default(),
                    pending_partial: Vec::new(),
                },
            );
            warn!(
                target: "agent_sessions::watch",
                provider = provider.as_str(),
                path = %path.display(),
                old_len = old.len,
                new_len = len,
                "jsonl session file truncated"
            );
            return Some(WatchUpdate {
                provider,
                path,
                session_id: old.session_id,
                cwd: old.cwd,
                title: old.title,
                change: WatchChange::Truncated,
                events: Vec::new().into_boxed_slice(),
                error: None,
            });
        }

        if len == old.len {
            self.baselines.insert(
                path,
                FileSnapshot {
                    len,
                    has_subscriber: old.has_subscriber,
                    session_id: old.session_id,
                    cwd: old.cwd,
                    title: old.title,
                    watch_state: old.watch_state,
                    pending_partial: old.pending_partial,
                },
            );
            return None;
        }

        self.watch_existing_path(&path);
        self.process_jsonl_delta(provider, path, old, len)
    }

    /// Register a newly discovered session file.
    ///
    /// Incremental providers get a streaming metadata read and then track
    /// appended deltas. Non-incremental targets are only baselined; watch
    /// never reparses whole session files.
    fn process_created_path(
        &mut self,
        provider: WatchProvider,
        path: PathBuf,
        len: u64,
    ) -> Option<WatchUpdate> {
        self.watch_existing_path(&path);
        let has_subscriber = self.config.has_subscriber_for_path(&path);
        if !has_subscriber {
            self.baselines.insert(
                path.clone(),
                FileSnapshot {
                    len,
                    has_subscriber,
                    session_id: None,
                    cwd: None,
                    title: None,
                    watch_state: ProviderWatchState::default(),
                    pending_partial: Vec::new(),
                },
            );
            trace!(
                target: "agent_sessions::watch",
                provider = provider.as_str(),
                path = %path.display(),
                "new session file baselined without parsing because session has no subscriber"
            );
            return None;
        }
        let incremental = provider_supports_incremental_watch_events(provider);
        if incremental {
            return match self.read_metadata(provider, &path) {
                Ok(metadata) => {
                    let metadata_title = metadata.title.clone();
                    let tail = match self.read_latest_jsonl_tail(&path, len) {
                        Ok(tail) => tail,
                        Err(error) => {
                            warn!(
                                target: "agent_sessions::watch",
                                provider = provider.as_str(),
                                path = %path.display(),
                                error = %error,
                                "failed to read new jsonl session tail"
                            );
                            return Some(WatchUpdate {
                                provider,
                                path,
                                session_id: metadata.session_id,
                                cwd: metadata.cwd,
                                title: metadata.title,
                                change: WatchChange::ParseError,
                                events: Vec::new().into_boxed_slice(),
                                error: Some(error.to_string().into()),
                            });
                        }
                    };
                    let mut watch_state = ProviderWatchState::default();
                    let (session_id, cwd, title, events) = if tail.complete.is_empty() {
                        (
                            metadata.session_id.clone(),
                            metadata.cwd.clone(),
                            metadata_title.clone(),
                            Vec::new().into_boxed_slice(),
                        )
                    } else {
                        match self.parse_delta(provider, &path, tail.complete) {
                            Ok(parsed) => {
                                let events = dedupe_provider_watch_events(
                                    provider,
                                    parsed.events,
                                    &mut watch_state,
                                );
                                let events = normalize_provider_watch_events(
                                    provider,
                                    events,
                                    &mut watch_state,
                                );
                                let events = created_watch_tail_visible_events(events);
                                (
                                    metadata.session_id.clone().or(parsed.session_id),
                                    metadata.cwd.clone().or(parsed.cwd),
                                    metadata_title.clone().or(parsed.title),
                                    events,
                                )
                            }
                            Err(error) => {
                                warn!(
                                    target: "agent_sessions::watch",
                                    provider = provider.as_str(),
                                    path = %path.display(),
                                    error = %error,
                                    "failed to parse new jsonl session tail"
                                );
                                return Some(WatchUpdate {
                                    provider,
                                    path,
                                    session_id: metadata.session_id,
                                    cwd: metadata.cwd,
                                    title: metadata.title,
                                    change: WatchChange::ParseError,
                                    events: Vec::new().into_boxed_slice(),
                                    error: Some(error.to_string().into()),
                                });
                            }
                        }
                    };
                    self.baselines.insert(
                        path.clone(),
                        FileSnapshot {
                            len,
                            has_subscriber,
                            session_id: session_id.clone(),
                            cwd: cwd.clone(),
                            title: title.clone(),
                            watch_state,
                            pending_partial: tail.pending_partial,
                        },
                    );
                    debug!(
                        target: "agent_sessions::watch",
                        provider = provider.as_str(),
                        path = %path.display(),
                        events = events.len(),
                        "new jsonl session file parsed",
                    );
                    Some(WatchUpdate {
                        provider,
                        path,
                        session_id,
                        cwd,
                        title,
                        change: WatchChange::Created,
                        events,
                        error: None,
                    })
                }
                Err(error) => {
                    warn!(
                        target: "agent_sessions::watch",
                        provider = provider.as_str(),
                        path = %path.display(),
                        error = %error,
                        "failed to parse new jsonl session metadata"
                    );
                    Some(WatchUpdate {
                        provider,
                        path,
                        session_id: None,
                        cwd: None,
                        title: None,
                        change: WatchChange::ParseError,
                        events: Vec::new().into_boxed_slice(),
                        error: Some(error.to_string().into()),
                    })
                }
            };
        }
        self.baselines.insert(
            path.clone(),
            FileSnapshot {
                len,
                has_subscriber,
                session_id: None,
                cwd: None,
                title: None,
                watch_state: ProviderWatchState::default(),
                pending_partial: Vec::new(),
            },
        );
        debug!(
            target: "agent_sessions::watch",
            provider = provider.as_str(),
            path = %path.display(),
            "new non-incremental watch target baselined without full parse"
        );
        Some(WatchUpdate {
            provider,
            path,
            session_id: None,
            cwd: None,
            title: None,
            change: WatchChange::Created,
            events: Vec::new().into_boxed_slice(),
            error: None,
        })
    }

    fn process_jsonl_delta(
        &mut self,
        provider: WatchProvider,
        path: PathBuf,
        old: FileSnapshot,
        len: u64,
    ) -> Option<WatchUpdate> {
        if !old.has_subscriber {
            self.baselines.insert(
                path.clone(),
                FileSnapshot {
                    len,
                    has_subscriber: false,
                    session_id: old.session_id,
                    cwd: old.cwd,
                    title: old.title,
                    watch_state: old.watch_state,
                    pending_partial: Vec::new(),
                },
            );
            trace!(
                target: "agent_sessions::watch",
                provider = provider.as_str(),
                path = %path.display(),
                len,
                "jsonl delta skipped because session has no subscriber"
            );
            return None;
        }

        match self.read_delta(&path, &old, len) {
            Ok((advance_to, bytes)) => {
                let mut pending_partial = Vec::new();
                let complete = split_complete_lines(bytes, &mut pending_partial);
                if complete.is_empty() {
                    self.baselines.insert(
                        path.clone(),
                        FileSnapshot {
                            len: advance_to,
                            has_subscriber: old.has_subscriber,
                            session_id: old.session_id.clone(),
                            cwd: old.cwd.clone(),
                            title: old.title.clone(),
                            watch_state: old.watch_state,
                            pending_partial,
                        },
                    );
                    trace!(
                        target: "agent_sessions::watch",
                        provider = provider.as_str(),
                        path = %path.display(),
                        len = advance_to,
                        "jsonl delta has no complete lines yet"
                    );
                    return None;
                }
                let complete_bytes = complete.len();
                match self.parse_delta(provider, &path, complete) {
                    Ok(parsed) => {
                        let mut watch_state = old.watch_state.clone();
                        let events =
                            dedupe_provider_watch_events(provider, parsed.events, &mut watch_state);
                        let events =
                            normalize_provider_watch_events(provider, events, &mut watch_state);
                        let metadata = if old.session_id.is_none() || old.cwd.is_none() {
                            self.read_metadata(provider, &path).ok()
                        } else {
                            None
                        };
                        let session_id = old
                            .session_id
                            .or(parsed.session_id)
                            .or_else(|| metadata.as_ref().and_then(|meta| meta.session_id.clone()));
                        let cwd = old
                            .cwd
                            .or(parsed.cwd)
                            .or_else(|| metadata.as_ref().and_then(|meta| meta.cwd.clone()));
                        let title = old
                            .title
                            .or(parsed.title)
                            .or_else(|| metadata.as_ref().and_then(|meta| meta.title.clone()));
                        self.baselines.insert(
                            path.clone(),
                            FileSnapshot {
                                len: advance_to,
                                has_subscriber: old.has_subscriber,
                                session_id: session_id.clone(),
                                cwd: cwd.clone(),
                                title: title.clone(),
                                watch_state,
                                pending_partial,
                            },
                        );
                        debug!(
                            target: "agent_sessions::watch",
                            provider = provider.as_str(),
                            path = %path.display(),
                            bytes = complete_bytes,
                            events = events.len(),
                            "parsed jsonl session delta"
                        );
                        if events.is_empty() {
                            None
                        } else {
                            Some(WatchUpdate {
                                provider,
                                path,
                                session_id,
                                cwd,
                                title,
                                change: WatchChange::Updated,
                                events,
                                error: None,
                            })
                        }
                    }
                    Err(error) => {
                        warn!(
                            target: "agent_sessions::watch",
                            provider = provider.as_str(),
                            path = %path.display(),
                            error = %error,
                            "failed to parse jsonl session delta"
                        );
                        Some(WatchUpdate {
                            provider,
                            path,
                            session_id: old.session_id,
                            cwd: old.cwd,
                            title: old.title,
                            change: WatchChange::ParseError,
                            events: Vec::new().into_boxed_slice(),
                            error: Some(error.to_string().into()),
                        })
                    }
                }
            }
            Err(error) => {
                warn!(
                    target: "agent_sessions::watch",
                    provider = provider.as_str(),
                    path = %path.display(),
                    error = %error,
                    "failed to read jsonl session delta"
                );
                Some(WatchUpdate {
                    provider,
                    path,
                    session_id: old.session_id,
                    cwd: old.cwd,
                    title: old.title,
                    change: WatchChange::ParseError,
                    events: Vec::new().into_boxed_slice(),
                    error: Some(error.to_string().into()),
                })
            }
        }
    }

    fn read_delta(
        &self,
        path: &Path,
        old: &FileSnapshot,
        len: u64,
    ) -> std::io::Result<(u64, Vec<u8>)> {
        // Always read the full [old.len, len) range (chunked). Do NOT stop early
        // when a trailing complete JSONL record appears: Grok (and peers) can emit
        // single agent_message lines larger than MAX_WATCH_READ_BYTES. Early-stop +
        // drop_leading_partial_line permanently discarded those oversized lines and
        // left only turn_completed without body text — channel saw nothing.
        let mut file = fs::File::open(path)?;
        let mut start = len;
        let lower = old.len;
        let mut bytes = Vec::new();
        while start > lower {
            let chunk_start = start.saturating_sub(MAX_WATCH_READ_BYTES).max(lower);
            file.seek(SeekFrom::Start(chunk_start))?;
            let mut chunk = Vec::new();
            (&mut file)
                .take(start.saturating_sub(chunk_start))
                .read_to_end(&mut chunk)?;
            trace!(
                target: "agent_sessions::watch",
                path = %path.display(),
                chunk_start,
                chunk_end = start,
                old_len = lower,
                new_len = len,
                chunk_bytes = chunk.len(),
                "read jsonl delta chunk"
            );
            chunk.extend_from_slice(&bytes);
            bytes = chunk;
            if chunk_start == lower {
                if !old.pending_partial.is_empty() {
                    let mut combined = old.pending_partial.clone();
                    combined.extend_from_slice(&bytes);
                    bytes = combined;
                }
                break;
            }
            start = chunk_start;
        }
        Ok((len, bytes))
    }

    fn read_latest_jsonl_tail(&self, path: &Path, len: u64) -> std::io::Result<JsonlTail> {
        let mut file = fs::File::open(path)?;
        let mut start = len;
        let mut bytes = Vec::new();
        while start > 0 {
            let chunk_start = start.saturating_sub(MAX_WATCH_READ_BYTES);
            file.seek(SeekFrom::Start(chunk_start))?;
            let mut chunk = Vec::new();
            (&mut file)
                .take(start.saturating_sub(chunk_start))
                .read_to_end(&mut chunk)?;
            chunk.extend_from_slice(&bytes);
            bytes = chunk;

            if bytes.ends_with(b"\n") {
                if chunk_start == 0 {
                    keep_last_complete_jsonl_record(&mut bytes);
                    return Ok(JsonlTail {
                        complete: bytes,
                        pending_partial: Vec::new(),
                    });
                }
                if has_complete_jsonl_record_after_leading_boundary(&bytes) {
                    drop_leading_partial_line(&mut bytes);
                    keep_last_complete_jsonl_record(&mut bytes);
                    return Ok(JsonlTail {
                        complete: bytes,
                        pending_partial: Vec::new(),
                    });
                }
            } else if let Some(last_newline) = bytes.iter().rposition(|byte| *byte == b'\n') {
                return Ok(JsonlTail {
                    complete: Vec::new(),
                    pending_partial: bytes.split_off(last_newline + 1),
                });
            }

            start = chunk_start;
        }

        Ok(if bytes.ends_with(b"\n") {
            keep_last_complete_jsonl_record(&mut bytes);
            JsonlTail {
                complete: bytes,
                pending_partial: Vec::new(),
            }
        } else {
            JsonlTail {
                complete: Vec::new(),
                pending_partial: bytes,
            }
        })
    }

    fn read_trailing_partial_jsonl(&self, path: &Path, len: u64) -> std::io::Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let mut file = fs::File::open(path)?;
        file.seek(SeekFrom::Start(len - 1))?;
        let mut last = [0_u8; 1];
        file.read_exact(&mut last)?;
        if last[0] == b'\n' {
            return Ok(Vec::new());
        }

        let mut start = len;
        let mut bytes = Vec::new();
        while start > 0 {
            let chunk_start = start.saturating_sub(MAX_WATCH_READ_BYTES);
            file.seek(SeekFrom::Start(chunk_start))?;
            let mut chunk = Vec::new();
            (&mut file)
                .take(start.saturating_sub(chunk_start))
                .read_to_end(&mut chunk)?;
            chunk.extend_from_slice(&bytes);
            bytes = chunk;

            if let Some(last_newline) = bytes.iter().rposition(|byte| *byte == b'\n') {
                return Ok(bytes.split_off(last_newline + 1));
            }
            start = chunk_start;
        }
        Ok(bytes)
    }

    fn parse_delta(
        &self,
        provider: WatchProvider,
        path: &Path,
        bytes: Vec<u8>,
    ) -> crate::Result<ParsedWatchSession> {
        let mut reader = std::io::Cursor::new(bytes);
        provider.parse_watch_reader(path, &mut reader, self.config.selection.with_meta())
    }

    fn read_metadata(
        &self,
        provider: WatchProvider,
        path: &Path,
    ) -> crate::Result<ParsedWatchSession> {
        let mut reader = self.metadata_reader(path)?;
        parse_provider_metadata_reader(provider, path, &mut reader)
    }

    fn baseline_watch_state(&self, provider: WatchProvider, path: &Path) -> ProviderWatchState {
        if !provider_needs_watch_state_seed(provider) {
            return ProviderWatchState::default();
        }
        self.read_bounded_lookback(path)
            .ok()
            .and_then(|bytes| {
                self.parse_delta(provider, path, bytes)
                    .ok()
                    .map(|parsed| seed_provider_watch_state(provider, &parsed.events))
            })
            .unwrap_or_default()
    }

    fn read_bounded_lookback(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        let len = fs::metadata(path)?.len();
        let start = len.saturating_sub(MAX_WATCH_READ_BYTES);
        let mut file = fs::File::open(path)?;
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        file.take(len - start).read_to_end(&mut bytes)?;
        if start > 0 {
            drop_leading_partial_line(&mut bytes);
        }
        Ok(bytes)
    }

    fn metadata_reader(&self, path: &Path) -> std::io::Result<BufReader<fs::File>> {
        let file = fs::File::open(path)?;
        Ok(BufReader::with_capacity(
            MAX_WATCH_METADATA_READ_BYTES as usize,
            file,
        ))
    }

    fn provider_for_path(&self, path: &Path) -> Option<WatchProvider> {
        self.provider_root_for_path(path)
            .map(|(_root, provider)| provider)
    }

    fn provider_root_for_path(&self, path: &Path) -> Option<(&Path, WatchProvider)> {
        let mut best = None;
        let mut best_len = 0usize;
        for (root, provider) in &self.providers_by_path {
            if path.starts_with(root) || root.starts_with(path) || path == root {
                let len = root.components().count();
                if len >= best_len {
                    best = Some((root.as_path(), *provider));
                    best_len = len;
                }
            }
        }
        best
    }

    fn initialize_baselines(&mut self) {
        let discovered = self.discover_session_files();
        trace!(
            target: "agent_sessions::watch",
            discovered = discovered.len(),
            "discovered initial session baselines"
        );
        for path in discovered {
            if let Ok(metadata) = fs::metadata(&path) {
                let initial = self
                    .provider_for_path(&path)
                    .and_then(|provider| self.initial_snapshot(provider, &path));
                trace!(
                    target: "agent_sessions::watch",
                    path = %path.display(),
                    len = metadata.len(),
                    has_meta = initial.as_ref().is_some_and(|snapshot| {
                            snapshot.session_id.is_some()
                                || snapshot.cwd.is_some()
                                || snapshot.title.is_some()
                        }),
                    "baselined session file"
                );
                self.baselines.insert(
                    path.clone(),
                    initial.unwrap_or(FileSnapshot {
                        len: metadata.len(),
                        has_subscriber: self.config.has_subscriber_for_path(&path),
                        session_id: None,
                        cwd: None,
                        title: None,
                        watch_state: ProviderWatchState::default(),
                        pending_partial: Vec::new(),
                    }),
                );
            }
        }
    }

    fn discover_session_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for (root, provider) in &self.providers_by_path {
            if root.is_file() {
                if is_session_like_path(root) {
                    files.push(root.clone());
                }
            } else {
                discover_provider_session_files_into(
                    *provider,
                    root,
                    &mut |path| self.is_recent_session_path(path),
                    &mut |path| files.push(path),
                );
            }
        }
        files.sort();
        files.dedup();
        files
    }

    #[cfg(all(
        test,
        any(feature = "codex", all(feature = "claude", not(target_os = "macos")))
    ))]
    fn initial_watch_paths(&self, roots: &[PathBuf]) -> Vec<PathBuf> {
        self.initial_watch_targets(roots)
            .into_iter()
            .map(|target| target.path)
            .collect()
    }

    fn initial_watch_targets(&self, roots: &[PathBuf]) -> Vec<WatchTarget> {
        let mut paths = Vec::new();
        for root in roots {
            let target = self.root_watch_target(root);
            let recursive = matches!(target.recursive_mode, RecursiveMode::Recursive);
            push_watch_target(&mut paths, target);
            if !recursive {
                paths.extend(
                    self.discover_initial_watch_directories_under(root)
                        .into_iter()
                        .map(WatchTarget::non_recursive),
                );
            }
        }
        for session_path in self.baselines.keys() {
            if let Some(parent) = session_path.parent()
                && !self.has_recursive_root_for_path(parent)
            {
                push_watch_target(&mut paths, WatchTarget::non_recursive(parent.to_path_buf()));
            }
            if self.needs_session_file_watch_target(session_path, &paths) {
                push_watch_target(&mut paths, WatchTarget::non_recursive(session_path.clone()));
            }
        }
        paths.sort_by(|left, right| left.path.cmp(&right.path));
        paths
    }

    fn root_watch_target(&self, root: &Path) -> WatchTarget {
        if root.is_file() {
            return WatchTarget::non_recursive(
                root.parent().map(Path::to_path_buf).unwrap_or(root.into()),
            );
        }
        let recursive_mode = self
            .provider_for_path(root)
            .map(directory_root_watch_mode)
            .unwrap_or(RecursiveMode::NonRecursive);
        WatchTarget {
            path: root.into(),
            recursive_mode,
        }
    }

    fn has_recursive_root_for_path(&self, path: &Path) -> bool {
        self.providers_by_path.iter().any(|(root, provider)| {
            root.is_dir()
                && path.starts_with(root)
                && matches!(
                    directory_root_watch_mode(*provider),
                    RecursiveMode::Recursive
                )
        })
    }

    fn discover_initial_watch_directories_under(&self, path: &Path) -> Vec<PathBuf> {
        self.discover_watch_directories_under(path, initial_watch_directory_depth)
    }

    fn discover_changed_watch_directories_under(&self, path: &Path) -> Vec<PathBuf> {
        self.discover_watch_directories_under(path, changed_watch_directory_depth)
    }

    fn discover_watch_directories_under(
        &self,
        path: &Path,
        depth_for_provider: fn(WatchProvider) -> Option<usize>,
    ) -> Vec<PathBuf> {
        let mut directories = Vec::new();
        for (root, provider) in &self.providers_by_path {
            if root.is_file() {
                if (path.starts_with(root) || root.starts_with(path) || path == root)
                    && let Some(parent) = root.parent()
                {
                    directories.push(parent.to_path_buf());
                }
                continue;
            }
            if !(path.starts_with(root) || root.starts_with(path) || path == root) {
                continue;
            }
            let base = if path.starts_with(root) { path } else { root };
            if !base.is_dir() {
                continue;
            }
            let Some(max_depth) = depth_for_provider(*provider) else {
                continue;
            };
            for entry in walkdir::WalkDir::new(base)
                .min_depth(0)
                .max_depth(max_depth)
                .into_iter()
                .filter_entry(|entry| {
                    includes_provider_candidate_in_history(*provider, root, entry.path())
                })
                .filter_map(std::result::Result::ok)
            {
                if entry.file_type().is_dir()
                    && self.should_watch_directory(*provider, root, entry.path())
                {
                    directories.push(entry.into_path());
                }
            }
        }
        directories.sort();
        directories.dedup();
        directories
    }

    fn should_watch_directory(&self, provider: WatchProvider, root: &Path, path: &Path) -> bool {
        provider.includes_watch_directory(root, path, self.is_recent_directory_path(path))
    }

    fn initial_snapshot(&self, provider: WatchProvider, path: &Path) -> Option<FileSnapshot> {
        let metadata = fs::metadata(path).ok()?;
        let has_subscriber = self.config.has_subscriber_for_path(path);
        let incremental = provider_supports_incremental_watch_events(provider);
        if incremental {
            if !has_subscriber {
                return Some(FileSnapshot {
                    len: metadata.len(),
                    has_subscriber,
                    session_id: None,
                    cwd: None,
                    title: None,
                    watch_state: ProviderWatchState::default(),
                    pending_partial: Vec::new(),
                });
            }
            let parsed = self.read_metadata(provider, path).ok()?;
            return Some(FileSnapshot {
                len: metadata.len(),
                has_subscriber,
                session_id: parsed.session_id,
                cwd: parsed.cwd,
                title: parsed.title,
                watch_state: self.baseline_watch_state(provider, path),
                pending_partial: self
                    .read_trailing_partial_jsonl(path, metadata.len())
                    .unwrap_or_default(),
            });
        }
        Some(FileSnapshot {
            len: metadata.len(),
            has_subscriber,
            session_id: None,
            cwd: None,
            title: None,
            watch_state: ProviderWatchState::default(),
            pending_partial: Vec::new(),
        })
    }

    fn discover_session_files_under(&self, path: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for (root, provider) in &self.providers_by_path {
            if path.starts_with(root) || root.starts_with(path) || path == root {
                if root.is_file() {
                    if is_session_like_path(root) {
                        files.push(root.clone());
                    }
                } else {
                    discover_provider_session_files_into(
                        *provider,
                        root,
                        &mut |file| self.is_recent_session_path(file),
                        &mut |file| {
                            if file.starts_with(path) || path.starts_with(&file) {
                                files.push(file);
                            }
                        },
                    );
                }
            }
        }
        files.sort();
        files.dedup();
        files
    }

    fn is_recent_session_path(&self, path: &Path) -> bool {
        let Ok(metadata) = fs::metadata(path) else {
            return false;
        };
        let Ok(modified) = metadata.modified() else {
            return true;
        };
        session_modified_within(modified, SystemTime::now(), self.config.recent_window)
    }

    fn is_hot_session_path(&self, path: &Path) -> bool {
        let Ok(metadata) = fs::metadata(path) else {
            return false;
        };
        let Ok(modified) = metadata.modified() else {
            return true;
        };
        session_modified_within(modified, SystemTime::now(), self.config.hot_file_window)
    }

    fn should_watch_session_file_target(&self, path: &Path) -> bool {
        self.is_recent_session_path(path) || self.is_hot_session_path(path)
    }

    fn needs_session_file_watch_target(&self, path: &Path, targets: &[WatchTarget]) -> bool {
        if self.has_recursive_root_for_path(path) {
            return self.should_watch_session_file_target(path);
        }
        if directory_watch_covers_child_file_events()
            && path
                .parent()
                .is_some_and(|parent| has_non_recursive_watch_target(targets, parent))
        {
            return false;
        }
        true
    }

    fn is_recent_directory_path(&self, path: &Path) -> bool {
        let Ok(metadata) = fs::metadata(path) else {
            return false;
        };
        let Ok(modified) = metadata.modified() else {
            return true;
        };
        session_modified_within(modified, SystemTime::now(), self.config.recent_window)
    }
}

impl Stream for SessionWatcher {
    type Item = std::result::Result<Box<[WatchUpdate]>, WatchError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        loop {
            if this.disconnected {
                return Poll::Ready(None);
            }
            if let Some(updates) = this.pending_updates.pop_front() {
                return Poll::Ready(Some(Ok(updates)));
            }

            loop {
                match this.raw_rx.poll_recv(cx) {
                    Poll::Ready(Some(event)) => this.accept_raw_event(event),
                    Poll::Ready(None) => {
                        this.disconnected = true;
                        break;
                    }
                    Poll::Pending => break,
                }
            }

            if let Some(quiet_until) = this.quiet_until {
                let now = Instant::now();
                if now >= quiet_until {
                    return Poll::Ready(Some(Ok(this.process_pending_paths().into_boxed_slice())));
                }

                let delay = quiet_until.saturating_duration_since(now);
                if poll_sleep(&mut this.quiet_sleep, delay, cx).is_ready() {
                    return Poll::Ready(Some(Ok(this.process_pending_paths().into_boxed_slice())));
                }
            } else {
                this.quiet_sleep = None;
            }

            if let Some(retention_scan_at) = this.retention_scan_at {
                let now = Instant::now();
                if now >= retention_scan_at {
                    this.downgrade_stale_session_targets();
                    this.schedule_next_retention_scan();
                    continue;
                }

                let delay = retention_scan_at.saturating_duration_since(now);
                if poll_sleep(&mut this.retention_sleep, delay, cx).is_ready() {
                    this.downgrade_stale_session_targets();
                    this.schedule_next_retention_scan();
                    continue;
                }
            } else {
                this.retention_sleep = None;
            }

            if this.disconnected {
                return Poll::Ready(Some(Err(WatchError::Disconnected)));
            }

            return Poll::Pending;
        }
    }
}

fn poll_sleep(
    sleep: &mut Option<Pin<Box<tokio::time::Sleep>>>,
    delay: Duration,
    cx: &mut Context<'_>,
) -> Poll<()> {
    if sleep.is_none() {
        *sleep = Some(Box::pin(tokio::time::sleep(delay)));
    }
    let Some(active) = sleep.as_mut() else {
        return Poll::Pending;
    };
    match active.as_mut().poll(cx) {
        Poll::Ready(()) => {
            *sleep = None;
            Poll::Ready(())
        }
        Poll::Pending => Poll::Pending,
    }
}

fn retention_scan_interval(config: &WatchConfig) -> Duration {
    if config.scan_interval.is_zero() {
        Duration::from_millis(1)
    } else {
        config.scan_interval
    }
}

fn session_parent_matches(path: &Path, parent: &Path) -> bool {
    path.parent().is_some_and(|candidate_parent| {
        let candidate_parent =
            fs::canonicalize(candidate_parent).unwrap_or_else(|_| candidate_parent.to_path_buf());
        candidate_parent == parent
    })
}

fn has_complete_jsonl_record_after_leading_boundary(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .is_some_and(|newline| newline + 1 < bytes.len())
}

fn created_watch_tail_visible_events(events: Box<[WatchEvent]>) -> Box<[WatchEvent]> {
    events
        .into_vec()
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                WatchEvent::AssistantMessage(_)
                    | WatchEvent::Attachment(_)
                    | WatchEvent::ToolCall(_)
                    | WatchEvent::ToolResult(_)
                    | WatchEvent::Usage(_)
                    | WatchEvent::TurnCompleted(_)
                    | WatchEvent::TurnFailed(_)
            )
        })
        .collect::<Vec<_>>()
        .into_boxed_slice()
}

fn keep_last_complete_jsonl_record(bytes: &mut Vec<u8>) {
    if !bytes.ends_with(b"\n") {
        return;
    }
    let before_final_newline = bytes.len().saturating_sub(1);
    let second_last_start = bytes[..before_final_newline]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .and_then(|last_newline| {
            bytes[..last_newline]
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map(|second_last_newline| second_last_newline + 1)
        })
        .unwrap_or(0);
    if second_last_start > 0 {
        bytes.drain(..second_last_start);
    }
}

fn session_modified_within(modified: SystemTime, now: SystemTime, recent_window: Duration) -> bool {
    match now.duration_since(modified) {
        Ok(age) => age <= recent_window,
        Err(_) => true,
    }
}

#[cfg(any(target_os = "macos", windows))]
fn directory_root_watch_mode(_provider: WatchProvider) -> RecursiveMode {
    RecursiveMode::Recursive
}

#[cfg(not(any(target_os = "macos", windows)))]
fn directory_root_watch_mode(_provider: WatchProvider) -> RecursiveMode {
    RecursiveMode::NonRecursive
}

fn push_watch_target(targets: &mut Vec<WatchTarget>, target: WatchTarget) {
    if let Some(existing) = targets
        .iter_mut()
        .find(|existing| existing.path == target.path)
    {
        if matches!(target.recursive_mode, RecursiveMode::Recursive) {
            existing.recursive_mode = RecursiveMode::Recursive;
        }
        return;
    }
    targets.push(target);
}

fn has_non_recursive_watch_target(targets: &[WatchTarget], path: &Path) -> bool {
    targets.iter().any(|target| {
        target.path == path && matches!(target.recursive_mode, RecursiveMode::NonRecursive)
    })
}

#[cfg(target_os = "linux")]
fn directory_watch_covers_child_file_events() -> bool {
    true
}

#[cfg(not(target_os = "linux"))]
fn directory_watch_covers_child_file_events() -> bool {
    false
}

fn initial_watch_directory_depth(provider: WatchProvider) -> Option<usize> {
    provider.initial_watch_directory_depth()
}

fn changed_watch_directory_depth(provider: WatchProvider) -> Option<usize> {
    provider.changed_watch_directory_depth()
}

#[cfg(all(test, feature = "agent_session"))]
mod tests;
