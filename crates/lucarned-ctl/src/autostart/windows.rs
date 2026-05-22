#![cfg(windows)]

use super::AutostartPaths;
use crate::process::CommandSpec;

const TASK_NAME: &str = "LucarneLucarned";

pub fn install(paths: &AutostartPaths) -> Result<(), String> {
    std::fs::create_dir_all(paths.log_dir.as_path())
        .map_err(|err| format!("create log dir: {err}"))?;
    super::run_checked(create_command(&paths.lucarned)?)
}

pub fn uninstall() -> Result<(), String> {
    super::run_checked(delete_command())
}

pub fn start_command() -> Result<CommandSpec, String> {
    Ok(CommandSpec::new("schtasks")
        .arg("/Run")
        .arg("/TN")
        .arg(TASK_NAME))
}

pub fn stop_command() -> Result<CommandSpec, String> {
    Ok(CommandSpec::new("schtasks")
        .arg("/End")
        .arg("/TN")
        .arg(TASK_NAME))
}

pub fn status_command() -> Result<CommandSpec, String> {
    Ok(CommandSpec::new("schtasks")
        .arg("/Query")
        .arg("/TN")
        .arg(TASK_NAME)
        .arg("/FO")
        .arg("LIST"))
}

fn create_command(lucarned: &std::path::Path) -> Result<CommandSpec, String> {
    Ok(CommandSpec::new("schtasks")
        .arg("/Create")
        .arg("/SC")
        .arg("ONLOGON")
        .arg("/TN")
        .arg(TASK_NAME)
        .arg("/TR")
        .arg(task_action(lucarned)?)
        .arg("/RL")
        .arg("LIMITED")
        .arg("/F"))
}

fn delete_command() -> CommandSpec {
    CommandSpec::new("schtasks")
        .arg("/Delete")
        .arg("/TN")
        .arg(TASK_NAME)
        .arg("/F")
}

fn task_action(path: &std::path::Path) -> Result<String, String> {
    let raw = path.display().to_string();
    if raw.contains('"') {
        return Err("lucarned path contains unsupported quote character".to_string());
    }
    Ok(format!("\"{raw}\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_action_quotes_path_with_spaces() {
        assert_eq!(
            task_action(std::path::Path::new(r"C:\Users\A B\lucarned.exe")).unwrap(),
            r#""C:\Users\A B\lucarned.exe""#
        );
    }

    #[test]
    fn create_command_uses_current_user_logon_task() {
        let spec = create_command(std::path::Path::new(r"C:\Lucarne\lucarned.exe")).unwrap();
        let args = spec
            .args
            .iter()
            .map(|value| value.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(args.contains(&"ONLOGON".to_string()));
        assert!(args.contains(&"LIMITED".to_string()));
        assert!(args.contains(&"LucarneLucarned".to_string()));
    }
}
