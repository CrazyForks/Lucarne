use std::{path::Path, path::PathBuf};

use agent_sessions::{ParseSelection, SessionFileFormat};

use super::{
    transcript::{parse_bounded_jsonl_transcript_window, ProviderTranscript},
    HistoryTranscriptError,
};

pub(super) type HistoryProviderSource = agent_sessions::AgentProviderSource;

#[derive(Debug, Clone, Copy)]
pub struct HistoryProviderDescriptor {
    provider: agent_sessions::AgentProviderDescriptor,
}

impl HistoryProviderDescriptor {
    #[must_use]
    pub fn id(self) -> &'static str {
        self.provider.id()
    }

    #[must_use]
    pub fn display_name(self) -> &'static str {
        self.provider.display_name()
    }

    pub(super) fn default_roots(self) -> Vec<PathBuf> {
        self.provider.default_roots()
    }

    pub(super) fn discover_sources_into(
        self,
        emit: &mut dyn FnMut(HistoryProviderSource),
    ) -> agent_sessions::Result<()> {
        self.provider.discover_sources_into(emit)
    }

    pub(super) fn parse_source_meta(
        self,
        source: &HistoryProviderSource,
    ) -> agent_sessions::Result<agent_sessions::agent_session::SessionMeta> {
        self.provider.parse_source_meta(source)
    }

    #[cfg(test)]
    pub(super) fn parse_file_meta(
        self,
        path: PathBuf,
    ) -> agent_sessions::Result<agent_sessions::agent_session::SessionMeta> {
        self.provider.parse_file_meta(path)
    }

    pub(super) fn parse_transcript(
        self,
        path: &Path,
        cursor: Option<&str>,
        selection: ParseSelection,
        visible_limit: usize,
    ) -> Result<ProviderTranscript, HistoryTranscriptError> {
        match self.provider.session_file_format() {
            SessionFileFormat::LineDelimitedJson => parse_bounded_jsonl_transcript_window(
                self.provider,
                path,
                cursor,
                selection,
                visible_limit,
            ),
            SessionFileFormat::JsonDocument => Err(HistoryTranscriptError::UnsupportedProvider {
                provider_id: self.id().to_string(),
            }),
        }
    }

    #[must_use]
    pub(crate) fn watch_provider(self) -> agent_sessions::WatchProvider {
        self.provider
    }

    #[must_use]
    pub(super) fn is_transcript_user_text_visible(self, text: &str) -> bool {
        self.provider.is_transcript_user_text_visible(text)
    }
}

#[must_use]
pub fn history_providers() -> Vec<HistoryProviderDescriptor> {
    agent_sessions::agent_providers()
        .into_iter()
        .map(|provider| HistoryProviderDescriptor { provider })
        .collect()
}

#[must_use]
pub fn history_providers_for_ids(enabled_ids: &[String]) -> Vec<HistoryProviderDescriptor> {
    use std::collections::HashSet;

    let requested = enabled_ids
        .iter()
        .map(|id| id.as_str())
        .collect::<HashSet<_>>();
    history_providers()
        .into_iter()
        .filter(|provider| requested.contains(provider.id()))
        .collect()
}

#[must_use]
pub fn history_provider(provider_id: &str) -> Option<HistoryProviderDescriptor> {
    agent_sessions::agent_provider(provider_id)
        .map(|provider| HistoryProviderDescriptor { provider })
}
