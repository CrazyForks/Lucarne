use std::path::Path;

use crate::{
    autostart, paths,
    process::{run, CommandSpec},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckLevel {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub level: CheckLevel,
    pub name: &'static str,
    pub message: String,
}

pub fn run_doctor() -> Result<(), String> {
    let checks = collect_checks()?;
    print_checks(&checks);
    if checks.iter().any(|check| check.level == CheckLevel::Fail) {
        Err("doctor found critical failures".to_string())
    } else {
        Ok(())
    }
}

#[cfg(feature = "updates")]
pub async fn run_doctor_async(
    client: &reqwest::Client,
    update_config: crate::updates::UpdateConfig,
    update_config_warning: Option<String>,
) -> Result<(), String> {
    let mut checks = collect_checks()?;
    if let Some(message) = update_config_warning {
        checks.push(Check {
            level: CheckLevel::Warn,
            name: "update-config",
            message,
        });
    }
    checks.push(update_check(client, update_config).await);
    print_checks(&checks);
    if checks.iter().any(|check| check.level == CheckLevel::Fail) {
        Err("doctor found critical failures".to_string())
    } else {
        Ok(())
    }
}

fn print_checks(checks: &[Check]) {
    for check in checks {
        let prefix = match check.level {
            CheckLevel::Ok => "ok",
            CheckLevel::Warn => "warn",
            CheckLevel::Fail => "fail",
        };
        println!("{prefix}: {}: {}", check.name, check.message);
    }
}

fn collect_checks() -> Result<Vec<Check>, String> {
    let info = paths::current_path_info(None)?;
    let mut checks = Vec::new();
    match &info.lucarned {
        Some(path) => {
            checks.push(Check {
                level: CheckLevel::Ok,
                name: "lucarned",
                message: path.display().to_string(),
            });
            checks.push(check_lucarned_help(path));
        }
        None => checks.push(Check {
            level: CheckLevel::Fail,
            name: "lucarned",
            message: "not found on PATH or next to current executable".to_string(),
        }),
    }
    checks.push(path_exists_or_parent(&info.config_file, "config"));
    checks.push(dir_or_parent_writable(&info.log_dir, "logs"));
    checks.push(autostart_check(&info));
    add_platform_checks(&mut checks);
    for agent in ["codex", "claude", "pi", "gemini", "copilot"] {
        checks.push(optional_cli(agent));
    }
    Ok(checks)
}

#[cfg(feature = "updates")]
async fn update_check(
    client: &reqwest::Client,
    update_config: crate::updates::UpdateConfig,
) -> Check {
    if !update_config.enabled {
        return Check {
            level: CheckLevel::Ok,
            name: "update",
            message: "checks disabled by config".to_string(),
        };
    }

    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        crate::updates::check_now(client, &update_config, env!("CARGO_PKG_VERSION")),
    )
    .await
    {
        Ok(Ok(status)) => {
            let (level, message) = update_check_message(&status);
            Check {
                level,
                name: "update",
                message,
            }
        }
        Ok(Err(err)) => Check {
            level: CheckLevel::Warn,
            name: "update",
            message: format!("check failed: {err}"),
        },
        Err(_) => Check {
            level: CheckLevel::Warn,
            name: "update",
            message: "check failed: timed out after 10s".to_string(),
        },
    }
}

#[cfg(feature = "updates")]
fn update_check_message(status: &crate::updates::UpdateStatus) -> (CheckLevel, String) {
    if !status.automatic_checks_enabled {
        return (CheckLevel::Ok, "checks disabled by config".to_string());
    }

    if status.is_newer {
        let latest = status.latest_version.as_deref().unwrap_or("unknown");
        let url = status
            .release_url
            .as_deref()
            .unwrap_or("release URL unavailable");
        return (CheckLevel::Warn, format!("{latest} available: {url}"));
    }

    if let Some(latest) = &status.latest_version {
        return (
            CheckLevel::Ok,
            format!(
                "current version {} (latest {latest})",
                status.current_version
            ),
        );
    }

    (
        CheckLevel::Warn,
        "no stable GitHub release found".to_string(),
    )
}

