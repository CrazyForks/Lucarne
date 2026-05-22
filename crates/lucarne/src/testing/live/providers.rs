use crate::adapter::ProtocolAdapter;
#[cfg(feature = "claude")]
use crate::adapters::claude;
#[cfg(feature = "gemini")]
use crate::adapters::gemini;
#[cfg(feature = "pi")]
use crate::adapters::pi;
#[cfg(feature = "codex")]
use crate::adapters::{codex, codex_env_overrides};
#[cfg(any(feature = "codex", feature = "gemini"))]
use crate::adapters::{merged_env_map, resolve_command_for_launch};
#[cfg(any(feature = "codex", feature = "gemini"))]
use crate::host::process::ManagedProcess;
use once_cell::sync::Lazy;
#[cfg(any(feature = "codex", feature = "gemini"))]
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(any(feature = "codex", feature = "gemini"))]
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

pub const LIVE_REPLAY_POST_TURN_QUIET: Duration = Duration::from_millis(50);
#[cfg(any(feature = "codex", feature = "gemini"))]
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
#[cfg(any(feature = "codex", feature = "gemini"))]
use tokio::process::Command;

#[cfg(feature = "codex")]
static LIVE_CODEX_MODEL: Lazy<String> = Lazy::new(|| {
    first_non_empty(&[
        std::env::var("LUCARNE_LIVE_CODEX_MODEL").ok(),
        Some(read_codex_configured_model(&live_codex_shared_home())),
        Some("gpt-5.4".into()),
    ])
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    #[cfg(feature = "claude")]
    Claude,
    #[cfg(feature = "codex")]
    Codex,
    #[cfg(feature = "gemini")]
    Gemini,
    #[cfg(feature = "pi")]
    Pi,
    #[cfg(not(any(
        feature = "claude",
        feature = "codex",
        feature = "gemini",
        feature = "pi"
    )))]
    Unavailable,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            #[cfg(feature = "claude")]
            Self::Claude => "claude",
            #[cfg(feature = "codex")]
            Self::Codex => "codex",
            #[cfg(feature = "gemini")]
            Self::Gemini => "gemini",
            #[cfg(feature = "pi")]
            Self::Pi => "pi",
            #[cfg(not(any(
                feature = "claude",
                feature = "codex",
                feature = "gemini",
                feature = "pi"
            )))]
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LiveProvider {
    pub kind: ProviderKind,
    pub model: String,
    pub binary: String,
}

static PREFLIGHT_CACHE: Lazy<Mutex<BTreeMap<String, Result<(), String>>>> =
    Lazy::new(|| Mutex::new(BTreeMap::new()));

