use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use crate::history::{
    collect_candidates, entries_page_from_candidates, finish_workspace_accumulators,
    ranked_candidate_page_for_providers, record_workspace_meta, HistoryCandidate,
    HistoryCandidateKey, HistoryEntry, HistoryProviderDescriptor, HistorySessionMeta,
    HistoryWorkspace,
};
use tracing::{debug, info};

pub struct HistoryIndex {
    providers: Vec<HistoryProviderDescriptor>,
    provider_ids: Vec<&'static str>,
    ttl: Duration,
    cache: RwLock<HistoryCache>,
}

#[derive(Default)]
struct HistoryCache {
    metadata: HashMap<HistoryCandidateKey, Option<HistorySessionMeta>>,
    available_provider_ids: Option<Vec<&'static str>>,
    ranked_pages: HashMap<RankedCandidatePageKey, Arc<crate::history::HistoryCandidatePage>>,
    workspace_snapshots: HashMap<Vec<&'static str>, Arc<Vec<HistoryWorkspace>>>,
    refreshed_at: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RankedCandidatePageKey {
    provider_ids: Vec<&'static str>,
    retain_limit: usize,
}

pub struct IndexedHistoryPage {
    pub entries: Vec<HistoryEntry>,
    pub total: usize,
}

pub struct IndexedHistoryWorkspacePage {
    pub entries: Vec<HistoryWorkspace>,
    pub total: usize,
}

impl HistoryIndex {
    pub fn new(providers: Vec<HistoryProviderDescriptor>, ttl: Duration) -> Self {
        let provider_ids = providers.iter().map(|provider| provider.id()).collect();
        Self {
            providers,
            provider_ids,
            ttl,
            cache: RwLock::new(HistoryCache::default()),
        }
    }

    pub fn provider_ids(&self) -> &[&'static str] {
        &self.provider_ids
    }

    pub fn provider_catalog(&self) -> &[HistoryProviderDescriptor] {
        &self.providers
    }

    pub fn available_provider_ids(&self) -> Vec<&'static str> {
        self.refresh_if_stale();
        if let Some(provider_ids) = self
            .cache
            .read()
            .expect("history cache lock")
            .available_provider_ids
            .clone()
        {
            return provider_ids;
        }

        let page = self.ranked_candidate_page(&self.providers, 0);
        self.cache_available_provider_ids_from_page(
            &self.providers,
            &page.provider_ids_by_activity,
        );
        page.provider_ids_by_activity.clone()
    }

    pub fn refresh(&self) {
        let mut cache = self.cache.write().expect("history cache lock");
        self.refresh_locked(&mut cache);
    }

    fn refresh_locked(&self, cache: &mut HistoryCache) {
        *cache = HistoryCache {
            refreshed_at: Some(Instant::now()),
            ..HistoryCache::default()
        };
        info!(
            target: "lucarne::history::index",
            provider_count = self.provider_ids.len(),
            "history index cache invalidated"
        );
    }

    pub fn is_stale(&self) -> bool {
        let cache = self.cache.read().expect("history cache lock");
        cache.is_stale(self.ttl)
    }

    pub fn refresh_if_stale(&self) {
        let mut cache = self.cache.write().expect("history cache lock");
        if cache.is_stale(self.ttl) {
            self.refresh_locked(&mut cache);
        }
    }

    pub fn list_page(&self, offset: usize, limit: usize) -> IndexedHistoryPage {
        self.refresh_if_stale();
        let (entries, total) =
            self.paged_entries_from_ranked_candidates(&self.providers, None, offset, limit);
        let page = IndexedHistoryPage { entries, total };
        debug!(
            target: "lucarne::history::index",
            offset,
            limit,
            returned = page.entries.len(),
            total = page.total,
            "history index page served"
        );
        page
    }

    fn paged_entries_from_ranked_candidates(
        &self,
        providers: &[HistoryProviderDescriptor],
        cwd: Option<&Path>,
        offset: usize,
        limit: usize,
    ) -> (Vec<HistoryEntry>, usize) {
        if cwd.is_some() {
            return self.paged_entries_matching_cwd(providers, cwd, offset, limit);
        }
        if limit == 0 {
            let page = self.ranked_candidate_page(providers, 0);
            return (Vec::new(), page.total);
        }

        let needed = offset.saturating_add(limit).max(1);
        let mut retain = needed;
        loop {
            let page = self.ranked_candidate_page(providers, retain);
            self.cache_available_provider_ids_from_page(providers, &page.provider_ids_by_activity);
            let (entries, total) = entries_page_from_candidates(
                &page.candidates,
                None,
                offset,
                limit,
                |candidate| self.cached_candidate_meta(candidate),
                page.total,
            );
            if entries.len() == limit || page.candidates.len() >= page.total {
                return (entries, total);
            }
            let next_retain = retain.saturating_mul(2).min(page.total);
            if next_retain <= retain {
                return (entries, total);
            }
            retain = next_retain;
        }
    }