fn check_lucarned_help(path: &Path) -> Check {
    let spec = CommandSpec::new(path.as_os_str()).arg("--help");
    match run(&spec) {
        Ok(result) if result.code == Some(0) => Check {
            level: CheckLevel::Ok,
            name: "lucarned-help",
            message: "lucarned --help succeeded".to_string(),
        },
        Ok(result) => Check {
            level: CheckLevel::Fail,
            name: "lucarned-help",
            message: format!("lucarned --help exited with {:?}", result.code),
        },
        Err(err) => Check {
            level: CheckLevel::Fail,
            name: "lucarned-help",
            message: err.to_string(),
        },
    }
}

fn path_exists_or_parent(path: &Path, name: &'static str) -> Check {
    if path.exists() {
        Check {
            level: CheckLevel::Ok,
            name,
            message: path.display().to_string(),
        }
    } else if path.parent().is_some_and(Path::exists) {
        Check {
            level: CheckLevel::Warn,
            name,
            message: format!("{} missing; parent exists", path.display()),
        }
    } else {
        Check {
            level: CheckLevel::Warn,
            name,
            message: format!(
                "{} missing; run lucarned init or start lucarned once",
                path.display()
            ),
        }
    }
}

fn dir_or_parent_writable(path: &Path, name: &'static str) -> Check {
    if path.is_dir() {
        Check {
            level: CheckLevel::Ok,
            name,
            message: path.display().to_string(),
        }
    } else if path.parent().is_some_and(Path::exists) {
        Check {
            level: CheckLevel::Warn,
            name,
            message: format!("{} missing; parent exists", path.display()),
        }
    } else {
        Check {
            level: CheckLevel::Warn,
            name,
            message: format!("{} missing", path.display()),
        }
    }
}

fn autostart_check(info: &paths::PathInfo) -> Check {
    let (installed, summary) = autostart::status_summary();
    let level = if installed {
        CheckLevel::Ok
    } else {
        CheckLevel::Warn
    };
    Check {
        level,
        name: "autostart",
        message: format!(
            "{} {} ({})",
            info.autostart_kind,
            info.autostart_entry.display(),
            summary
        ),
    }
}

fn optional_cli(name: &'static str) -> Check {
    match paths::find_on_path(name) {
        Some(path) => Check {
            level: CheckLevel::Ok,
            name,
            message: path.display().to_string(),
        },
        None => Check {
            level: CheckLevel::Warn,
            name,
            message: "optional agent CLI not found on PATH".to_string(),
        },
    }
}

#[cfg(target_os = "linux")]
fn add_platform_checks(checks: &mut Vec<Check>) {
    checks.push(systemctl_user_check());
    checks.push(env_check(
        "xdg-runtime-dir",
        "XDG_RUNTIME_DIR",
        "missing; systemd user sessions and headless autostart may need `loginctl enable-linger $USER`",
    ));
    checks.push(required_linux_tool(
        "loginctl",
        &["/usr/bin/loginctl", "/bin/loginctl"],
        "install systemd/loginctl or use manual autostart on non-systemd Linux",
    ));
    checks.push(required_linux_tool(
        "ps",
        &["/usr/bin/ps", "/bin/ps"],
        linux_package_hint(),
    ));
    checks.push(required_linux_tool(
        "lsof",
        &["/usr/bin/lsof", "/usr/sbin/lsof"],
        linux_package_hint(),
    ));
    checks.push(linux_open_file_limit_check());
    checks.push(linux_proc_limit_check(
        "inotify-watches",
        "/proc/sys/fs/inotify/max_user_watches",
        8_192,
        "raise fs.inotify.max_user_watches if session watch reports ENOSPC",
    ));
    checks.push(linux_proc_limit_check(
        "inotify-instances",
        "/proc/sys/fs/inotify/max_user_instances",
        128,
        "raise fs.inotify.max_user_instances if multiple watchers cannot start",
    ));
    checks.push(Check {
        level: CheckLevel::Ok,
        name: "linux-packages",
        message: linux_package_hint().to_string(),
    });
}

