use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::{AgentProviderDescriptor, ParseSelection, agent_providers};

pub(super) const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(500);
pub(super) const DEFAULT_RECENT_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);
pub(super) const DEFAULT_SCAN_INTERVAL: Duration = Duration::from_secs(60);
pub(super) const DEFAULT_HOT_FILE_WINDOW: Duration = Duration::from_secs(60 * 60);

pub type WatchProvider = AgentProviderDescriptor;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchConfig {
    pub(super) providers: Vec<WatchProvider>,
    pub(super) provider_roots: HashMap<WatchProvider, Vec<PathBuf>>,
    pub(super) selection: ParseSelection,
    pub(super) debounce: Duration,
    pub(super) scan_interval: Duration,
    pub(super) recent_window: Duration,
    pub(super) hot_file_window: Duration,
    pub(super) subscriptions: WatchSubscriptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WatchSubscriptions {
    All,
    Paths(HashSet<PathBuf>),
}

impl WatchConfig {
    #[must_use]
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            provider_roots: HashMap::new(),
            selection: ParseSelection::full(),
            debounce: DEFAULT_DEBOUNCE,
            scan_interval: DEFAULT_SCAN_INTERVAL,
            recent_window: DEFAULT_RECENT_WINDOW,
            hot_file_window: DEFAULT_HOT_FILE_WINDOW,
            subscriptions: WatchSubscriptions::All,
        }
    }

    #[must_use]
    pub fn providers<I>(mut self, providers: I) -> Self
    where
        I: IntoIterator<Item = WatchProvider>,
    {
        self.providers = providers.into_iter().collect();
        self
    }

    #[must_use]
    pub fn all_enabled_providers() -> Vec<WatchProvider> {
        agent_providers()
    }

    #[must_use]
    pub fn with_all_enabled_providers(mut self) -> Self {
        self.providers = Self::all_enabled_providers();
        self
    }

    #[must_use]
    pub fn provider_roots<I, P>(mut self, provider: WatchProvider, roots: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        self.provider_roots
            .insert(provider, roots.into_iter().map(Into::into).collect());
        self
    }

    #[must_use]
    pub fn selection(mut self, selection: ParseSelection) -> Self {
        self.selection = selection;
        self
    }

    #[must_use]
    pub fn debounce(mut self, debounce: Duration) -> Self {
        self.debounce = debounce;
        self
    }

    #[must_use]
    pub fn scan_interval(mut self, scan_interval: Duration) -> Self {
        self.scan_interval = scan_interval;
        self
    }

    #[must_use]
    pub fn recent_window(mut self, recent_window: Duration) -> Self {
        self.recent_window = recent_window;
        self
    }

    #[must_use]
    pub fn hot_file_window(mut self, hot_file_window: Duration) -> Self {
        self.hot_file_window = hot_file_window;
        self
    }

    #[must_use]
    pub fn subscribe_all(mut self) -> Self {
        self.subscriptions = WatchSubscriptions::All;
        self
    }

    #[must_use]
    pub fn subscribed_paths<I, P>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        self.subscriptions = WatchSubscriptions::Paths(
            paths
                .into_iter()
                .map(Into::into)
                .map(canonicalize_lossy)
                .collect(),
        );
        self
    }

    pub(super) fn has_subscriber_for_path(&self, path: &Path) -> bool {
        match &self.subscriptions {
            WatchSubscriptions::All => true,
            WatchSubscriptions::Paths(paths) => paths.contains(&canonicalize_lossy(path)),
        }
    }

    pub(super) fn roots_for_provider(&self, provider: WatchProvider) -> Vec<PathBuf> {
        if let Some(roots) = self.provider_roots.get(&provider) {
            return roots.clone();
        }
        provider.default_roots()
    }
}

fn canonicalize_lossy(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self::new()
    }
}