impl LiveProvider {
    pub fn name(&self) -> &'static str {
        self.kind.as_str()
    }

    pub fn with_binary(&self, binary: impl Into<String>) -> Self {
        let mut next = self.clone();
        next.binary = binary.into();
        next
    }

    pub fn adapter(&self) -> Arc<ProtocolAdapter> {
        match self.kind {
            #[cfg(feature = "claude")]
            ProviderKind::Claude => claude::new(claude::Options {
                binary: self.binary.clone(),
            }),
            #[cfg(feature = "codex")]
            ProviderKind::Codex => codex::new(codex::Options {
                binary: self.binary.clone(),
            }),
            #[cfg(feature = "gemini")]
            ProviderKind::Gemini => gemini::new(gemini::Options {
                binary: self.binary.clone(),
            }),
            #[cfg(feature = "pi")]
            ProviderKind::Pi => pi::new(pi::Options {
                binary: self.binary.clone(),
            }),
            #[cfg(not(any(
                feature = "claude",
                feature = "codex",
                feature = "gemini",
                feature = "pi"
            )))]
            ProviderKind::Unavailable => unreachable!("no live provider is compiled"),
        }
    }

    pub fn timeout(&self) -> Duration {
        match self.kind {
            #[cfg(feature = "codex")]
            ProviderKind::Codex => Duration::from_secs(4 * 60),
            #[cfg(feature = "gemini")]
            ProviderKind::Gemini => Duration::from_secs(4 * 60),
            #[cfg(feature = "pi")]
            ProviderKind::Pi => Duration::from_secs(3 * 60),
            #[cfg(feature = "claude")]
            ProviderKind::Claude => Duration::from_secs(2 * 60),
            #[cfg(not(any(
                feature = "claude",
                feature = "codex",
                feature = "gemini",
                feature = "pi"
            )))]
            ProviderKind::Unavailable => Duration::from_secs(0),
        }
    }

    pub fn post_turn_quiet(&self) -> Duration {
        match self.kind {
            #[cfg(feature = "claude")]
            ProviderKind::Claude => Duration::from_secs(15),
            #[cfg(feature = "pi")]
            ProviderKind::Pi => Duration::from_secs(2),
            #[cfg(feature = "codex")]
            ProviderKind::Codex => Duration::from_millis(1500),
            #[cfg(feature = "gemini")]
            ProviderKind::Gemini => Duration::from_millis(1500),
            #[cfg(not(any(
                feature = "claude",
                feature = "codex",
                feature = "gemini",
                feature = "pi"
            )))]
            ProviderKind::Unavailable => Duration::from_millis(0),
        }
    }

    pub fn extra_args(&self, workdir: &Path) -> Vec<String> {
        let _ = workdir;
        match self.kind {
            #[cfg(feature = "claude")]
            ProviderKind::Claude => claude_allowed_dirs(workdir)
                .into_iter()
                .flat_map(|dir| ["--add-dir".to_string(), dir])
                .collect(),
            #[cfg(feature = "codex")]
            ProviderKind::Codex => Vec::new(),
            #[cfg(feature = "gemini")]
            ProviderKind::Gemini => Vec::new(),
            #[cfg(feature = "pi")]
            ProviderKind::Pi => Vec::new(),
            #[cfg(not(any(
                feature = "claude",
                feature = "codex",
                feature = "gemini",
                feature = "pi"
            )))]
            ProviderKind::Unavailable => Vec::new(),
        }
    }

    pub fn extra_env(
        &self,
        _temp_root: &Path,
        _workdir: &Path,
    ) -> Result<BTreeMap<String, String>, String> {
        match self.kind {
            #[cfg(feature = "codex")]
            ProviderKind::Codex => Ok(codex_env_overrides(&BTreeMap::new())),
            #[cfg(feature = "claude")]
            ProviderKind::Claude => Ok(BTreeMap::new()),
            #[cfg(feature = "gemini")]
            ProviderKind::Gemini => Ok(BTreeMap::new()),
            #[cfg(feature = "pi")]
            ProviderKind::Pi => Ok(BTreeMap::new()),
            #[cfg(not(any(
                feature = "claude",
                feature = "codex",
                feature = "gemini",
                feature = "pi"
            )))]
            ProviderKind::Unavailable => Ok(BTreeMap::new()),
        }
    }

    pub fn recording_env(&self, workdir: &Path) -> Result<BTreeMap<&'static str, PathBuf>, String> {
        let _ = workdir;
        match self.kind {
            #[cfg(feature = "codex")]
            ProviderKind::Codex => {
                let home = workdir.join(".codex-home");
                prepare_codex_recording_home(&home)?;
                Ok(BTreeMap::from([("CODEX_HOME", home)]))
            }
            #[cfg(feature = "claude")]
            ProviderKind::Claude => Ok(BTreeMap::new()),
            #[cfg(feature = "gemini")]
            ProviderKind::Gemini => Ok(BTreeMap::new()),
            #[cfg(feature = "pi")]
            ProviderKind::Pi => Ok(BTreeMap::new()),
            #[cfg(not(any(
                feature = "claude",
                feature = "codex",
                feature = "gemini",
                feature = "pi"
            )))]
            ProviderKind::Unavailable => Ok(BTreeMap::new()),
        }
    }
}

pub async fn preflight_live_provider(
    provider: &LiveProvider,
    temp_root: &Path,
    workdir: &Path,
) -> Result<(), String> {
    let timeout = match provider.kind {
        #[cfg(feature = "codex")]
        ProviderKind::Codex => Duration::from_secs(35),
        #[cfg(feature = "claude")]
        ProviderKind::Claude => Duration::from_secs(10),
        #[cfg(feature = "pi")]
        ProviderKind::Pi => Duration::from_secs(10),
        #[cfg(feature = "gemini")]
        ProviderKind::Gemini => Duration::from_secs(30),
        #[cfg(not(any(
            feature = "claude",
            feature = "codex",
            feature = "gemini",
            feature = "pi"
        )))]
        ProviderKind::Unavailable => Duration::from_secs(0),
    };
    preflight_live_provider_with_timeout(provider, temp_root, workdir, timeout).await
}