#[cfg(not(target_os = "linux"))]
fn add_platform_checks(_checks: &mut Vec<Check>) {}

#[cfg(target_os = "linux")]
fn systemctl_user_check() -> Check {
    let Some(systemctl) = command_path("systemctl", &["/usr/bin/systemctl", "/bin/systemctl"])
    else {
        return Check {
            level: CheckLevel::Warn,
            name: "systemctl-user",
            message: "systemctl not found; autostart requires systemd user services".to_string(),
        };
    };

    let spec = CommandSpec::new(systemctl.as_os_str())
        .arg("--user")
        .arg("show-environment");
    match run(&spec) {
        Ok(result) if result.code == Some(0) => Check {
            level: CheckLevel::Ok,
            name: "systemctl-user",
            message: "systemctl --user is available".to_string(),
        },
        Ok(result) => Check {
            level: CheckLevel::Warn,
            name: "systemctl-user",
            message: format!(
                "systemctl --user unavailable (exit {:?}); headless autostart may need `loginctl enable-linger $USER`",
                result.code
            ),
        },
        Err(err) => Check {
            level: CheckLevel::Warn,
            name: "systemctl-user",
            message: format!(
                "systemctl --user probe failed: {err}; headless autostart may need `loginctl enable-linger $USER`"
            ),
        },
    }
}

#[cfg(target_os = "linux")]
fn required_linux_tool(name: &'static str, fallbacks: &[&str], hint: &str) -> Check {
    match command_path(name, fallbacks) {
        Some(path) => Check {
            level: CheckLevel::Ok,
            name,
            message: path.display().to_string(),
        },
        None => Check {
            level: CheckLevel::Warn,
            name,
            message: format!("not found; {hint}"),
        },
    }
}

