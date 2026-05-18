use lucarne::agent_runtime::{
    AgentErrorKind, ApprovalRequest, CallId, CommandId, CommandRejectedEvent, InstanceId,
    InterventionRequest, OpenSession, Question, QuestionRequest, ResumeSession, RuntimeBusEvent,
    RuntimeBusFilter, RuntimeBusOutput, RuntimeCommand, SessionId, SessionOpenedEvent, SessionRef,
};
use lucarne::error::LucarneError;
use lucarne::ProviderId;
use serde_json::{from_str, to_string, to_value, Value};
use smol_str::SmolStr;

#[test]
fn agent_runtime_types_identifier_wrappers_clone_and_eq() {
    let session_id = SessionId(SmolStr::new("session-1"));
    let session_ref = SessionRef(SmolStr::new("session-ref-1"));
    let call_id = CallId(SmolStr::new("call-1"));
    let instance_id = InstanceId(SmolStr::new("instance-1"));
    let command_id = CommandId(SmolStr::new("command-1"));

    assert_eq!(session_id, session_id.clone());
    assert_eq!(session_ref, session_ref.clone());
    assert_eq!(call_id, call_id.clone());
    assert_eq!(instance_id, instance_id.clone());
    assert_eq!(command_id, command_id.clone());
}

#[test]
fn agent_runtime_types_runtime_bus_filter_round_trips_through_serde() {
    let filter = RuntimeBusFilter {
        session_lifecycle: true,
        user_messages: true,
        assistant_messages: false,
        reasoning: true,
        tool_calls: false,
        tool_results: true,
        usage: false,
        intervention_requests: true,
        turn_lifecycle: true,
    };

    let json = to_string(&filter).unwrap();
    let round_tripped: RuntimeBusFilter = from_str(&json).unwrap();

    assert_eq!(round_tripped, filter);
}

#[test]
fn agent_runtime_types_runtime_bus_filter_defaults_lifecycle_on() {
    let filter = RuntimeBusFilter::default();
    assert!(filter.session_lifecycle);
    assert!(!filter.user_messages);
    assert!(!filter.assistant_messages);
    assert!(!filter.reasoning);
    assert!(!filter.tool_calls);
    assert!(!filter.tool_results);
    assert!(!filter.usage);
    assert!(!filter.intervention_requests);
}

#[test]
fn agent_runtime_types_agent_error_kind_maps_internal_errors() {
    assert_eq!(
        AgentErrorKind::from(LucarneError::Closed),
        AgentErrorKind::InvalidState
    );
    assert_eq!(
        AgentErrorKind::from(LucarneError::Adapter("unknown id \"missing\"".into())),
        AgentErrorKind::Unsupported
    );
    assert_eq!(
        AgentErrorKind::from(LucarneError::Adapter("adapter parse failed".into())),
        AgentErrorKind::Internal
    );
    assert_eq!(
        AgentErrorKind::from(LucarneError::Dialect("bad frame".into())),
        AgentErrorKind::Internal
    );
    assert_eq!(
        AgentErrorKind::from(LucarneError::Protocol("bad handshake".into())),
        AgentErrorKind::Internal
    );
    assert_eq!(
        AgentErrorKind::from(LucarneError::Runtime("boom".into())),
        AgentErrorKind::Internal
    );
    assert_eq!(
        AgentErrorKind::from(LucarneError::Launcher("boom".into())),
        AgentErrorKind::Internal
    );
}

#[test]
fn agent_runtime_types_open_and_resume_default_missing_args_to_null() {
    let open: OpenSession = from_str(r#"{"model":"claude"}"#).unwrap();
    assert_eq!(open.args, Value::Null);
    assert_eq!(open.idle_timeout_ms, None);

    let resume: ResumeSession = from_str(r#"{"session_ref":"session-ref-1"}"#).unwrap();
    assert_eq!(resume.args, Value::Null);
    assert_eq!(resume.idle_timeout_ms, None);
}

#[test]
fn agent_runtime_types_open_and_resume_accept_idle_timeout_ms() {
    let open: OpenSession = from_str(r#"{"model":"claude","idle_timeout_ms":15000}"#).unwrap();
    assert_eq!(open.idle_timeout_ms, Some(15_000));

    let resume: ResumeSession =
        from_str(r#"{"session_ref":"session-ref-1","idle_timeout_ms":0}"#).unwrap();
    assert_eq!(resume.idle_timeout_ms, Some(0));
}

#[test]
fn agent_runtime_types_intervention_request_serializes_with_type_tag() {
    let request = InterventionRequest::Approval(ApprovalRequest {
        req_id: SmolStr::new("req-1"),
        tool_name: SmolStr::new("shell"),
        message: Some("approve".into()),
        input: None,
    });

    let json = to_value(&request).unwrap();
    assert_eq!(json["type"], "approval");
    assert_eq!(json["payload"]["tool_name"], "shell");
}

#[test]
fn agent_runtime_types_runtime_command_serializes_with_type_tag() {
    let command = RuntimeCommand::UpdateFilter {
        filter: RuntimeBusFilter {
            session_lifecycle: true,
            user_messages: false,
            assistant_messages: true,
            reasoning: false,
            tool_calls: true,
            tool_results: false,
            usage: true,
            intervention_requests: false,
            turn_lifecycle: true,
        },
    };

    let json = to_value(&command).unwrap();
    assert_eq!(json["type"], "update_filter");
    assert_eq!(json["payload"]["filter"]["tool_calls"], true);
}

#[test]
fn agent_runtime_types_runtime_bus_output_serializes_with_type_tag() {
    let output = RuntimeBusOutput::SessionOpened(SessionOpenedEvent {
        command_id: CommandId(SmolStr::new("command-1")),
        instance_id: InstanceId(SmolStr::new("instance-1")),
        provider_id: ProviderId::from_static("claude"),
        session_id: SessionId(SmolStr::new("session-1")),
    });

    let json = to_value(&output).unwrap();
    assert_eq!(json["type"], "session_opened");
    assert_eq!(json["payload"]["provider_id"], "claude");
}

#[test]
fn agent_runtime_types_runtime_bus_event_serializes_with_type_tag() {
    let output = RuntimeBusOutput::Event(RuntimeBusEvent {
        instance_id: InstanceId(SmolStr::new("instance-1")),
        provider_id: ProviderId::from_static("claude"),
        session_id: SessionId(SmolStr::new("session-1")),
        event: lucarne::agent_runtime::Event::InterventionRequest(InterventionRequest::Question(
            QuestionRequest {
                req_id: SmolStr::new("req-1"),
                questions: vec![Question {
                    header: Some("hdr".into()),
                    text: "question?".into(),
                    options: vec![],
                    multi_select: false,
                }],
            },
        )),
    });

    let json = to_value(&output).unwrap();
    assert_eq!(json["type"], "event");
    assert_eq!(json["payload"]["event"]["type"], "question");
    assert_eq!(
        json["payload"]["event"]["payload"]["questions"][0]["text"],
        "question?"
    );
}

#[test]
fn agent_runtime_types_command_rejected_serializes_with_type_tag() {
    let output = RuntimeBusOutput::CommandRejected(CommandRejectedEvent {
        command_id: Some(CommandId(SmolStr::new("command-1"))),
        session_id: Some(SessionId(SmolStr::new("session-1"))),
        instance_id: Some(InstanceId(SmolStr::new("instance-1"))),
        message: "nope".into(),
    });

    let json = to_value(&output).unwrap();
    assert_eq!(json["type"], "command_rejected");
    assert_eq!(json["payload"]["message"], "nope");
}
