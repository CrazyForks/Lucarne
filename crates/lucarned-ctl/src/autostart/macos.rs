#![cfg(target_os = "macos")]

use std::{fs, path::PathBuf, process::Command};

use super::AutostartPaths;
use crate::process::CommandSpec;

const LABEL: &str = "com.tuchg.lucarned";

pub fn install(paths: &AutostartPaths) -> Result<(), String> {
    fs::create_dir_all(paths.log_dir.as_path()).map_err(|err| format!("create log dir: {err}"))?;
    let plist = launch_agent_path()?;
    if let Some(parent) = plist.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create launch agent dir: {err}"))?;
    }
    fs::write(&plist, render_plist(paths))
        .map_err(|err| format!("write {}: {err}", plist.display()))?;
    let _ = super::run_checked(bootout_command()?);
    super::run_checked(bootstrap_command(plist)?)
}

pub fn uninstall() -> Result<(), String> {
    let _ = super::run_checked(bootout_command()?);
    let plist = launch_agent_path()?;
    if plist.exists() {
        fs::remove_file(&plist).map_err(|err| format!("remove {}: {err}", plist.display()))?;
    }
    Ok(())
}

pub fn start_command() -> Result<CommandSpec, String> {
    Ok(CommandSpec::new("launchctl")
        .arg("kickstart")
        .arg("-k")
        .arg(format!("gui/{}/{}", uid()?, LABEL)))
}

pub fn stop_command() -> Result<CommandSpec, String> {
    bootout_command()
}

pub fn status_command() -> Result<CommandSpec, String> {
    Ok(CommandSpec::new("launchctl")
        .arg("print")
        .arg(format!("gui/{}/{}", uid()?, LABEL)))
}

fn bootstrap_command(plist: PathBuf) -> Result<CommandSpec, String> {
    Ok(CommandSpec::new("launchctl")
        .arg("bootstrap")
        .arg(format!("gui/{}", uid()?))
        .arg(plist.into_os_string()))
}

fn bootout_command() -> Result<CommandSpec, String> {
    Ok(CommandSpec::new("launchctl")
        .arg("bootout")
        .arg(format!("gui/{}/{}", uid()?, LABEL)))
}

fn launch_agent_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
    Ok(PathBuf::from(home).join("Library/LaunchAgents/com.tuchg.lucarned.plist"))
}

fn uid() -> Result<String, String> {
    if let Ok(uid) = std::env::var("UID") {
        if !uid.is_empty() {
            return Ok(uid);
        }
    }
    let output = Command::new("id")
        .arg("-u")
        .output()
        .map_err(|err| format!("id -u: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "id -u failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn render_plist(paths: &AutostartPaths) -> String {
    render_plist_with_path(paths, &super::service_path_env())
}

fn render_plist_with_path(paths: &AutostartPaths, path_env: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
  <key>Label</key><string>{}</string>\n\
  <key>ProgramArguments</key><array><string>{}</string></array>\n\
  <key>EnvironmentVariables</key>\n\
  <dict>\n\
    <key>PATH</key><string>{}</string>\n\
  </dict>\n\
  <key>RunAtLoad</key><true/>\n\
  <key>KeepAlive</key><false/>\n\
  <key>StandardOutPath</key><string>{}</string>\n\
  <key>StandardErrorPath</key><string>{}</string>\n\
</dict>\n\
</plist>\n",
        LABEL,
        xml_escape(&paths.lucarned.display().to_string()),
        xml_escape(path_env),
        xml_escape(&paths.log_dir.join("launchd.out.log").display().to_string()),
        xml_escape(&paths.log_dir.join("launchd.err.log").display().to_string()),
    )
}

fn xml_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_escapes_paths() {
        let paths = AutostartPaths {
            lucarned: PathBuf::from("/tmp/A&B/lucarned"),
            config_dir: PathBuf::from("/tmp/config"),
            log_dir: PathBuf::from("/tmp/logs"),
        };
        let plist = render_plist(&paths);
        assert!(plist.contains("/tmp/A&amp;B/lucarned"));
        assert!(plist.contains("com.tuchg.lucarned"));
    }

    #[test]
    fn plist_sets_cli_lookup_path() {
        let paths = AutostartPaths {
            lucarned: PathBuf::from("/tmp/lucarned"),
            config_dir: PathBuf::from("/tmp/config"),
            log_dir: PathBuf::from("/tmp/logs"),
        };
        let plist = render_plist(&paths);
        assert!(plist.contains("<key>EnvironmentVariables</key>"));
        assert!(plist.contains("<key>PATH</key>"));
        assert!(plist.contains("/usr/bin"));
    }

    #[test]
    fn plist_persists_current_process_path_only() {
        let paths = AutostartPaths {
            lucarned: PathBuf::from("/tmp/lucarned"),
            config_dir: PathBuf::from("/tmp/config"),
            log_dir: PathBuf::from("/tmp/logs"),
        };
        let plist = render_plist_with_path(&paths, "/custom/bin:/usr/bin");
        assert!(plist.contains("<key>PATH</key><string>/custom/bin:/usr/bin</string>"));
        assert!(!plist.contains("HOMEBREW_PATH"));
        assert!(!plist.contains("LUCARNE_TEST_ENV"));
    }
}