#[cfg(target_os = "linux")]
fn env_check(name: &'static str, var: &str, missing: &str) -> Check {
    if std::env::var_os(var).is_some() {
        Check {
            level: CheckLevel::Ok,
            name,
            message: format!("{var} is set"),
        }
    } else {
        Check {
            level: CheckLevel::Warn,
            name,
            message: missing.to_string(),
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_open_file_limit_check() -> Check {
    match std::fs::read_to_string("/proc/self/limits")
        .ok()
        .and_then(|raw| parse_proc_self_limit_soft(&raw, "Max open files"))
    {
        Some(limit) if limit >= 1_024 => Check {
            level: CheckLevel::Ok,
            name: "open-files-limit",
            message: format!("soft limit {limit}"),
        },
        Some(limit) => Check {
            level: CheckLevel::Warn,
            name: "open-files-limit",
            message: format!(
                "soft limit {limit}; raise ulimit -n if process startup or watches fail"
            ),
        },
        None => Check {
            level: CheckLevel::Warn,
            name: "open-files-limit",
            message: "could not read /proc/self/limits".to_string(),
        },
    }
}

#[cfg(target_os = "linux")]
fn linux_proc_limit_check(
    name: &'static str,
    path: &str,
    warn_below: u64,
    hint: &'static str,
) -> Check {
    match std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| parse_linux_proc_u64(&raw))
    {
        Some(value) if value >= warn_below => Check {
            level: CheckLevel::Ok,
            name,
            message: format!("{path}={value}"),
        },
        Some(value) => Check {
            level: CheckLevel::Warn,
            name,
            message: format!("{path}={value}; {hint}"),
        },
        None => Check {
            level: CheckLevel::Warn,
            name,
            message: format!("could not read {path}; {hint}"),
        },
    }
}

#[cfg(any(test, target_os = "linux"))]
fn parse_linux_proc_u64(raw: &str) -> Option<u64> {
    raw.trim().parse::<u64>().ok()
}

#[cfg(any(test, target_os = "linux"))]
fn parse_proc_self_limit_soft(raw: &str, limit_name: &str) -> Option<u64> {
    raw.lines().find_map(|line| {
        let line = line.trim_start();
        let rest = line.strip_prefix(limit_name)?.trim_start();
        rest.split_whitespace().next()?.parse::<u64>().ok()
    })
}

#[cfg(target_os = "linux")]
fn command_path(name: &str, fallbacks: &[&str]) -> Option<std::path::PathBuf> {
    paths::find_on_path(name).or_else(|| {
        fallbacks
            .iter()
            .map(std::path::PathBuf::from)
            .find(|path| path.is_file())
    })
}

#[cfg(any(test, target_os = "linux"))]
fn linux_package_hint() -> &'static str {
    "Debian/Ubuntu: procps lsof; Fedora: procps-ng lsof; Arch: procps-ng lsof"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_config_is_warning() {
        let check =
            path_exists_or_parent(Path::new("/definitely/not/present/lucarned.yaml"), "config");
        assert_eq!(check.level, CheckLevel::Warn);
        assert_eq!(check.name, "config");
    }

    #[test]
    fn missing_optional_cli_is_warning() {
        let check = optional_cli("definitely-not-a-lucarne-agent-cli");
        assert_eq!(check.level, CheckLevel::Warn);
    }

    #[test]
    fn linux_package_hint_names_supported_distros() {
        let hint = linux_package_hint();

        assert!(hint.contains("Debian/Ubuntu: procps lsof"));
        assert!(hint.contains("Fedora: procps-ng lsof"));
        assert!(hint.contains("Arch: procps-ng lsof"));
    }

    #[test]
    fn parses_linux_proc_u64_limit() {
        assert_eq!(parse_linux_proc_u64("524288\n"), Some(524_288));
        assert_eq!(parse_linux_proc_u64("not-a-number\n"), None);
    }

    #[test]
    fn parses_proc_self_open_file_soft_limit() {
        let raw = "Limit                     Soft Limit           Hard Limit           Units\n\
                   Max open files            1024                 1048576              files\n";

        assert_eq!(
            parse_proc_self_limit_soft(raw, "Max open files"),
            Some(1_024)
        );
    }

    #[cfg(feature = "updates")]
    #[tokio::test]
    async fn disabled_update_check_is_ok() {
        let client = reqwest::Client::new();
        let check = update_check(
            &client,
            crate::updates::UpdateConfig {
                enabled: false,
                ..crate::updates::UpdateConfig::default()
            },
        )
        .await;

        assert_eq!(check.level, CheckLevel::Ok);
        assert_eq!(check.name, "update");
        assert_eq!(check.message, "checks disabled by config");
    }

    #[cfg(feature = "updates")]
    #[tokio::test]
    async fn failed_update_check_is_warning() {
        let client = reqwest::Client::new();
        let check = update_check(
            &client,
            crate::updates::UpdateConfig {
                repository: "not-a-repository".to_string(),
                ..crate::updates::UpdateConfig::default()
            },
        )
        .await;

        assert_eq!(check.level, CheckLevel::Warn);
        assert_eq!(check.name, "update");
        assert!(check.message.contains("check failed:"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_env_check_warns_with_linger_hint() {
        let check = env_check(
            "xdg-runtime-dir",
            "DEFINITELY_NOT_A_LUCARNE_ENV_VAR",
            "missing; systemd user sessions and headless autostart may need `loginctl enable-linger $USER`",
        );

        assert_eq!(check.level, CheckLevel::Warn);
        assert!(check.message.contains("loginctl enable-linger $USER"));
    }
}