pub async fn preflight_live_provider_with_timeout(
    provider: &LiveProvider,
    temp_root: &Path,
    workdir: &Path,
    timeout: Duration,
) -> Result<(), String> {
    let _ = (temp_root, workdir, timeout);
    let cache_key = format!("{}:{}:{}", provider.name(), provider.binary, provider.model);
    if let Some(cached) = PREFLIGHT_CACHE.lock().unwrap().get(&cache_key).cloned() {
        return cached;
    }

    let result = match provider.kind {
        #[cfg(feature = "codex")]
        ProviderKind::Codex => {
            preflight_codex_app_server_turn(provider, temp_root, workdir, timeout).await
        }
        #[cfg(feature = "gemini")]
        ProviderKind::Gemini => {
            preflight_gemini_acp_initialize(provider, temp_root, workdir, timeout).await
        }
        #[cfg(feature = "claude")]
        ProviderKind::Claude => Ok(()),
        #[cfg(feature = "pi")]
        ProviderKind::Pi => Ok(()),
        #[cfg(not(any(
            feature = "claude",
            feature = "codex",
            feature = "gemini",
            feature = "pi"
        )))]
        ProviderKind::Unavailable => Ok(()),
    };
    PREFLIGHT_CACHE
        .lock()
        .unwrap()
        .insert(cache_key, result.clone());
    result
}

#[cfg(any(feature = "codex", feature = "gemini"))]
fn resolve_live_binary(
    binary: &str,
    workdir: &Path,
    extra_env: &BTreeMap<String, String>,
) -> Result<String, String> {
    let env = merged_env_map(extra_env);
    resolve_command_for_launch(binary, &workdir.to_string_lossy(), &env)
        .map_err(|err| err.to_string())
}

