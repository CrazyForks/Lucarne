//! Usage / logs / resume-handle sub-domain: the payload types attached
//! to `Log` and `UsageDelta` events, and the opaque adapter-owned
//! `ResumeHandle` used to pick a session back up.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LogLine {
    pub level: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stream: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default, skip_serializing_if = "super::is_zero_i64")]
    pub input_tokens: i64,
    #[serde(default, skip_serializing_if = "super::is_zero_i64")]
    pub output_tokens: i64,
    #[serde(default, skip_serializing_if = "super::is_zero_i64")]
    pub cache_read_tokens: i64,
    #[serde(default, skip_serializing_if = "super::is_zero_i64")]
    pub cache_write_tokens: i64,
    #[serde(default, skip_serializing_if = "super::is_zero_f64")]
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageDelta {
    pub delta: Usage,
}

/// `ResumeHandle` is an opaque serialization of "where to pick up next
/// time". The owning provider is tracked by the surrounding workspace or
/// runtime event, not by this handle.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResumeHandle {
    pub version: i32,
    #[serde(default)]
    pub data: BTreeMap<String, serde_json::Value>,
}