    fn ranked_candidate_page(
        &self,
        providers: &[HistoryProviderDescriptor],
        retain_limit: usize,
    ) -> Arc<crate::history::HistoryCandidatePage> {
        let key = RankedCandidatePageKey::new(providers, retain_limit);
        if let Some(page) = self
            .cache
            .read()
            .expect("history cache lock")
            .ranked_pages
            .get(&key)
            .cloned()
        {
            return page;
        }

        let page = Arc::new(ranked_candidate_page_for_providers(providers, retain_limit));
        let mut cache = self.cache.write().expect("history cache lock");
        cache
            .ranked_pages
            .entry(key)
            .or_insert_with(|| Arc::clone(&page))
            .clone()
    }

    fn cache_available_provider_ids_from_page(
        &self,
        providers: &[HistoryProviderDescriptor],
        provider_ids: &[&'static str],
    ) {
        if !self.providers_match_configured(providers) {
            return;
        }
        let mut cache = self.cache.write().expect("history cache lock");
        if cache.available_provider_ids.is_none() {
            cache.available_provider_ids = Some(provider_ids.to_vec());
        }
    }

    fn paged_entries_matching_cwd(
        &self,
        providers: &[HistoryProviderDescriptor],
        cwd: Option<&Path>,
        offset: usize,
        limit: usize,
    ) -> (Vec<HistoryEntry>, usize) {
        let retain_limit = offset.saturating_add(limit);
        let mut candidates = Vec::<HistoryCandidate>::new();
        let mut total = 0usize;
        for provider in providers {
            collect_candidates(*provider, |candidate| {
                let Some(meta) = self.cached_candidate_meta(&candidate) else {
                    return;
                };
                if !meta.matches_cwd(cwd) {
                    return;
                }
                total = total.saturating_add(1);
                if retain_limit == 0 {
                    return;
                }
                candidates.push(candidate);
                candidates.sort_by(|left, right| {
                    right
                        .last_modified_unix()
                        .cmp(&left.last_modified_unix())
                        .then_with(|| left.path().cmp(right.path()))
                });
                candidates.truncate(retain_limit);
            });
        }
        let (entries, _) = entries_page_from_candidates(
            &candidates,
            cwd,
            offset,
            limit,
            |candidate| self.cached_candidate_meta(candidate),
            total,
        );
        (entries, total)
    }

    fn cached_candidate_meta(&self, candidate: &HistoryCandidate) -> Option<HistorySessionMeta> {
        let key = candidate.key();
        if let Some(meta) = self
            .cache
            .read()
            .expect("history cache lock")
            .metadata
            .get(&key)
            .cloned()
        {
            return meta;
        }

        let parsed = match crate::history::parse_candidate_meta(candidate) {
            Ok(meta) => Some(meta),
            Err(err) => {
                debug!(
                    target: "lucarne::history::index",
                    provider = candidate.provider_id(),
                    error_kind = err.kind(),
                    error = %err.message(),
                    "paged metadata candidate skipped"
                );
                None
            }
        };
        let mut cache = self.cache.write().expect("history cache lock");
        cache.metadata.entry(key).or_insert_with(|| parsed.clone());
        parsed
    }

    pub fn entry_at(&self, index: usize) -> Option<HistoryEntry> {
        self.list_page(index, 1).entries.into_iter().next()
    }

    pub fn list_page_filtered(
        &self,
        provider_ids: &[&str],
        cwd: Option<&Path>,
        offset: usize,
        limit: usize,
    ) -> IndexedHistoryPage {
        self.refresh_if_stale();
        let provider_ids = self.normalize_provider_ids(provider_ids);
        let providers = self.providers_for_ids(&provider_ids);
        let (entries, total) =
            self.paged_entries_from_ranked_candidates(&providers, cwd, offset, limit);
        IndexedHistoryPage { entries, total }
    }

    pub fn entry_at_filtered(
        &self,
        provider_ids: &[&str],
        cwd: Option<&Path>,
        index: usize,
    ) -> Option<HistoryEntry> {
        self.list_page_filtered(provider_ids, cwd, index, 1)
            .entries
            .into_iter()
            .next()
    }

    pub fn entry_for_provider_session(
        &self,
        provider_id: &str,
        session_id: &str,
        cwd: Option<&Path>,
    ) -> Option<HistoryEntry> {
        self.refresh_if_stale();
        let provider_ids = self.normalize_provider_ids(&[provider_id]);
        let providers = self.providers_for_ids(&provider_ids);
        for provider in providers {
            let mut found = None;
            collect_candidates(provider, |candidate| {
                if found.is_some() {
                    return;
                }
                let Some(meta) = self.cached_candidate_meta(&candidate) else {
                    return;
                };
                if meta.session_id == session_id && meta.matches_cwd(cwd) {
                    found = Some(meta.to_entry(&candidate));
                }
            });
            if found.is_some() {
                return found;
            }
        }
        None
    }

    pub fn list_workspaces_page(
        &self,
        provider_ids: &[&str],
        offset: usize,
        limit: usize,
    ) -> IndexedHistoryWorkspacePage {
        let workspaces = self.workspace_snapshot_for_providers(provider_ids);
        let total = workspaces.len();
        let entries = if limit == 0 {
            Vec::new()
        } else {
            workspaces
                .iter()
                .skip(offset)
                .take(limit)
                .cloned()
                .collect()
        };
        IndexedHistoryWorkspacePage { entries, total }
    }

    pub fn list_workspaces(&self, provider_ids: &[&str]) -> Vec<HistoryWorkspace> {
        self.workspace_snapshot_for_providers(provider_ids)
            .as_ref()
            .clone()
    }

    fn workspace_snapshot_for_providers(
        &self,
        provider_ids: &[&str],
    ) -> Arc<Vec<HistoryWorkspace>> {
        self.refresh_if_stale();
        let provider_ids = self.normalize_provider_ids(provider_ids);
        if provider_ids.is_empty() {
            return Arc::new(Vec::new());
        }
        if let Some(snapshot) = self.cached_workspace_snapshot(&provider_ids) {
            return snapshot;
        }

        let providers = self.providers_for_ids(&provider_ids);
        let mut workspaces = BTreeMap::new();
        for provider in providers {
            collect_candidates(provider, |candidate| {
                if let Some(meta) = self.cached_candidate_meta(&candidate) {
                    record_workspace_meta(&mut workspaces, &candidate, &meta);
                }
            });
        }
        let snapshot = Arc::new(finish_workspace_accumulators(workspaces));
        let mut cache = self.cache.write().expect("history cache lock");
        cache
            .workspace_snapshots
            .insert(provider_ids, Arc::clone(&snapshot));
        snapshot
    }

    fn cached_workspace_snapshot(
        &self,
        provider_ids: &[&'static str],
    ) -> Option<Arc<Vec<HistoryWorkspace>>> {
        self.cache
            .read()
            .expect("history cache lock")
            .workspace_snapshots
            .get(provider_ids)
            .cloned()
    }

    fn providers_for_ids(&self, provider_ids: &[&'static str]) -> Vec<HistoryProviderDescriptor> {
        self.providers
            .iter()
            .copied()
            .filter(|provider| provider_ids.contains(&provider.id()))
            .collect()
    }

    fn providers_match_configured(&self, providers: &[HistoryProviderDescriptor]) -> bool {
        providers.len() == self.providers.len()
            && providers
                .iter()
                .map(|provider| provider.id())
                .eq(self.provider_ids.iter().copied())
    }

    fn normalize_provider_ids(&self, provider_ids: &[&str]) -> Vec<&'static str> {
        let mut out = Vec::new();
        for supported_provider_id in &self.provider_ids {
            if provider_ids
                .iter()
                .any(|provider_id| provider_id == supported_provider_id)
            {
                out.push(*supported_provider_id);
            }
        }
        out
    }
}

impl RankedCandidatePageKey {
    fn new(providers: &[HistoryProviderDescriptor], retain_limit: usize) -> Self {
        Self {
            provider_ids: providers.iter().map(|provider| provider.id()).collect(),
            retain_limit,
        }
    }
}

impl HistoryCache {
    fn is_stale(&self, ttl: Duration) -> bool {
        match self.refreshed_at {
            Some(refreshed_at) => refreshed_at.elapsed() >= ttl,
            None => true,
        }
    }
}
