use std::path::PathBuf;

use notify::Event;
use smol_str::SmolStr;

#[derive(Debug)]
pub(super) enum RawWatchEvent {
    Paths(Vec<PathBuf>),
    Error(SmolStr),
}

impl RawWatchEvent {
    pub(super) fn from_notify_result(result: notify::Result<Event>) -> Self {
        match result {
            Ok(event) => Self::Paths(event.paths),
            Err(error) => Self::Error(error.to_string().into()),
        }
    }
}
