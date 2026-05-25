#![cfg(target_os = "linux")]

use std::{fs, path::PathBuf};

use super::AutostartPaths;
use crate::process::CommandSpec;

const UNIT_NAME: &str = "lucarned.service";

pub fn install(paths: &AutostartPaths) -> Result<(), String> {
    fs::create_dir_all(paths.log_dir.as_path()).map_err(|err| format!("create log dir: {err}"))?;
    let unit = unit_path()?;
    if let Some(parent) = unit.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create systemd user dir: {err}"))?;
    }
    fs::write(&unit, render_unit(paths))
        .map_err(|err| format!("write {}: {err}", unit.display()))?;
    super::run_checked(systemctl(&["daemon-reload"]))?;
    super::run_checked(systemctl(&["enable", UNIT_NAME]))
}

pub fn uninstall() -> Result<(), String> {
    let _ = super::run_checked(systemctl(&["disable", UNIT_NAME]));
    let unit = unit_path()?;
    if unit.exists() {
        fs::remove_file(&unit).map_err(|err| format!("remove {}: {err}", unit.display()))?;
    }
    let _ = super::run_checked(systemctl(&["daemon-reload"]));
    Ok(())
}

pub fn start_command() -> Result<CommandSpec, String> {
    Ok(systemctl(&["start", UNIT_NAME]))
}

pub fn stop_command() -> Result<CommandSpec, String> {
    Ok(systemctl(&["stop", UNIT_NAME]))
}

pub fn status_command() -> Result<CommandSpec, String> {
    Ok(systemctl(&["status", UNIT_NAME, "--no-pager"]))
}

fn systemctl(args: &[&str]) -> CommandSpec {
    let mut spec = CommandSpec::new("systemctl").arg("--user");
    for arg in args {
        spec = spec.arg(*arg);
    }
    spec
}

fn unit_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
    Ok(PathBuf::from(home).join(".config/systemd/user/lucarned.service"))
}

fn render_unit(paths: &AutostartPaths) -> String {
    render_unit_with_path(paths, &super::service_path_env())
}

fn render_unit_with_path(paths: &AutostartPaths, path_env: &str) -> String {
    format!(
        "[Unit]\nDescription=Lucarne daemon\n\n[Service]\nType=simple\nExecStart={}\nEnvironment=\"PATH={}\"\nRestart=on-failure\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n",
        systemd_quote(&paths.lucarned.display().to_string()),
        systemd_env_escape(path_env),
    )
}

fn systemd_quote(input: &str) -> String {
    if input
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"/._-".contains(&byte))
    {
        input.to_string()
    } else {
        format!("\"{}\"", input.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

fn systemd_env_escape(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_quotes_exec_start_with_spaces() {
        let paths = AutostartPaths {
            lucarned: PathBuf::from("/home/me/My Apps/lucarned"),
            config_dir: PathBuf::from("/home/me/.lucarned"),
            log_dir: PathBuf::from("/home/me/.lucarned/logs"),
        };
        let unit = render_unit(&paths);
        assert!(unit.contains("ExecStart=\"/home/me/My Apps/lucarned\""));
        assert!(unit.contains("Environment=\"PATH="));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("RestartSec=5"));
        assert!(unit.contains("WantedBy=default.target"));
    }

    #[test]
    fn unit_persists_current_process_path_only() {
        let paths = AutostartPaths {
            lucarned: PathBuf::from("/home/me/lucarned"),
            config_dir: PathBuf::from("/home/me/.lucarned"),
            log_dir: PathBuf::from("/home/me/.lucarned/logs"),
        };
        let unit = render_unit_with_path(&paths, "/custom/bin:/usr/bin");
        assert!(unit.contains("Environment=\"PATH=/custom/bin:/usr/bin\""));
        assert!(!unit.contains("HOMEBREW_PATH"));
        assert!(!unit.contains("LUCARNE_TEST_ENV"));
    }
}