#[cfg(feature = "codex")]
async fn preflight_codex_app_server_turn(
    provider: &LiveProvider,
    temp_root: &Path,
    workdir: &Path,
    timeout: Duration,
) -> Result<(), String> {
    let extra_env = provider.extra_env(temp_root, workdir)?;
    let binary = resolve_live_binary(&provider.binary, workdir, &extra_env)?;
    let mut command = Command::new(&binary);
    command
        .arg("app-server")
        .arg("--listen")
        .arg("stdio://")
        .current_dir(workdir)
        .envs(extra_env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::host::process::configure_command(&mut command);
    let mut child = command
        .spawn()
        .map_err(|err| format!("codex preflight spawn({}): {err}", provider.binary))?;
    let managed_child = match ManagedProcess::attach(&child) {
        Ok(managed_child) => managed_child,
        Err(err) => {
            let stop_error = child
                .kill()
                .await
                .err()
                .map(|err| format!("kill preflight child after manage failure: {err}"));
            return Err(append_stop_error(
                format!("codex preflight manage({}): {err}", provider.binary),
                stop_error,
            ));
        }
    };
    let mut stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => {
            let stop_error = stop_preflight_child(&mut child, &managed_child).await.err();
            return Err(append_stop_error(
                "codex preflight stdin unavailable".to_string(),
                stop_error,
            ));
        }
    };
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            drop(stdin);
            let stop_error = stop_preflight_child(&mut child, &managed_child).await.err();
            return Err(append_stop_error(
                "codex preflight stdout unavailable".to_string(),
                stop_error,
            ));
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            drop(stdin);
            let stop_error = stop_preflight_child(&mut child, &managed_child).await.err();
            return Err(append_stop_error(
                "codex preflight stderr unavailable".to_string(),
                stop_error,
            ));
        }
    };
    let mut stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        let mut buf = String::new();
        let _ = reader.read_to_string(&mut buf).await;
        buf
    });

    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientInfo": {"name": "lucarne-live-preflight", "title": "lucarne", "version": "0.1.0"},
            "capabilities": {"experimentalApi": true}
        }
    });
    let initialized = json!({
        "jsonrpc": "2.0",
        "method": "initialized",
        "params": {}
    });
    let thread_start = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "thread/start",
        "params": {
            "cwd": workdir,
            "model": provider.model,
            "approvalPolicy": "never",
            "sandbox": "danger-full-access"
        }
    });

    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut seen_stdout = Vec::new();
    let result: Result<Result<(), String>, tokio::time::error::Elapsed> =
        tokio::time::timeout(timeout, async {
            write_json_line(&mut stdin, &initialize)
                .await
                .map_err(|err| format!("codex preflight write initialize: {err}"))?;
            write_json_line(&mut stdin, &initialized)
                .await
                .map_err(|err| format!("codex preflight write initialized: {err}"))?;
            write_json_line(&mut stdin, &thread_start)
                .await
                .map_err(|err| format!("codex preflight write thread/start: {err}"))?;

            let mut turn_started = false;
            loop {
                let Some(line) = stdout_lines
                    .next_line()
                    .await
                    .map_err(|err| format!("codex preflight read stdout: {err}"))?
                else {
                    return Err("raw `codex app-server` exited before completing a minimal turn".into());
                };
                push_snippet(&mut seen_stdout, &line);
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                if value.get("id").and_then(|id| id.as_i64()) == Some(2) {
                    let thread_id = value
                        .get("result")
                        .and_then(|result| result.get("thread"))
                        .and_then(|thread| thread.get("id"))
                        .and_then(|id| id.as_str())
                        .map(str::to_owned);
                    let Some(thread_id) = thread_id.clone() else {
                        return Err("codex preflight thread/start did not return thread id".into());
                    };
                    let turn_start = json!({
                        "jsonrpc": "2.0",
                        "id": 3,
                        "method": "turn/start",
                        "params": {
                            "threadId": thread_id,
                            "cwd": workdir,
                            "model": provider.model,
                            "input": [{"type": "text", "text": "Reply with exactly LIVE_OK and nothing else."}]
                        }
                    });
                    write_json_line(&mut stdin, &turn_start)
                        .await
                        .map_err(|err| format!("codex preflight write turn/start: {err}"))?;
                    turn_started = true;
                    continue;
                }
                if !turn_started {
                    continue;
                }
                if value.get("id").and_then(|id| id.as_i64()) == Some(3) {
                    continue;
                }
                if value.get("method").and_then(|method| method.as_str()) == Some("error") {
                    if value
                        .pointer("/params/error/codexErrorInfo/responseStreamDisconnected")
                        .is_some()
                    {
                        return Err(
                            "raw `codex app-server` disconnected its response stream during a minimal text-only turn".into(),
                        );
                    }
                }
                if value.get("method").and_then(|method| method.as_str()) == Some("item/completed")
                {
                    let item = value.pointer("/params/item");
                    if item
                        .and_then(|item| item.get("type"))
                        .and_then(|ty| ty.as_str())
                        == Some("agentMessage")
                    {
                        return Ok(());
                    }
                }
                if value.get("method").and_then(|method| method.as_str())
                    == Some("turn/completed")
                {
                    let status = value
                        .pointer("/params/turn/status")
                        .and_then(|status| status.as_str());
                    if status == Some("completed") || status == Some("done") {
                        return Ok(());
                    }
                    return Err(format!(
                        "raw `codex app-server` completed a minimal turn with unexpected status {}",
                        status.unwrap_or("<missing>")
                    ));
                }
            }
        })
        .await;

    drop(stdin);
    let stop_error = stop_preflight_child(&mut child, &managed_child).await.err();
    let mut stderr_summary = summarize_snippet(drain_preflight_stderr(&mut stderr_task).await);
    if let Some(err) = stop_error {
        if stderr_summary.is_empty() || stderr_summary == "<none>" {
            stderr_summary = err;
        } else {
            stderr_summary = format!("{stderr_summary}; {err}");
        }
    }

    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(format!(
            "codex preflight failed: {err}; stdout={}{}",
            summarize_lines(&seen_stdout),
            format_stderr_suffix(&stderr_summary),
        )),
        Err(_) => Err(format!(
            "codex preflight failed: raw `codex app-server` did not complete a minimal text-only turn within {}; stdout={}{}",
            format_duration(timeout),
            summarize_lines(&seen_stdout),
            format_stderr_suffix(&stderr_summary),
        )),
    }
}

