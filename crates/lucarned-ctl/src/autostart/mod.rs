use std::path::PathBuf;

use super::process::{run, CommandResult, CommandSpec};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(windows)]
use windows as platform;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutostartPaths {
    pub lucarned: PathBuf,
    pub config_dir: PathBuf,
    pub log_dir: PathBuf,
}

pub fn install(paths: &AutostartPaths, start: bool) -> Result<(), String> {
    platform::install(paths)?;
    if start {
        start_service()?;
    }
    Ok(())
}

pub fn uninstall(stop: bool) -> Result<(), String> {
    if stop {
        let _ = stop_service();
    }
    platform::uninstall()
}

pub fn start_service() -> Result<(), String> {
    run_checked(platform::start_command()?)
}

pub fn stop_service() -> Result<(), String> {
    run_checked(platform::stop_command()?)
}

pub fn status() -> Result<(), String> {
    let spec = platform::status_command()?;
    let result = run(&spec).map_err(|err| format!("{}: {err}", spec.program.to_string_lossy()))?;
    print!("{}", result.stdout);
    eprint!("{}", result.stderr);
    if result.code == Some(0) {
        Ok(())
    } else {
        Err(format!(
            "autostart status failed with code {:?}",
            result.code
        ))
    }
}

pub fn status_summary() -> (bool, String) {
    let spec = match platform::status_command() {
        Ok(spec) => spec,
        Err(err) => return (false, err),
    };
    match run(&spec) {
        Ok(result) if result.code == Some(0) => (
            true,
            first_non_empty_line(&result.stdout).unwrap_or_else(|| "installed".to_string()),
        ),
        Ok(result) => {
            let message = first_non_empty_line(&result.stderr)
                .or_else(|| first_non_empty_line(&result.stdout))
                .unwrap_or_else(|| format!("status exited with {:?}", result.code));
            (false, message)
        }
        Err(err) => (false, err.to_string()),
    }
}

fn first_non_empty_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned)
}

fn run_checked(spec: CommandSpec) -> Result<(), String> {
    let result = run(&spec).map_err(|err| format!("{}: {err}", spec.program.to_string_lossy()))?;
    if result.code == Some(0) {
        Ok(())
    } else {
        Err(format_command_failure(&spec, &result))
    }
}

fn format_command_failure(spec: &CommandSpec, result: &CommandResult) -> String {
    let stderr = result.stderr.trim();
    if stderr.is_empty() {
        format!(
            "{} failed with code {:?}",
            spec.program.to_string_lossy(),
            result.code
        )
    } else {
        format!(
            "{} failed with code {:?}: {}",
            spec.program.to_string_lossy(),
            result.code,
            stderr
        )
    }
}
