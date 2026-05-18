use smol_str::SmolStr;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use bytes::Bytes;
use serde::Deserialize;

use crate::Error;
use crate::agent::{AgentProviderSourceEntry, DiscoverableProvider};
#[cfg(feature = "agent_session")]
use crate::agent_session::SessionMeta;
use crate::providers::AgentProviderSource;
use crate::{Gemini, Result};
use tracing::trace;

const GEMINI_HOME_ENV: &str = "GEMINI_HOME";
const GEMINI_CONFIG_DIR_ENV: &str = "GEMINI_CONFIG_DIR";
const DEFAULT_GEMINI_HOME: &str = ".gemini";
const PROJECTS_FILE: &str = "projects.json";
const TMP_DIR: &str = "tmp";
const CHATS_DIR: &str = "chats";

impl DiscoverableProvider for Gemini {
    fn name() -> &'static str {
        Gemini::name()
    }

    fn default_roots() -> Vec<PathBuf> {
        vec![default_gemini_home()]
    }

    fn discover_in<I, P>(roots: I, emit: &mut dyn FnMut(AgentProviderSource)) -> Result<()>
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        let mut include_all = |_path: &Path| true;
        Self::discover_recent_in(roots, &mut include_all, emit)
    }

    fn discover_recent_in<I, P>(
        roots: I,
        is_recent: &mut dyn FnMut(&Path) -> bool,
        emit: &mut dyn FnMut(AgentProviderSource),
    ) -> Result<()>
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        let roots = roots.into_iter().map(Into::into).collect::<Vec<_>>();
        trace!(
            target: "agent_sessions::discovery",
            agent = Gemini::name(),
            roots = roots.len(),
            "discovering Gemini sessions"
        );
        let mut candidates = 0usize;

        for root in roots {
            if !root.is_dir() {
                continue;
            }

            let projects = read_project_records(&root)?;
            for project in projects {
                let chats_dir = gemini_chats_dir(&root, project.storage_name.as_ref());
                if !chats_dir.is_dir() {
                    continue;
                }

                for entry in walkdir::WalkDir::new(&chats_dir)
                    .min_depth(1)
                    .into_iter()
                    .filter_map(std::result::Result::ok)
                {
                    let path = entry.into_path();
                    if !path.is_file()
                        || !is_recent(&path)
                        || path.extension().is_none_or(|ext| ext != "json")
                    {
                        continue;
                    }

                    let file_name: SmolStr = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(|name| name.to_owned().into())
                        .unwrap_or_else(|| "session.json".into());

                    let mut entries = vec![
                        AgentProviderSourceEntry::new(path.clone())
                            .named(file_name)
                            .with_media_type("application/json"),
                    ];

                    if let Some(cwd) = project.cwd.as_ref() {
                        entries.push(
                            AgentProviderSourceEntry::new(gemini_projects_path(&root))
                                .named("project.json")
                                .with_kind("project_metadata")
                                .with_media_type("application/json")
                                .with_inline_data(Bytes::from(
                                    serde_json::json!({
                                        "cwd": cwd.display().to_string()
                                    })
                                    .to_string(),
                                )),
                        );
                    }

                    candidates += 1;
                    emit(AgentProviderSource::new(path, entries));
                }
            }
        }

        trace!(
            target: "agent_sessions::discovery",
            agent = Gemini::name(),
            candidates,
            "discovered Gemini sessions"
        );
        Ok(())
    }

    #[cfg(feature = "agent_session")]
    fn parse_candidate_entries_agent_session_meta(
        entries: &[AgentProviderSourceEntry],
    ) -> Result<SessionMeta> {
        let entry = entries
            .iter()
            .find(|entry| entry.kind.as_deref() != Some("project_metadata"))
            .or_else(|| entries.first())
            .ok_or(Error::EmptyInput)?;
        let raw = read_session_meta(entry)?;
        let cwd = entries
            .iter()
            .find(|entry| entry.kind.as_deref() == Some("project_metadata"))
            .map(read_inline_project_meta)
            .transpose()?
            .and_then(|meta| meta.cwd);
        Ok(SessionMeta {
            session_id: Some(raw.session_id.into()),
            cwd: cwd.map(|cwd| cwd.into()),
            title: raw.summary.map(Into::into),
            created_at: crate::agent_session::smol_opt(raw.start_time),
            updated_at: crate::agent_session::smol_opt(raw.last_updated),
            source_kind: raw.kind.map(Into::into),
            ..SessionMeta::default()
        })
    }

    #[cfg(feature = "agent_session")]
    fn parse_direct_agent_session_reader_selected<R>(
        reader: &mut R,
        metadata: crate::InputMetadata<'_>,
        selection: crate::ParseSelection,
    ) -> Result<Option<crate::agent_session::Session>>
    where
        R: std::io::BufRead,
    {
        super::event::parse_direct_agent_session_reader_selected(reader, metadata, selection)
            .map(Some)
    }
}