pub fn configured_live_providers() -> Vec<LiveProvider> {
    let mut providers = ["claude", "codex", "gemini", "pi"]
        .into_iter()
        .filter_map(live_provider_by_name)
        .collect::<Vec<_>>();

    let allowed = std::env::var("LUCARNE_LIVE_PROVIDERS")
        .ok()
        .map(|raw| {
            raw.split(',')
                .map(|part| part.trim().to_string())
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !allowed.is_empty() {
        providers.retain(|provider| allowed.iter().any(|name| name == provider.name()));
    }
    providers
}

pub fn live_provider_by_name(name: &str) -> Option<LiveProvider> {
    match name {
        #[cfg(feature = "claude")]
        "claude" => Some(LiveProvider {
            kind: ProviderKind::Claude,
            model: first_non_empty(&[
                std::env::var("LUCARNE_LIVE_CLAUDE_MODEL").ok(),
                Some("claude-haiku-4-5".into()),
            ]),
            binary: live_claude_binary(),
        }),
        #[cfg(feature = "codex")]
        "codex" => Some(LiveProvider {
            kind: ProviderKind::Codex,
            model: LIVE_CODEX_MODEL.clone(),
            binary: std::env::var("LUCARNE_LIVE_CODEX_BIN")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "codex".into()),
        }),
        #[cfg(feature = "gemini")]
        "gemini" => Some(LiveProvider {
            kind: ProviderKind::Gemini,
            model: first_non_empty(&[
                std::env::var("LUCARNE_LIVE_GEMINI_MODEL").ok(),
                Some("gemini-2.5-flash-lite".into()),
            ]),
            binary: std::env::var("LUCARNE_LIVE_GEMINI_BIN")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "gemini".into()),
        }),
        #[cfg(feature = "pi")]
        "pi" => Some(LiveProvider {
            kind: ProviderKind::Pi,
            model: first_non_empty(&[
                std::env::var("LUCARNE_LIVE_PI_MODEL").ok(),
                Some("deepseek/deepseek-v4-flash".into()),
            ]),
            binary: live_pi_binary(),
        }),
        _ => None,
    }
}

pub fn live_tool_prompt(
    provider_name: &str,
    workdir: &Path,
    readme_path: &Path,
    output_path: &Path,
) -> String {
    let readme = path_for_prompt(workdir, readme_path);
    let output = path_for_prompt(workdir, output_path);
    match provider_name {
        "claude" => format!(
            "Use tools, do not simulate. Read {} using the Read tool, then attempt to create {} containing exactly lucarne-live-tool on one line using the Write tool. If permission approval is required, ask once. After approval is received, reply with exactly TOOL_OK and stop. Do not switch to Bash, tee, or alternative write methods.",
            readme_path.display(),
            output_path.display(),
        ),
        _ => format!(
            "Use tools, do not simulate. Read {readme}, then create {output} containing exactly lucarne-live-tool on one line. Write directly to that exact path. After the file exists, reply with exactly TOOL_OK."
        ),
    }
}

pub fn live_delete_prompt(provider_name: &str, workdir: &Path, target_path: &Path) -> String {
    let target = path_for_prompt(workdir, target_path);
    match provider_name {
        "claude" => format!(
            "Use tools, do not simulate. Use a shell command to delete {target}. Do not use write, edit, or patch tools. After the file is gone, reply with exactly DELETE_OK."
        ),
        "gemini" => format!(
            "Use tools, do not simulate. Use a shell or terminal command to delete {target}. Do not use write, edit, or patch tools. After the file is gone, reply with exactly DELETE_OK."
        ),
        "codex" => format!(
            "Use tools, do not simulate. Use a shell command to delete {target}. Do not ask a natural-language approval question first. Do not use write, edit, or patch tools. After the file is gone, reply with exactly DELETE_OK."
        ),
        _ => format!(
            "Use tools, do not simulate. Use a shell or terminal command to delete {target}. Do not use write, edit, or patch tools. After the file is gone, reply with exactly DELETE_OK."
        ),
    }
}

