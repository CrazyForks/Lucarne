use tokio::process::Command;
use tracing::trace;

use super::ProcessSample;

pub(super) async fn snapshot() -> Result<Vec<ProcessSample>, String> {
    trace!(target: "lucarne::host::process_table", "sampling unix process table");
    let output = Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,pgid=,rss=,%cpu="])
        .output()
        .await
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        trace!(target: "lucarne::host::process_table", status = ?output.status, stderr = %stderr.trim(), "unix process table snapshot failed");
        return Err(stderr.trim().to_string());
    }
    let samples = parse_process_table(&output.stdout);
    trace!(target: "lucarne::host::process_table", count = samples.len(), "sampled unix process table");
    Ok(samples)
}

fn parse_process_table(stdout: &[u8]) -> Vec<ProcessSample> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(parse_process_sample)
        .collect()
}

fn parse_process_sample(line: &str) -> Option<ProcessSample> {
    let mut parts = line.split_whitespace();
    let pid = parts.next()?.parse().ok()?;
    let parent_pid = parts.next()?.parse().ok()?;
    let group_id = parts.next()?.parse().ok()?;
    let rss_kib = parts.next()?.parse::<u64>().ok()?;
    let cpu_percent = parts.next()?.parse().ok()?;
    Some(ProcessSample {
        pid,
        parent_pid: Some(parent_pid),
        group_id: Some(group_id),
        rss_bytes: rss_kib.saturating_mul(1024),
        cpu_percent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_process_sample_reads_ps_columns() {
        let sample = parse_process_sample(" 123  45  123  64  3.5").expect("sample");

        assert_eq!(sample.pid, 123);
        assert_eq!(sample.parent_pid, Some(45));
        assert_eq!(sample.group_id, Some(123));
        assert_eq!(sample.rss_bytes, 64 * 1024);
        assert_eq!(sample.cpu_percent, 3.5);
    }
}
