use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateState {
    pub last_checked_at_unix_secs: Option<u64>,
    pub last_latest_version: Option<String>,
    pub last_latest_url: Option<String>,
    pub last_notified_version: Option<String>,
    pub last_notified_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct UpdateStateStore {
    path: PathBuf,
}

impl UpdateStateStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<UpdateState, io::Error> {
        let raw = match fs::read_to_string(&self.path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(UpdateState::default()),
            Err(err) => return Err(err),
        };
        serde_json::from_str(&raw).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }

    pub fn save(&self, state: &UpdateState) -> Result<(), io::Error> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let file_name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_else(|| "update-state.json".into());
        static NEXT_TMP_ID: AtomicU64 = AtomicU64::new(0);
        let tmp_id = NEXT_TMP_ID.fetch_add(1, Ordering::Relaxed);
        let tmp_path = parent.join(format!(".{file_name}.tmp-{}-{tmp_id}", std::process::id()));
        let bytes = serde_json::to_vec_pretty(state)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

        let result = (|| {
            let mut file = fs::File::create(&tmp_path)?;
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            drop(file);
            fs::rename(&tmp_path, &self.path)
        })();

        if result.is_err() {
            let _ = fs::remove_file(&tmp_path);
        }
        result
    }
}

pub fn should_notify(
    state: &UpdateState,
    latest_version: &str,
    now_unix_secs: u64,
    remind_interval_secs: u64,
) -> bool {
    if state.last_notified_version.as_deref() != Some(latest_version) {
        return true;
    }

    let Some(last_notified_at) = state.last_notified_at_unix_secs else {
        return true;
    };

    now_unix_secs.saturating_sub(last_notified_at) >= remind_interval_secs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_state_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "lucarned-ctl-update-state-{name}-{}-{unique}.json",
            std::process::id()
        ))
    }

    #[test]
    fn missing_file_loads_default() {
        let store = UpdateStateStore::new(temp_state_path("missing"));
        assert_eq!(store.load().unwrap(), UpdateState::default());
    }

    #[test]
    fn malformed_file_returns_error() {
        let path = temp_state_path("malformed");
        fs::write(&path, "not json").unwrap();
        let store = UpdateStateStore::new(path.clone());
        assert!(store.load().is_err());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn save_then_load_round_trips() {
        let path = temp_state_path("roundtrip");
        let store = UpdateStateStore::new(path.clone());
        let state = UpdateState {
            last_checked_at_unix_secs: Some(10),
            last_latest_version: Some("0.2.0".to_string()),
            last_latest_url: Some("https://example.invalid/release".to_string()),
            last_notified_version: Some("0.2.0".to_string()),
            last_notified_at_unix_secs: Some(11),
        };

        store.save(&state).unwrap();
        assert_eq!(store.load().unwrap(), state);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn same_version_inside_interval_suppresses() {
        let state = UpdateState {
            last_notified_version: Some("0.2.0".to_string()),
            last_notified_at_unix_secs: Some(100),
            ..UpdateState::default()
        };
        assert!(!should_notify(&state, "0.2.0", 150, 100));
    }

    #[test]
    fn same_version_after_interval_notifies() {
        let state = UpdateState {
            last_notified_version: Some("0.2.0".to_string()),
            last_notified_at_unix_secs: Some(100),
            ..UpdateState::default()
        };
        assert!(should_notify(&state, "0.2.0", 201, 100));
    }

    #[test]
    fn different_version_notifies_immediately() {
        let state = UpdateState {
            last_notified_version: Some("0.2.0".to_string()),
            last_notified_at_unix_secs: Some(100),
            ..UpdateState::default()
        };
        assert!(should_notify(&state, "0.3.0", 101, 100));
    }
}