fn read_project_records(root: &Path) -> Result<Vec<ProjectRecord>> {
    let projects_path = gemini_projects_path(root);
    if projects_path.is_file() {
        return read_project_records_file(&projects_path);
    }

    let tmp_root = gemini_tmp_dir(root);
    if !tmp_root.is_dir() {
        return Ok(Vec::new());
    }

    Ok(std::fs::read_dir(tmp_root)?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            path.is_dir().then(|| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| ProjectRecord {
                        cwd: None,
                        storage_name: name.to_owned().into(),
                    })
            })?
        })
        .collect())
}

fn read_project_records_file(projects_path: &Path) -> Result<Vec<ProjectRecord>> {
    let signature = project_file_signature(projects_path)?;
    let key = projects_path.to_path_buf();

    if let Some(records) = project_record_cache()
        .read()
        .expect("gemini project cache")
        .get(&key)
        .filter(|entry| entry.signature == signature)
        .map(|entry| entry.records.clone())
    {
        return Ok(records);
    }

    let mut content = String::new();
    File::open(projects_path)?.read_to_string(&mut content)?;
    let projects: ProjectsFile = serde_json::from_str(&content)?;
    let records = projects
        .projects
        .into_iter()
        .map(|(cwd, storage_name)| ProjectRecord {
            cwd: Some(PathBuf::from(cwd)),
            storage_name: storage_name.into(),
        })
        .collect::<Vec<_>>();

    project_record_cache()
        .write()
        .expect("gemini project cache")
        .insert(
            key,
            ProjectRecordCacheEntry {
                signature,
                records: records.clone(),
            },
        );
    Ok(records)
}

fn project_file_signature(projects_path: &Path) -> Result<ProjectFileSignature> {
    let metadata = std::fs::metadata(projects_path)?;
    Ok(ProjectFileSignature {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn project_record_cache() -> &'static RwLock<BTreeMap<PathBuf, ProjectRecordCacheEntry>> {
    static CACHE: OnceLock<RwLock<BTreeMap<PathBuf, ProjectRecordCacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(BTreeMap::new()))
}

#[derive(Clone)]
struct ProjectRecordCacheEntry {
    signature: ProjectFileSignature,
    records: Vec<ProjectRecord>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ProjectFileSignature {
    len: u64,
    modified: Option<std::time::SystemTime>,
}

#[derive(Clone)]
struct ProjectRecord {
    cwd: Option<PathBuf>,
    storage_name: SmolStr,
}

fn default_gemini_home() -> PathBuf {
    std::env::var_os(GEMINI_HOME_ENV)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os(GEMINI_CONFIG_DIR_ENV).map(PathBuf::from))
        .or_else(|| crate::paths::home_dir().map(|home| home.join(DEFAULT_GEMINI_HOME)))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_GEMINI_HOME))
}

fn gemini_projects_path(home: &Path) -> PathBuf {
    home.join(PROJECTS_FILE)
}

fn gemini_tmp_dir(home: &Path) -> PathBuf {
    home.join(TMP_DIR)
}

fn gemini_chats_dir(home: &Path, storage_name: &str) -> PathBuf {
    gemini_tmp_dir(home).join(storage_name).join(CHATS_DIR)
}

