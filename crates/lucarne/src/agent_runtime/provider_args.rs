use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{
    adapter::{ArgProfile, SessionParams},
    dialect::PermissionMode,
    event::ResumeHandle,
    ProviderId,
};

use super::{
    AgentError, AgentErrorKind, AgentInput, AgentSessionOptions, OpenSession, ResumeSession,
    SessionId,
};

pub(crate) fn decode_open(
    provider_id: ProviderId,
    arg_profile: &ArgProfile,
    req: OpenSession,
) -> Result<(SessionParams, Option<AgentInput>, AgentSessionOptions), AgentError> {
    let session_options = AgentSessionOptions::from_idle_timeout_ms(req.idle_timeout_ms);
    let mut start = SessionParams {
        model: req.model.map(|model| model.to_string()).unwrap_or_default(),
        cwd: req.cwd.map(|cwd| cwd.to_string()).unwrap_or_default(),
        first_prompt: String::new(),
        ..Default::default()
    };

    let args = decode_args(provider_id, "open", arg_profile, req.args, false)?;
    start.system_prompt = args.system_prompt;
    start.extra_env = args.extra_env;
    start.extra_args = args.extra_args;
    start.permission_mode = args.permission_mode.try_into()?;

    Ok((start, req.initial_input, session_options))
}

pub(crate) fn decode_resume(
    provider_id: ProviderId,
    arg_profile: &ArgProfile,
    req: ResumeSession,
) -> Result<(SessionParams, AgentSessionOptions, Option<SessionId>), AgentError> {
    let session_options = AgentSessionOptions::from_idle_timeout_ms(req.idle_timeout_ms);
    if arg_profile.resume_session_key.is_empty() {
        return Err(unsupported(format!(
            "{} resume arg profile must declare resume_session_key",
            provider_id
        )));
    }
    let args = decode_args(provider_id, "resume", arg_profile, req.args, true)?;
    let known_session_id = (arg_profile.resume_session_id_hint && !req.session_ref.0.is_empty())
        .then(|| SessionId(req.session_ref.0.clone()));
    Ok((
        SessionParams {
            cwd: args.cwd.clone().unwrap_or_default(),
            system_prompt: args.system_prompt,
            first_prompt: String::new(),
            resume: Some(ResumeHandle {
                version: 1,
                data: resume_data(
                    &arg_profile.resume_session_key,
                    &req.session_ref.0,
                    args.cwd.as_deref(),
                ),
            }),
            extra_env: args.extra_env,
            extra_args: args.extra_args,
            permission_mode: args.permission_mode.try_into()?,
            ..Default::default()
        },
        session_options,
        known_session_id,
    ))
}

fn decode_args(
    provider_id: ProviderId,
    operation: &str,
    arg_profile: &ArgProfile,
    value: serde_json::Value,
    resume: bool,
) -> Result<ProviderArgs, AgentError> {
    if value.is_null() {
        return Ok(ProviderArgs::default());
    }
    let serde_json::Value::Object(map) = value else {
        return Err(unsupported(format!(
            "{} {} args must be a JSON object or null",
            provider_id, operation
        )));
    };
    for key in map.keys() {
        if !arg_field_allowed(key, arg_profile, resume) {
            return Err(unsupported(format!(
                "{} {} args contain unsupported field {:?}",
                provider_id, operation, key
            )));
        }
    }
    serde_json::from_value(serde_json::Value::Object(map)).map_err(|err| {
        unsupported(format!(
            "{} {} args are invalid: {}",
            provider_id, operation, err
        ))
    })
}

fn arg_field_allowed(key: &str, arg_profile: &ArgProfile, resume: bool) -> bool {
    matches!(key, "extra_env" | "extra_args" | "permission_mode")
        || (resume && key == "cwd")
        || (arg_profile.system_prompt && key == "system_prompt")
}

fn resume_data(
    session_key: &str,
    session_ref: &str,
    cwd: Option<&str>,
) -> BTreeMap<String, serde_json::Value> {
    let mut data = BTreeMap::new();
    data.insert(
        session_key.into(),
        serde_json::Value::String(session_ref.to_string()),
    );
    if let Some(cwd) = cwd.filter(|cwd| !cwd.is_empty()) {
        data.insert("cwd".into(), serde_json::Value::String(cwd.to_string()));
    }
    data
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ProviderArgs {
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    system_prompt: String,
    #[serde(default)]
    extra_env: BTreeMap<String, String>,
    #[serde(default)]
    extra_args: Vec<String>,
    #[serde(default)]
    permission_mode: PermissionModeArg,
}

#[derive(Debug, Deserialize, Default)]
#[serde(untagged)]
enum PermissionModeArg {
    #[default]
    Missing,
    Named(String),
}

impl TryFrom<PermissionModeArg> for PermissionMode {
    type Error = AgentError;

    fn try_from(value: PermissionModeArg) -> Result<Self, Self::Error> {
        match value {
            PermissionModeArg::Missing => Ok(PermissionMode::Default),
            PermissionModeArg::Named(name) => PermissionMode::parse(&name).ok_or_else(|| {
                unsupported(format!(
                    "unsupported permission_mode {:?}; expected one of {}",
                    name,
                    PermissionMode::accepted_values().join(", ")
                ))
            }),
        }
    }
}

fn unsupported(message: impl Into<String>) -> AgentError {
    AgentError {
        kind: AgentErrorKind::Unsupported,
        message: message.into().into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decode_open_defaults_permission_mode_to_default() {
        let (start, initial_input, session_options) = decode_open(
            ProviderId::from_static("codex"),
            &ArgProfile::default(),
            OpenSession {
                args: json!({}),
                ..Default::default()
            },
        )
        .expect("decode open");

        assert_eq!(start.permission_mode, PermissionMode::Default);
        assert!(initial_input.is_none());
        assert_eq!(
            session_options,
            // DEFAULT_IDLE_TIMEOUT in session.rs = 2h
            AgentSessionOptions::with_idle_timeout(Some(std::time::Duration::from_secs(
                2 * 60 * 60
            )))
        );
    }

    #[test]
    fn decode_open_accepts_canonical_permission_modes() {
        for (raw, expected) in [
            ("default", PermissionMode::Default),
            ("write", PermissionMode::Write),
            ("read_only", PermissionMode::ReadOnly),
            ("auto", PermissionMode::Auto),
            ("full", PermissionMode::Full),
            ("bypass", PermissionMode::Bypass),
        ] {
            let (start, _, _) = decode_open(
                ProviderId::from_static("gemini"),
                &ArgProfile::default(),
                OpenSession {
                    args: json!({ "permission_mode": raw }),
                    ..Default::default()
                },
            )
            .unwrap_or_else(|err| panic!("decode open for {raw}: {err}"));
            assert_eq!(start.permission_mode, expected, "raw={raw}");
        }
    }

    #[test]
    fn decode_open_rejects_provider_specific_permission_mode_names() {
        for raw in [
            "acceptEdits",
            "bypassPermissions",
            "dontAsk",
            "auto_edit",
            "yolo",
            "plan",
            "untrusted",
            "on-request",
        ] {
            let err = decode_open(
                ProviderId::from_static("claude"),
                &ArgProfile::default(),
                OpenSession {
                    args: json!({ "permission_mode": raw }),
                    ..Default::default()
                },
            )
            .expect_err("provider-specific mode should fail");
            assert_eq!(err.kind, AgentErrorKind::Unsupported, "raw={raw}");
        }
    }
}