pub fn live_question_prompt(provider_name: &str) -> String {
    match provider_name {
        "claude" => "Use tools, do not simulate. Your first action must be the AskUserQuestion tool. Do not answer directly and do not emit plain text before the tool call. Ask exactly one clarifying question with exactly two answer options. After the question is answered, reply with exactly QUESTION_OK.".into(),
        "codex" => "This is a two-turn protocol. In your first assistant message, output exactly this question and nothing else: Which response style should I use next? `brief` or `detailed`? Do not output QUESTION_OK in the first assistant message. Wait for the user's next message. After the user answers, reply with exactly QUESTION_OK and nothing else.".into(),
        _ => "Ask exactly this clarifying question and nothing else before it: Which response style should I use next? `brief` or `detailed`? Do not ask follow-up questions, do not reinterpret the answer as a new task, and do not do any other work before the question. After I answer, reply with exactly QUESTION_OK and nothing else.".into(),
    }
}

pub fn live_failure_prompt(provider_name: &str) -> String {
    match provider_name {
        "claude" => "Use tools, do not simulate. Use Bash to run `cat ./missing-file.txt` in the current working directory. Do not recover or switch tools. After the tool error, reply with exactly FAIL_OK.".into(),
        "codex" => "Use tools, do not simulate. Use a shell command to run `cat ./missing-file.txt` in the current working directory. Do not recover or switch tools. After the tool error, reply with exactly FAIL_OK.".into(),
        _ => "Use tools, do not simulate. Do not use any shell or terminal command. Use an available file-reading tool to read /private/tmp/lucarne-definitely-missing.txt. Do not recover or switch tools. After the tool error, reply with exactly FAIL_OK.".into(),
    }
}

fn path_for_prompt(workdir: &Path, path: &Path) -> String {
    if let Ok(rel) = path.strip_prefix(workdir) {
        let rel = rel.to_string_lossy();
        if rel.is_empty() {
            ".".into()
        } else if rel.starts_with('.') {
            rel.into_owned()
        } else {
            format!("./{rel}")
        }
    } else {
        path.display().to_string()
    }
}

pub fn live_claude_binary() -> String {
    if let Some(override_bin) = std::env::var("LUCARNE_LIVE_CLAUDE_BIN")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        return override_bin;
    }
    for candidate in [
        "/Users/era/Library/pnpm/claude",
        "/Users/era/.local/bin/claude",
        "claude",
    ] {
        if candidate == "claude" {
            return candidate.into();
        }
        if Path::new(candidate).exists() {
            return candidate.into();
        }
    }
    "claude".into()
}

pub fn live_pi_binary() -> String {
    std::env::var("LUCARNE_LIVE_PI_BIN")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "pi".into())
}

pub fn live_codex_shared_home() -> String {
    if let Some(home) = std::env::var("CODEX_HOME")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        return home;
    }
    std::env::var("HOME")
        .map(|home| {
            PathBuf::from(home)
                .join(".codex")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default()
}

pub fn read_codex_configured_model(shared_home: &str) -> String {
    if shared_home.trim().is_empty() {
        return String::new();
    }
    let path = Path::new(shared_home).join("config.toml");
    let Ok(raw) = fs::read_to_string(path) else {
        return String::new();
    };
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "model" {
            continue;
        }
        let value = value.trim();
        if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
            return value[1..value.len() - 1].to_string();
        }
        return value.to_string();
    }
    String::new()
}

#[cfg(feature = "codex")]
fn prepare_codex_recording_home(home: &Path) -> Result<(), String> {
    fs::create_dir_all(home).map_err(|err| format!("mkdir {}: {err}", home.display()))?;
    let shared_home = live_codex_shared_home();
    if shared_home.trim().is_empty() {
        return Ok(());
    }
    let shared = Path::new(&shared_home);
    for name in ["config.toml", "auth.json", "credentials.json"] {
        let source = shared.join(name);
        if !source.is_file() {
            continue;
        }
        fs::copy(&source, home.join(name)).map_err(|err| {
            format!(
                "copy Codex recording home file {} -> {}: {err}",
                source.display(),
                home.join(name).display()
            )
        })?;
    }
    Ok(())
}

pub fn claude_allowed_dirs(workdir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let cleaned = workdir.to_path_buf();
    let canonical = fs::canonicalize(workdir).unwrap_or_else(|_| cleaned.clone());
    for candidate in [cleaned, canonical] {
        let text = candidate.to_string_lossy().into_owned();
        if !out.contains(&text) {
            out.push(text);
        }
    }
    out
}

