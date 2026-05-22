use super::UpdateStatus;

const CLI_CHANGE_LIMIT: usize = 4_000;
const NOTIFICATION_CHANGE_LIMIT: usize = 1_800;

pub const INSTALL_HINT: &str = "macOS/Linux:\n  curl -fsSL https://github.com/tuchg/Lucarne/releases/latest/download/lucarned-installer.sh | sh\n\nWindows PowerShell:\n  irm https://github.com/tuchg/Lucarne/releases/latest/download/lucarned-installer.ps1 | iex\n\nIf installed through a package manager, upgrade with that package manager.";

pub fn truncate_release_body(body: &str, max_chars: usize) -> String {
    if body.chars().count() <= max_chars {
        return body.to_string();
    }

    let mut truncated: String = body.chars().take(max_chars).collect();
    truncated.push_str("...(truncated)");
    truncated
}

pub fn render_update_cli(status: &UpdateStatus) -> String {
    let mut output = String::new();
    output.push_str("Lucarne update status\n");
    output.push_str(&format!("Current version: {}\n", status.current_version));

    if !status.automatic_checks_enabled {
        output.push_str("Automatic update checks: disabled by config\n");
    }

    let Some(latest_version) = &status.latest_version else {
        output.push_str("Latest version: unavailable\nNo stable GitHub release found.\n");
        return output;
    };

    output.push_str(&format!("Latest version: {latest_version}\n"));
    if status.is_newer {
        output.push_str("Status: update available\n");
    } else {
        output.push_str("Status: lucarned is up to date (current)\n");
    }

    if let Some(name) = non_empty(status.release_name.as_deref()) {
        output.push_str(&format!("Release: {name}\n"));
    }
    if let Some(url) = non_empty(status.release_url.as_deref()) {
        output.push_str(&format!("URL: {url}\n"));
    }
    if let Some(published_at) = non_empty(status.published_at.as_deref()) {
        output.push_str(&format!("Published: {published_at}\n"));
    }
    if let Some(body) = non_empty(status.release_body.as_deref()) {
        output.push_str("\nChanges:\n");
        output.push_str(&truncate_release_body(body, CLI_CHANGE_LIMIT));
        output.push('\n');
    }
    if status.is_newer {
        output.push_str("\nInstall/upgrade:\n");
        output.push_str(INSTALL_HINT);
        output.push('\n');
    }
    output
}

pub fn render_update_notification(status: &UpdateStatus) -> String {
    if !status.is_newer {
        return render_update_cli(status);
    }

    let mut output = String::new();
    output.push_str("Lucarne update available\n");
    output.push_str(&format!("Current: {}\n", status.current_version));
    if let Some(latest_version) = &status.latest_version {
        output.push_str(&format!("Latest: {latest_version}\n"));
    }
    if let Some(name) = non_empty(status.release_name.as_deref()) {
        output.push_str(&format!("Release: {name}\n"));
    }
    if let Some(url) = non_empty(status.release_url.as_deref()) {
        output.push_str(&format!("URL: {url}\n"));
    }
    if let Some(published_at) = non_empty(status.published_at.as_deref()) {
        output.push_str(&format!("Published: {published_at}\n"));
    }
    if let Some(body) = non_empty(status.release_body.as_deref()) {
        output.push_str("\nChanges:\n");
        output.push_str(&truncate_release_body(body, NOTIFICATION_CHANGE_LIMIT));
        output.push('\n');
    }
    output.push_str("\nInstall/upgrade:\n");
    output.push_str(INSTALL_HINT);
    output
}

pub fn render_doctor_message(status: &UpdateStatus) -> (crate::doctor::CheckLevel, String) {
    if !status.automatic_checks_enabled {
        return (
            crate::doctor::CheckLevel::Ok,
            "update checks disabled by config".to_string(),
        );
    }

    if status.is_newer {
        let latest = status.latest_version.as_deref().unwrap_or("unknown");
        let url = status
            .release_url
            .as_deref()
            .unwrap_or("release URL unavailable");
        return (
            crate::doctor::CheckLevel::Warn,
            format!("update {latest} available: {url}"),
        );
    }

    if let Some(latest) = &status.latest_version {
        return (
            crate::doctor::CheckLevel::Ok,
            format!(
                "current version {} (latest {latest})",
                status.current_version
            ),
        );
    }

    (
        crate::doctor::CheckLevel::Warn,
        "no stable GitHub release found".to_string(),
    )
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(value)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn newer_status() -> UpdateStatus {
        UpdateStatus {
            current_version: "0.1.0".to_string(),
            latest_version: Some("0.2.0".to_string()),
            release_name: Some("Lucarne 0.2.0".to_string()),
            release_url: Some("https://github.com/tuchg/Lucarne/releases/tag/v0.2.0".to_string()),
            published_at: Some("2026-05-21T00:00:00Z".to_string()),
            release_body: Some("Fixed bugs and added features".to_string()),
            is_newer: true,
            automatic_checks_enabled: true,
        }
    }

    #[test]
    fn truncation_appends_marker_when_over_limit() {
        assert_eq!(truncate_release_body("abcdef", 3), "abc...(truncated)");
    }

    #[test]
    fn cli_output_includes_update_details_and_install_hint() {
        let output = render_update_cli(&newer_status());
        assert!(output.contains("0.1.0"));
        assert!(output.contains("0.2.0"));
        assert!(output.contains("Lucarne 0.2.0"));
        assert!(output.contains("https://github.com/tuchg/Lucarne/releases/tag/v0.2.0"));
        assert!(output.contains("curl -fsSL"));
        assert!(output.contains("PowerShell"));
    }

    #[test]
    fn cli_output_for_current_status_says_current() {
        let mut status = newer_status();
        status.current_version = "0.2.0".to_string();
        status.is_newer = false;
        let output = render_update_cli(&status);
        assert!(output.contains("up to date") || output.contains("current"));
    }

    #[test]
    fn notification_output_includes_update_details_and_truncated_changes() {
        let mut status = newer_status();
        status.release_body = Some("x".repeat(2_000));
        let output = render_update_notification(&status);
        assert!(output.contains("0.1.0"));
        assert!(output.contains("0.2.0"));
        assert!(output.contains("Lucarne 0.2.0"));
        assert!(output.contains("...(truncated)"));
    }
}