#[cfg(feature = "agent_session")]
fn read_inline_project_meta(entry: &AgentProviderSourceEntry) -> Result<RawProjectMetadata> {
    let Some(data) = entry.inline_data.as_ref() else {
        return Ok(RawProjectMetadata { cwd: None });
    };
    Ok(serde_json::from_slice(data.as_ref())?)
}

#[cfg(feature = "agent_session")]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawGeminiSessionMeta {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(default)]
    start_time: Option<String>,
    #[serde(default)]
    last_updated: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    summary: Option<String>,
}

#[cfg(feature = "agent_session")]
fn read_session_meta(entry: &AgentProviderSourceEntry) -> Result<RawGeminiSessionMeta> {
    match entry.inline_data.as_ref() {
        Some(data) => parse_session_meta_json(data.as_ref()),
        None => {
            let mut file = File::open(&entry.path)?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            parse_session_meta_json(&bytes)
        }
    }
}

#[cfg(feature = "agent_session")]
fn parse_session_meta_json(bytes: &[u8]) -> Result<RawGeminiSessionMeta> {
    Ok(serde_json::from_slice(bytes)?)
}

#[cfg(feature = "agent_session")]
#[derive(Deserialize)]
struct RawProjectMetadata {
    #[serde(default)]
    cwd: Option<String>,
}

