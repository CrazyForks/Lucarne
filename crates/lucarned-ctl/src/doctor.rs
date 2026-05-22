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
    for check in &checks {
        let prefix = match check.level {
            CheckLevel::Ok => "ok",
            CheckLevel::Warn => "warn",
            CheckLevel::Fail => "fail",
        };
        println!("{prefix}: {}: {}", check.name, check.message);
    }
    if checks.iter().any(|check| check.level == CheckLevel::Fail) {
        Err("doctor found critical failures".to_string())
    } else {
        Ok(())
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
    for agent in ["codex", "claude", "pi", "gemini", "copilot"] {
        checks.push(optional_cli(agent));
    }
    Ok(checks)
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
}
