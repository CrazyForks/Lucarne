use std::{path::Path, process::Command};
use tracing::trace;

pub(crate) fn observed_session_writer_pid(path: &Path) -> Option<i32> {
    let output = match Command::new("/usr/sbin/lsof")
        .args(["-t", "--"])
        .arg(path)
        .output()
    {
        Ok(output) => output,
        Err(err) => {
            trace!(target: "lucarne::host::file_users", path = %path.display(), error = %err, "lsof writer lookup failed");
            return None;
        }
    };
    if !output.status.success() {
        trace!(target: "lucarne::host::file_users", path = %path.display(), status = ?output.status, "lsof writer lookup returned no process");
        return None;
    }
    parse_lsof_pid_output(&output.stdout)
}

fn parse_lsof_pid_output(stdout: &[u8]) -> Option<i32> {
    let current_pid = std::process::id() as i32;
    String::from_utf8_lossy(stdout).lines().find_map(|line| {
        let pid = line.trim().parse::<i32>().ok()?;
        (pid > 0 && pid != current_pid).then_some(pid)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lsof_pid_output_skips_current_and_non_positive_pids() {
        let current_pid = std::process::id();
        let output = format!("0\n-4\n{}\n12345\n", current_pid);

        assert_eq!(parse_lsof_pid_output(output.as_bytes()), Some(12345));
    }

    #[test]
    fn parse_lsof_pid_output_returns_none_without_other_positive_pid() {
        let current_pid = std::process::id();
        let output = format!("0\n-4\n{}\nnot-a-pid\n", current_pid);

        assert_eq!(parse_lsof_pid_output(output.as_bytes()), None);
    }
}
