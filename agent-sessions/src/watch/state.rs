#[cfg(feature = "codex")]
use std::collections::VecDeque;

use smol_str::SmolStr;

#[cfg(feature = "codex")]
const MAX_EVENT_FINGERPRINTS: usize = 256;

#[derive(Debug, Clone)]
pub(super) struct FileSnapshot {
    pub(super) len: u64,
    pub(super) has_subscriber: bool,
    pub(super) session_id: Option<SmolStr>,
    pub(super) cwd: Option<SmolStr>,
    pub(super) title: Option<SmolStr>,
    pub(super) watch_state: ProviderWatchState,
    pub(super) pending_partial: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ProviderWatchState {
    pub(crate) last_prompt_timestamp: Option<SmolStr>,
    #[cfg(feature = "codex")]
    event_fingerprints: VecDeque<u64>,
}

impl ProviderWatchState {
    pub(crate) fn with_last_prompt_timestamp(last_prompt_timestamp: Option<SmolStr>) -> Self {
        Self {
            last_prompt_timestamp,
            #[cfg(feature = "codex")]
            event_fingerprints: VecDeque::new(),
        }
    }

    #[cfg(feature = "codex")]
    pub(crate) fn insert_event_fingerprint(&mut self, fingerprint: u64) -> bool {
        if self
            .event_fingerprints
            .iter()
            .any(|existing| *existing == fingerprint)
        {
            return false;
        }
        if self.event_fingerprints.len() >= MAX_EVENT_FINGERPRINTS {
            self.event_fingerprints.pop_front();
        }
        self.event_fingerprints.push_back(fingerprint);
        true
    }
}

pub(super) fn split_complete_lines(mut bytes: Vec<u8>, pending_partial: &mut Vec<u8>) -> Vec<u8> {
    if bytes.is_empty() {
        return bytes;
    }
    let Some(last_newline) = bytes.iter().rposition(|byte| *byte == b'\n') else {
        *pending_partial = bytes;
        return Vec::new();
    };
    if last_newline + 1 < bytes.len() {
        *pending_partial = bytes.split_off(last_newline + 1);
    }
    bytes
}

pub(super) fn drop_leading_partial_line(bytes: &mut Vec<u8>) {
    let Some(first_newline) = bytes.iter().position(|byte| *byte == b'\n') else {
        bytes.clear();
        return;
    };
    bytes.drain(..=first_newline);
}
