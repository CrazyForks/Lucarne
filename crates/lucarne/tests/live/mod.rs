use base64::Engine;

pub mod common;
pub mod providers;
pub mod recording;
pub mod runtime;

pub use common::*;
pub use providers::*;
pub use recording::*;

#[allow(dead_code)]
pub fn expected_recorded_write_contents(
    provider_name: &str,
    suite: &'static str,
    case_id: &'static str,
    rel_path: &str,
    live_default: &'static str,
) -> String {
    let live_rerecord = std::env::var("LUCARNE_LIVE_E2E").unwrap_or_default() == "1"
        && std::env::var("LUCARNE_LIVE_RERECORD").unwrap_or_default() == "1";
    if live_rerecord {
        return live_default.to_string();
    }

    let effects_path = repo_root()
        .join("tests")
        .join("data")
        .join("live_recordings")
        .join(suite)
        .join(provider_name)
        .join(case_id)
        .join("effects.json");
    let Ok(raw) = std::fs::read_to_string(&effects_path) else {
        return live_default.to_string();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return live_default.to_string();
    };
    let Some(encoded) = json
        .get("writes")
        .and_then(serde_json::Value::as_array)
        .and_then(|writes| {
            writes.iter().find_map(|write| {
                let path = write.get("path").and_then(serde_json::Value::as_str)?;
                if path == rel_path {
                    write
                        .get("contents_base64")
                        .and_then(serde_json::Value::as_str)
                } else {
                    None
                }
            })
        })
    else {
        return live_default.to_string();
    };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
        return live_default.to_string();
    };
    String::from_utf8(bytes).unwrap_or_else(|_| live_default.to_string())
}

#[allow(dead_code)]
pub fn expected_live_tool_contents(
    provider_name: &str,
    suite: &'static str,
    case_id: &'static str,
    rel_path: &str,
) -> String {
    expected_recorded_write_contents(
        provider_name,
        suite,
        case_id,
        rel_path,
        "lucarne-live-tool\n",
    )
}