#[derive(Deserialize)]
struct ProjectsFile {
    projects: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};

    use super::*;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn restore_env(key: &str, value: Option<OsString>) {
        unsafe {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    fn default_roots_prefers_gemini_home_over_default_config_dir() {
        let _guard = env_lock().lock().unwrap();
        let previous_home = std::env::var_os("GEMINI_HOME");
        let previous_config = std::env::var_os("GEMINI_CONFIG_DIR");
        let gemini_home = std::env::temp_dir().join("agent-sessions-gemini-home");
        let config_dir = std::env::temp_dir().join("agent-sessions-gemini-config");

        unsafe {
            std::env::set_var("GEMINI_HOME", &gemini_home);
            std::env::set_var("GEMINI_CONFIG_DIR", &config_dir);
        }

        assert_eq!(Gemini::default_roots(), vec![gemini_home]);

        restore_env("GEMINI_HOME", previous_home);
        restore_env("GEMINI_CONFIG_DIR", previous_config);
    }

    #[cfg(feature = "agent_session")]
    #[test]
    fn parse_candidate_meta_keeps_summary_title() {
        let entry = AgentProviderSourceEntry::new(PathBuf::from("gemini-session.json"))
            .named("gemini-session.json")
            .with_media_type("application/json")
            .with_inline_data(
                "{\"sessionId\":\"gemini-fork\",\"summary\":\"Task: You are a delegated subagent running from a fork of the parent session. Treat the inherited conversation as reference-only context, not a live thread to continue.\\n\\nTask:\\nImplement the Gemini title parser.\",\"startTime\":\"2026-05-11T00:49:47.936Z\",\"lastUpdated\":\"2026-05-11T00:49:51.117Z\"}",
            );

        let meta =
            <Gemini as DiscoverableProvider>::parse_candidate_entries_agent_session_meta(&[entry])
                .unwrap();

        assert_eq!(meta.session_id.as_deref(), Some("gemini-fork"));
        assert_eq!(
            meta.title.as_deref(),
            Some(
                "Task: You are a delegated subagent running from a fork of the parent session. Treat the inherited conversation as reference-only context, not a live thread to continue.\n\nTask:\nImplement the Gemini title parser."
            )
        );
    }

    #[cfg(feature = "agent_session")]
    #[test]
    fn parse_candidate_meta_requires_valid_full_session_json() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("gemini-session.json");
        std::fs::write(
            &path,
            concat!(
                r#"{"sessionId":"gemini-hot-meta","summary":"Hot metadata only","startTime":"2026-05-15T00:00:00Z","lastUpdated":"2026-05-15T00:00:01Z","messages":["#,
                "\n",
                "this is intentionally not valid json after the metadata header"
            ),
        )
        .expect("write gemini session");
        let entry = AgentProviderSourceEntry::new(path)
            .named("gemini-session.json")
            .with_media_type("application/json");

        let error =
            <Gemini as DiscoverableProvider>::parse_candidate_entries_agent_session_meta(&[entry])
                .unwrap_err();

        assert!(matches!(error, Error::Message(_) | Error::Json(_)));
    }

    #[cfg(feature = "agent_session")]
    #[test]
    fn parse_candidate_meta_continues_after_large_contained_messages() {
        let mut raw = String::from(r#"{"messages":["#);
        for idx in 0..5000 {
            if idx > 0 {
                raw.push(',');
            }
            raw.push_str(
                &serde_json::json!({"role":"user","parts":[format!("hi-{idx:04}")]}).to_string(),
            );
        }
        raw.push_str(
            r#"],"sessionId":"gemini-contained-messages","summary":"Metadata after messages","lastUpdated":"2026-05-15T00:00:01Z"}"#,
        );
        let entry = AgentProviderSourceEntry::new(PathBuf::from("gemini-session.json"))
            .named("gemini-session.json")
            .with_media_type("application/json")
            .with_inline_data(raw);

        let meta =
            <Gemini as DiscoverableProvider>::parse_candidate_entries_agent_session_meta(&[entry])
                .unwrap();

        assert_eq!(
            meta.session_id.as_deref(),
            Some("gemini-contained-messages")
        );
        assert_eq!(meta.title.as_deref(), Some("Metadata after messages"));
        assert_eq!(meta.updated_at.as_deref(), Some("2026-05-15T00:00:01Z"));
    }

    #[cfg(feature = "agent_session")]
    #[test]
    fn parse_candidate_meta_ignores_truncated_optional_string() {
        let mut raw = String::from(
            r#"{"sessionId":"gemini-truncated-optional","summary":"Metadata before optional string","debug":""#,
        );
        raw.push_str(&"x".repeat(128 * 1024));
        raw.push_str(r#"","messages":[]}"#);
        let entry = AgentProviderSourceEntry::new(PathBuf::from("gemini-session.json"))
            .named("gemini-session.json")
            .with_media_type("application/json")
            .with_inline_data(raw);

        let meta =
            <Gemini as DiscoverableProvider>::parse_candidate_entries_agent_session_meta(&[entry])
                .unwrap();

        assert_eq!(
            meta.session_id.as_deref(),
            Some("gemini-truncated-optional")
        );
        assert_eq!(
            meta.title.as_deref(),
            Some("Metadata before optional string")
        );
    }

    #[cfg(feature = "agent_session")]
    #[test]
    fn parse_candidate_meta_rejects_truncated_session_id() {
        let mut raw = String::from(r#"{"sessionId":""#);
        raw.push_str(&"x".repeat(128 * 1024));
        let entry = AgentProviderSourceEntry::new(PathBuf::from("gemini-session.json"))
            .named("gemini-session.json")
            .with_media_type("application/json")
            .with_inline_data(raw);

        let error =
            <Gemini as DiscoverableProvider>::parse_candidate_entries_agent_session_meta(&[entry])
                .unwrap_err();

        assert!(matches!(error, Error::Message(_) | Error::Json(_)));
    }

    #[cfg(feature = "agent_session")]
    #[test]
    fn parse_candidate_meta_keeps_collected_fields_after_large_unused_container() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("gemini-session.json");
        let mut raw = String::from(
            r#"{"sessionId":"gemini-big-container","summary":"Hot metadata before directories","directories":["#,
        );
        for idx in 0..5000 {
            raw.push_str(&serde_json::to_string(&format!("/tmp/path-{idx:04}")).unwrap());
            raw.push(',');
        }
        raw.push_str(r#""/tmp/final"],"messages":[]}"#);
        std::fs::write(&path, raw).expect("write gemini session");
        let entry = AgentProviderSourceEntry::new(path)
            .named("gemini-session.json")
            .with_media_type("application/json");

        let meta =
            <Gemini as DiscoverableProvider>::parse_candidate_entries_agent_session_meta(&[entry])
                .unwrap();

        assert_eq!(meta.session_id.as_deref(), Some("gemini-big-container"));
        assert_eq!(
            meta.title.as_deref(),
            Some("Hot metadata before directories")
        );
    }
}
