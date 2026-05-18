use chrono::{Local, TimeZone};

pub(crate) fn format_last_active_display(unix: i64) -> String {
    Local
        .timestamp_opt(unix, 0)
        .single()
        .map(|datetime| datetime.format("%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "unknown".into())
}
