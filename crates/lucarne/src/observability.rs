pub fn emit_memory_profile_snapshot(label: &str) {
    static PAUSE_MS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

    let ts_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    eprintln!(
        "LUCARNE_MEMORY_SNAPSHOT pid={} label={} ts_ms={}",
        std::process::id(),
        label,
        ts_ms
    );

    let pause_ms = *PAUSE_MS.get_or_init(|| {
        std::env::var("LUCARNE_MEMORY_PROFILE_PAUSE_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0)
    });
    if pause_ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(pause_ms));
    }
}