#[cfg(any(
    feature = "claude",
    feature = "codex",
    feature = "gemini",
    feature = "pi"
))]
fn first_non_empty(candidates: &[Option<String>]) -> String {
    candidates
        .iter()
        .filter_map(|value| value.as_ref())
        .find(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or_default()
}

#[cfg(feature = "gemini")]
async fn preflight_gemini_acp_initialize(
    provider: &LiveProvider,
    temp_root: &Path,
    workdir: &Path,
    timeout: Duration,
) -> Result<(), String> {
    let extra_env = provider.extra_env(temp_root, workdir)?;
    let binary = resolve_live_binary(&provider.binary, workdir, &extra_env)?;
    let mut command = Command::new(&binary);
    command
        .arg("--acp")
        .current_dir(workdir)
        .envs(extra_env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::host::process::configure_command(&mut command);
    let mut child = command
        .spawn()
        .map_err(|err| format!("gemini ACP preflight spawn({}): {err}", provider.binary))?;
    let managed_child = match ManagedProcess::attach(&child) {
        Ok(managed_child) => managed_child,
        Err(err) => {
            let stop_error = child
                .kill()
                .await
                .err()
                .map(|err| format!("kill preflight child after manage failure: {err}"));
            return Err(append_stop_error(
                format!("gemini ACP preflight manage({}): {err}", provider.binary),
                stop_error,
            ));
        }
    };
    let mut stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => {
            let stop_error = stop_preflight_child(&mut child, &managed_child).await.err();
            return Err(append_stop_error(
                "gemini ACP preflight stdin unavailable".to_string(),
                stop_error,
            ));
        }
    };
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            drop(stdin);
            let stop_error = stop_preflight_child(&mut child, &managed_child).await.err();
            return Err(append_stop_error(
                "gemini ACP preflight stdout unavailable".to_string(),
                stop_error,
            ));
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            drop(stdin);
            let stop_error = stop_preflight_child(&mut child, &managed_child).await.err();
            return Err(append_stop_error(
                "gemini ACP preflight stderr unavailable".to_string(),
                stop_error,
            ));
        }
    };
    let mut stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        let mut buf = String::new();
        let _ = reader.read_to_string(&mut buf).await;
        buf
    });

    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientInfo": {"name": "lucarne-live-preflight", "title": "lucarne", "version": "0.1.0"},
            "clientCapabilities": {
                "auth": {"terminal": false},
                "fs": {"readTextFile": false, "writeTextFile": false},
                "terminal": false
            }
        }
    });

    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut seen_stdout = Vec::new();
    let result: Result<Result<bool, String>, tokio::time::error::Elapsed> =
        tokio::time::timeout(timeout, async {
            let mut payload =
                serde_json::to_vec(&init).map_err(|err| format!("serialize initialize: {err}"))?;
            payload.push(b'\n');
            stdin
                .write_all(&payload)
                .await
                .map_err(|err| format!("gemini ACP preflight write initialize: {err}"))?;
            stdin
                .flush()
                .await
                .map_err(|err| format!("gemini ACP preflight flush initialize: {err}"))?;

            loop {
                let Some(line) = stdout_lines
                    .next_line()
                    .await
                    .map_err(|err| format!("gemini ACP preflight read stdout: {err}"))?
                else {
                    return Ok(false);
                };
                push_snippet(&mut seen_stdout, &line);
                if is_initialize_result(&line) {
                    return Ok(true);
                }
            }
        })
        .await;

    drop(stdin);
    let stop_error = stop_preflight_child(&mut child, &managed_child).await.err();
    let mut stderr_summary = summarize_snippet(drain_preflight_stderr(&mut stderr_task).await);
    if let Some(err) = stop_error {
        if stderr_summary.is_empty() || stderr_summary == "<none>" {
            stderr_summary = err;
        } else {
            stderr_summary = format!("{stderr_summary}; {err}");
        }
    }

    match result {
        Ok(Ok(true)) => Ok(()),
        Ok(Ok(false)) => Err(format!(
            "gemini ACP preflight failed: raw `gemini --acp` exited before answering initialize; stdout={}{}",
            summarize_lines(&seen_stdout),
            format_stderr_suffix(&stderr_summary),
        )),
        Ok(Err(err)) => Err(format!(
            "gemini ACP preflight failed: {err}{}",
            format_stderr_suffix(&stderr_summary),
        )),
        Err(_) => Err(format!(
            "gemini ACP preflight failed: raw `gemini --acp` did not answer initialize within {}; stdout={}{}",
            format_duration(timeout),
            summarize_lines(&seen_stdout),
            format_stderr_suffix(&stderr_summary),
        )),
    }
}

