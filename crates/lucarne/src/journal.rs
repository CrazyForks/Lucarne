//! Append-only event store per session.
//!
//! Contract:
//!
//! * `append` stamps `Seq` (monotonic within `(session, epoch)`).
//! * `since` returns all events with `Seq > cursor.seq`; if the stored
//!   epoch differs from the cursor's, the caller is told to reset.
//! * Appends are serialized per session. Cross-session inserts run
//!   concurrently.

use crate::event::Event;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use tracing::{debug, trace};

#[derive(Debug, Clone, Default)]
pub struct Cursor {
    pub epoch: String,
    pub seq: u64,
}

#[derive(Debug, Default, Clone)]
pub struct SinceResult {
    pub events: Vec<Event>,
    pub next_seq: u64,
    pub epoch: String,
    /// True when the stored epoch differs from the cursor's.
    pub epoch_reset: bool,
}

pub struct Memory {
    inner: RwLock<HashMap<String, Arc<Mutex<Stream>>>>,
}

#[derive(Default)]
struct Stream {
    epoch: String,
    next_seq: u64,
    events: Vec<Event>,
}

impl Default for Memory {
    fn default() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }
}

impl Memory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn append(&self, session_id: &str, mut ev: Event) -> Event {
        let stream = self.stream(session_id);
        let mut s = stream.lock().unwrap();
        if s.next_seq == 0 {
            s.next_seq = 1;
        }
        if !ev.epoch.is_empty() && ev.epoch != s.epoch {
            s.epoch = ev.epoch.clone();
            s.next_seq = 1;
        }
        ev.seq = s.next_seq;
        s.next_seq += 1;
        s.events.push(ev.clone());
        trace!(
            target: "lucarne::journal",
            session_id,
            epoch = ev.epoch.as_str(),
            seq = ev.seq,
            "journal event appended"
        );
        ev
    }

    pub fn since(&self, session_id: &str, cursor: Cursor, limit: usize) -> SinceResult {
        let stream = match self.inner.read().unwrap().get(session_id).cloned() {
            Some(stream) => stream,
            None => return SinceResult::default(),
        };
        let s = stream.lock().unwrap();
        let mut res = SinceResult {
            epoch: s.epoch.clone(),
            ..Default::default()
        };
        let mut cur_seq = cursor.seq;
        if !cursor.epoch.is_empty() && cursor.epoch != s.epoch {
            res.epoch_reset = true;
            cur_seq = 0;
        }
        let mut out: Vec<Event> = s
            .events
            .iter()
            .filter(|e| e.epoch == s.epoch && e.seq > cur_seq)
            .cloned()
            .collect();
        out.sort_by_key(|e| e.seq);
        if limit > 0 && out.len() > limit {
            out.truncate(limit);
        }
        res.next_seq = out.last().map(|e| e.seq).unwrap_or(cur_seq);
        res.events = out;
        debug!(
            target: "lucarne::journal",
            session_id,
            epoch = res.epoch.as_str(),
            next_seq = res.next_seq,
            returned = res.events.len(),
            epoch_reset = res.epoch_reset,
            "journal events served"
        );
        res
    }

    fn stream(&self, session_id: &str) -> Arc<Mutex<Stream>> {
        if let Some(stream) = self.inner.read().unwrap().get(session_id).cloned() {
            return stream;
        }
        let mut guard = self.inner.write().unwrap();
        guard
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(Stream::default())))
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, LogLine, Payload};

    fn log(epoch: &str, text: &str) -> Event {
        Event {
            seq: 0,
            epoch: epoch.into(),
            ts: String::new(),
            payload: Payload::Log(LogLine {
                level: "info".into(),
                stream: "stdout".into(),
                text: text.into(),
            }),
        }
    }

    #[test]
    fn memory_serializes_per_stream_not_global_map() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/journal.rs"),
        )
        .expect("read journal source");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("production source");

        assert!(
            production.contains("inner: RwLock<HashMap<String, Arc<Mutex<Stream>>>>"),
            "journal memory should lock the stream, not the whole store"
        );
        assert!(
            !production.contains("inner: Mutex<HashMap<String, Stream>>"),
            "journal memory must not serialize cross-session append through one global mutex"
        );
    }

    #[test]
    fn memory_emits_structured_tracing() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/journal.rs"),
        )
        .expect("read journal source");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("production source");

        for needle in [
            "lucarne::journal",
            "journal event appended",
            "journal events served",
        ] {
            assert!(
                production.contains(needle),
                "journal tracing must cover append/read boundary: {needle}"
            );
        }
    }

    #[test]
    fn append_stamps_seq() {
        let j = Memory::new();
        let a = j.append("s1", log("e1", "a"));
        let b = j.append("s1", log("e1", "b"));
        assert_eq!(a.seq, 1);
        assert_eq!(b.seq, 2);
    }

    #[test]
    fn epoch_rollover_resets_seq() {
        let j = Memory::new();
        j.append("s1", log("e1", "a"));
        let b = j.append("s1", log("e2", "b"));
        assert_eq!(b.seq, 1);
    }

    #[test]
    fn since_skips_to_cursor() {
        let j = Memory::new();
        j.append("s1", log("e1", "a"));
        j.append("s1", log("e1", "b"));
        j.append("s1", log("e1", "c"));
        let r = j.since(
            "s1",
            Cursor {
                epoch: "e1".into(),
                seq: 1,
            },
            0,
        );
        assert_eq!(r.events.len(), 2);
        assert_eq!(r.next_seq, 3);
        assert!(!r.epoch_reset);
    }

    #[test]
    fn since_detects_epoch_change() {
        let j = Memory::new();
        j.append("s1", log("e1", "a"));
        j.append("s1", log("e2", "b"));
        let r = j.since(
            "s1",
            Cursor {
                epoch: "e1".into(),
                seq: 5,
            },
            0,
        );
        assert!(r.epoch_reset);
        assert_eq!(r.events.len(), 1);
    }
}
