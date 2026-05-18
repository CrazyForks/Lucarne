#[cfg(feature = "discovery")]
use smol_str::SmolStr;
#[cfg(all(feature = "discovery", feature = "agent_session"))]
use std::io::BufRead;
#[cfg(feature = "discovery")]
use std::path::{Path, PathBuf};

#[cfg(feature = "discovery")]
use bytes::Bytes;

#[cfg(feature = "discovery")]
use crate::Result;
#[cfg(all(feature = "discovery", feature = "agent_session"))]
use crate::{InputMetadata, ParseSelection};

#[cfg(feature = "discovery")]
pub(crate) trait DiscoverableProvider: Sized + 'static {
    fn name() -> &'static str;

    fn default_roots() -> Vec<PathBuf>;

    #[cfg(any(
        feature = "watch",
        feature = "claude",
        feature = "cursor",
        feature = "pi"
    ))]
    fn candidate_role(_root: &Path, _path: &Path) -> CandidateRole {
        CandidateRole::Primary
    }

    #[cfg(any(
        feature = "watch",
        feature = "claude",
        feature = "cursor",
        feature = "pi"
    ))]
    fn includes_candidate_in_history(root: &Path, path: &Path) -> bool {
        Self::candidate_role(root, path).is_history()
    }

    fn discover_in<I, P>(
        roots: I,
        emit: &mut dyn FnMut(crate::providers::AgentProviderSource),
    ) -> Result<()>
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>;

    fn discover_recent_in<I, P>(
        roots: I,
        is_recent: &mut dyn FnMut(&Path) -> bool,
        emit: &mut dyn FnMut(crate::providers::AgentProviderSource),
    ) -> Result<()>
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        Self::discover_in(roots, &mut |source| {
            if is_recent(source.path()) {
                emit(source);
            }
        })
    }

    #[cfg(feature = "agent_session")]
    fn parse_candidate_entries_agent_session_meta(
        entries: &[AgentProviderSourceEntry],
    ) -> Result<crate::agent_session::SessionMeta>;

    #[cfg(feature = "agent_session")]
    fn parse_direct_agent_session_reader_selected<R>(
        _reader: &mut R,
        _metadata: InputMetadata<'_>,
        _selection: ParseSelection,
    ) -> Result<Option<crate::agent_session::Session>>
    where
        R: BufRead,
    {
        Ok(None)
    }

    #[cfg(feature = "agent_session")]
    fn visible_transcript_user_offsets(_bytes: &[u8], _base_offset: u64) -> Vec<u64> {
        Vec::new()
    }

    #[cfg(feature = "agent_session")]
    fn is_transcript_user_text_visible(text: &str) -> bool {
        !text.trim().is_empty()
    }
}

#[cfg(all(
    feature = "discovery",
    any(
        feature = "watch",
        feature = "claude",
        feature = "cursor",
        feature = "pi"
    )
))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CandidateRole {
    Primary,
    #[cfg(any(feature = "claude", feature = "cursor", feature = "pi"))]
    Subagent,
}

#[cfg(all(
    feature = "discovery",
    any(
        feature = "watch",
        feature = "claude",
        feature = "cursor",
        feature = "pi"
    )
))]
impl CandidateRole {
    #[must_use]
    pub fn is_history(self) -> bool {
        match self {
            Self::Primary => true,
            #[cfg(any(feature = "claude", feature = "cursor", feature = "pi"))]
            Self::Subagent => false,
        }
    }
}

#[cfg(feature = "discovery")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentProviderSourceEntry {
    pub(crate) path: PathBuf,
    pub(crate) name: Option<SmolStr>,
    pub(crate) kind: Option<SmolStr>,
    pub(crate) media_type: Option<SmolStr>,
    pub(crate) inline_data: Option<Bytes>,
}

#[cfg(feature = "discovery")]
impl AgentProviderSourceEntry {
    #[must_use]
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            name: None,
            kind: None,
            media_type: None,
            inline_data: None,
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn named(mut self, name: impl Into<SmolStr>) -> Self {
        self.name = Some(name.into());
        self
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn with_kind(mut self, kind: impl Into<SmolStr>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn with_media_type(mut self, media_type: impl Into<SmolStr>) -> Self {
        self.media_type = Some(media_type.into());
        self
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn with_inline_data(mut self, data: impl Into<Bytes>) -> Self {
        self.inline_data = Some(data.into());
        self
    }
}