#[cfg(feature = "gemini")]
fn is_initialize_result(line: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return false;
    };
    value.get("id").and_then(|id| id.as_i64()) == Some(1) && value.get("result").is_some()
}

#[cfg(feature = "codex")]
async fn write_json_line(
    stdin: &mut tokio::process::ChildStdin,
    value: &serde_json::Value,
) -> Result<(), std::io::Error> {
    let mut payload = serde_json::to_vec(value)?;
    payload.push(b'\n');
    stdin.write_all(&payload).await?;
    stdin.flush().await
}

#[cfg(any(feature = "codex", feature = "gemini"))]
fn push_snippet(lines: &mut Vec<String>, line: &str) {
    if lines.len() == 4 {
        lines.remove(0);
    }
    let mut text = line.trim().to_string();
    if text.len() > 120 {
        text.truncate(120);
        text.push_str("...");
    }
    lines.push(text);
}

#[cfg(any(feature = "codex", feature = "gemini"))]
fn summarize_lines(lines: &[String]) -> String {
    if lines.is_empty() {
        "<none>".into()
    } else {
        lines.join(" | ")
    }
}

#[cfg(any(feature = "codex", feature = "gemini"))]
fn summarize_snippet(raw: String) -> String {
    let mut lines = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        push_snippet(&mut lines, line);
    }
    summarize_lines(&lines)
}

#[cfg(any(feature = "codex", feature = "gemini"))]
fn format_stderr_suffix(stderr: &str) -> String {
    if stderr.is_empty() || stderr == "<none>" {
        String::new()
    } else {
        format!("; stderr={stderr}")
    }
}

#[cfg(any(feature = "codex", feature = "gemini"))]
fn append_stop_error(mut message: String, stop_error: Option<String>) -> String {
    if let Some(err) = stop_error {
        message.push_str("; ");
        message.push_str(&err);
    }
    message
}

#[cfg(any(feature = "codex", feature = "gemini"))]
fn format_duration(duration: Duration) -> String {
    if duration.subsec_nanos() == 0 {
        format!("{}s", duration.as_secs())
    } else if duration.as_millis() < 1000 {
        format!("{}ms", duration.as_millis())
    } else {
        format!("{:.1}s", duration.as_secs_f64())
    }
}

#[cfg(any(feature = "codex", feature = "gemini"))]
async fn stop_preflight_child(
    child: &mut tokio::process::Child,
    managed_child: &ManagedProcess,
) -> Result<(), String> {
    let pid = child
        .id()
        .map(|pid| pid as i32)
        .ok_or_else(|| "missing preflight pid".to_string())?;
    let _ = managed_child.terminate_graceful(pid);
    match tokio::time::timeout(Duration::from_millis(250), child.wait()).await {
        Ok(Ok(_)) => return Ok(()),
        Ok(Err(err)) => return Err(format!("wait preflight child: {err}")),
        Err(_) => {}
    }

    let _ = managed_child.terminate_force(pid);
    tokio::time::timeout(Duration::from_millis(250), child.wait())
        .await
        .map_err(|_| "timed out waiting for preflight process tree to exit".to_string())?
        .map_err(|err| format!("wait preflight child: {err}"))?;
    Ok(())
}

#[cfg(any(feature = "codex", feature = "gemini"))]
async fn drain_preflight_stderr(stderr_task: &mut tokio::task::JoinHandle<String>) -> String {
    match tokio::time::timeout(Duration::from_millis(250), &mut *stderr_task).await {
        Ok(Ok(stderr)) => stderr,
        Ok(Err(_)) => String::new(),
        Err(_) => {
            stderr_task.abort();
            String::new()
        }
    }
}
