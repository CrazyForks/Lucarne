//! Copilot dialect — one-shot JSONL stream from `gh copilot -p "..." --output-format json`.
//!
//! Wire shape (inbound lines):
//!
//! ```text
//! {"type":"session.start","data":{"sessionId":"…","selectedModel":"…"}}
//! {"type":"assistant.turn_start","data":{"turnId":"0"}}
//! {"type":"assistant.message_delta","data":{"messageId":"…","deltaContent":"…"}}
//! {"type":"assistant.message","data":{"messageId":"…","content":"…","reasoningText":"…",
//!                                     "toolRequests":[{…}], "outputTokens":N}}
//! {"type":"assistant.reasoning"|"assistant.reasoning_delta","data":{"content":"…" | "deltaContent":"…"}}
//! {"type":"tool.execution_complete","data":{"toolCallId":"…","model":"…","success":bool,
//!                                            "result":{"content":"…"},"error":{"message":"…"}}}
//! {"type":"session.warning","data":{"message":"…"}}
//! {"type":"session.error","data":{"message":"…"}}
//! {"type":"result","sessionId":"…","exitCode":N}
//! ```
//!
//! The adapter does not support multi-turn `Send` — the prompt goes in
//! via `-p` argv. Permission interception is not supported.

use crate::{
    dialect::{Dialect, Input, OutFrame, SessionParams},
    error::{LucarneError, Result},
    event::{
        self, AssistantMessage, Event, LogLine, Payload, PermissionResponse, ResumeHandle,
        SessionClosed, SessionStarted, Timeline, TimelineItem, TimelineType, ToolResult,
        TurnCompleted, TurnFailed, TurnStarted, Usage, UsageDelta,
    },
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use tracing::debug;

pub struct Copilot {
    cfg: SessionParams,
    session_id: String,
    active_model: String,
    seen_delta: HashSet<String>,
}

impl Copilot {
    pub fn new() -> Self {
        Self {
            cfg: SessionParams::default(),
            session_id: String::new(),
            active_model: String::new(),
            seen_delta: HashSet::new(),
        }
    }
}

impl Default for Copilot {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    data: Option<Value>,
    #[serde(default, rename = "sessionId")]
    session_id: String,
    #[serde(default, rename = "exitCode")]
    exit_code: i32,
}

impl Dialect for Copilot {
    fn name(&self) -> &'static str {
        "copilot"
    }

    fn init(&mut self, cfg: &SessionParams) -> Vec<OutFrame> {
        self.cfg = cfg.clone();
        self.active_model = cfg.model.clone();
        debug!(
            target: "lucarne::dialects::copilot",
            model = cfg.model.as_str(),
            cwd = cfg.cwd.as_str(),
            "copilot dialect initialized"
        );
        Vec::new()
    }

    fn translate(&mut self, frame: &[u8]) -> Vec<Event> {
        let env: Envelope = match serde_json::from_slice(frame) {
            Ok(v) => v,
            Err(err) => {
                debug!(
                    target: "lucarne::dialects::copilot",
                    error = %err,
                    "copilot frame skipped"
                );
                return Vec::new();
            }
        };
        let data = env.data.unwrap_or(Value::Null);
        match env.r#type.as_str() {
            "session.start" => {
                let session_id = data
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let model = data
                    .get("selectedModel")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                self.session_id = session_id.clone();
                if !model.is_empty() {
                    self.active_model = model;
                }
                debug!(
                    target: "lucarne::dialects::copilot",
                    session_id = session_id.as_str(),
                    model = self.active_model.as_str(),
                    "copilot session started"
                );
                vec![Event::new(Payload::SessionStarted(SessionStarted {
                    session_id,
                    model: self.active_model.clone(),
                }))]
            }

            "assistant.turn_start" => {
                vec![Event::new(Payload::TurnStarted(TurnStarted::default()))]
            }

            "assistant.message_delta" => {
                let message_id = data
                    .get("messageId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let delta = data
                    .get("deltaContent")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if delta.is_empty() {
                    return Vec::new();
                }
                self.seen_delta.insert(message_id.clone());
                vec![tl(TimelineItem {
                    ty: TimelineType::AssistantMessage,
                    id: message_id,
                    assistant_message: Some(AssistantMessage {
                        text: delta.into(),
                        streaming: true,
                    }),
                    ..Default::default()
                })]
            }

            "assistant.message" => {
                let message_id = data
                    .get("messageId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let content = data.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let reasoning = data
                    .get("reasoningText")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let output_tokens = data
                    .get("outputTokens")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let tool_requests = data
                    .get("toolRequests")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();

                let mut out = Vec::new();
                if !content.is_empty() && !self.seen_delta.contains(&message_id) {
                    out.push(tl(event::new_timeline_assistant(
                        &message_id,
                        content,
                        false,
                    )));
                }
                if !reasoning.is_empty() {
                    out.push(tl(event::new_timeline_reasoning("", reasoning)));
                }
                for req in tool_requests {
                    let tool_call_id = req
                        .get("toolCallId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = req
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let args = req.get("arguments").cloned().unwrap_or(Value::Null);
                    out.push(tl(event::new_timeline_tool_call(
                        &tool_call_id,
                        event::tool_call(name.as_str(), args),
                    )));
                }
                if output_tokens > 0 {
                    out.push(Event::new(Payload::UsageDelta(UsageDelta {
                        delta: Usage {
                            output_tokens,
                            ..Default::default()
                        },
                    })));
                }
                out
            }

            "assistant.reasoning" | "assistant.reasoning_delta" => {
                let content = data.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let delta = data
                    .get("deltaContent")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let text = if !content.is_empty() { content } else { delta };
                if text.is_empty() {
                    return Vec::new();
                }
                vec![tl(event::new_timeline_reasoning("", text))]
            }

            "tool.execution_complete" => {
                let tool_call_id = data
                    .get("toolCallId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if let Some(model) = data.get("model").and_then(|v| v.as_str()) {
                    if !model.is_empty() {
                        self.active_model = model.into();
                    }
                }
                let success = data
                    .get("success")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let mut result = ToolResult::default();
                if success {
                    if let Some(content) = data.pointer("/result/content").and_then(|v| v.as_str())
                    {
                        result.output = content.into();
                    }
                } else if let Some(msg) = data.pointer("/error/message").and_then(|v| v.as_str()) {
                    result.error = msg.into();
                }
                vec![tl(event::new_timeline_tool_result(
                    "",
                    &tool_call_id,
                    result,
                ))]
            }

            "session.warning" => {
                let msg = data.get("message").and_then(|v| v.as_str()).unwrap_or("");
                if msg.is_empty() {
                    return Vec::new();
                }
                vec![Event::new(Payload::Log(LogLine {
                    level: "warn".into(),
                    stream: "stdout".into(),
                    text: msg.into(),
                }))]
            }

            "session.error" => {
                let msg = data.get("message").and_then(|v| v.as_str()).unwrap_or("");
                if msg.is_empty() {
                    return Vec::new();
                }
                vec![Event::new(Payload::TurnFailed(TurnFailed {
                    error: msg.into(),
                    ..Default::default()
                }))]
            }

            "result" => {
                if !env.session_id.is_empty() {
                    self.session_id = env.session_id;
                }
                debug!(
                    target: "lucarne::dialects::copilot",
                    session_id = self.session_id.as_str(),
                    exit_code = env.exit_code,
                    "copilot result translated"
                );
                if env.exit_code != 0 {
                    vec![Event::new(Payload::TurnFailed(TurnFailed {
                        error: format!("copilot exited with code {}", env.exit_code),
                        ..Default::default()
                    }))]
                } else {
                    vec![Event::new(Payload::TurnCompleted(TurnCompleted::default()))]
                }
            }

            _ => Vec::new(),
        }
    }

    fn encode_user_message(&mut self, _in: &Input) -> Result<Vec<OutFrame>> {
        Err(LucarneError::dialect(
            "copilot: one-shot adapter does not support Send; pass FirstPrompt in Start",
        ))
    }

    fn encode_permission_response(
        &mut self,
        _req_id: &str,
        _r: &PermissionResponse,
    ) -> Result<Vec<OutFrame>> {
        Err(LucarneError::dialect(
            "copilot: permission interception is not supported",
        ))
    }

    fn encode_interrupt(&mut self) -> Result<Vec<OutFrame>> {
        Ok(vec![OutFrame::Signal("SIGINT".into())])
    }

    fn on_exit(&mut self, exit_code: i32, err: Option<String>) -> Vec<Event> {
        let reason = if let Some(e) = err {
            e
        } else if exit_code != 0 {
            format!("copilot exited with code {}", exit_code)
        } else {
            String::new()
        };
        let mut payload = SessionClosed {
            reason,
            resume: None,
        };
        if !self.session_id.is_empty() {
            let mut data: BTreeMap<String, Value> = BTreeMap::new();
            data.insert("session_id".into(), Value::String(self.session_id.clone()));
            if !self.cfg.cwd.is_empty() {
                data.insert("cwd".into(), Value::String(self.cfg.cwd.clone()));
            }
            payload.resume = Some(ResumeHandle { version: 1, data });
        }
        vec![Event::new(Payload::SessionClosed(payload))]
    }
}

fn tl(item: TimelineItem) -> Event {
    Event::new(Payload::Timeline(Timeline { item }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Kind, TimelineType};

    fn events_of(d: &mut Copilot, lines: &[&str]) -> Vec<Event> {
        let mut out = Vec::new();
        for l in lines {
            out.extend(d.translate(l.as_bytes()));
        }
        out
    }

    #[test]
    fn basic_flow_matches_go_expectations() {
        let mut d = Copilot::new();
        d.init(&SessionParams {
            model: "gpt-4o".into(),
            ..Default::default()
        });
        let evs = events_of(
            &mut d,
            &[
                r#"{"type":"session.start","data":{"sessionId":"sess-1","selectedModel":"claude-sonnet-4"}}"#,
                r#"{"type":"assistant.turn_start","data":{"turnId":"0"}}"#,
                r#"{"type":"assistant.message_delta","data":{"messageId":"msg-1","deltaContent":"pong"}}"#,
                r#"{"type":"assistant.message","data":{"messageId":"msg-1","content":"pong","reasoningText":"thinking step","toolRequests":[{"toolCallId":"tool-1","name":"bash","arguments":{"command":"ls -1"}}],"outputTokens":7}}"#,
                r#"{"type":"tool.execution_complete","data":{"toolCallId":"tool-1","model":"claude-opus-4.6","success":true,"result":{"content":"AGENTS.md\n"}}}"#,
                r#"{"type":"session.warning","data":{"message":"approaching rate limit"}}"#,
                r#"{"type":"result","sessionId":"sess-1","exitCode":0}"#,
            ],
        );

        // SessionStarted, TurnStarted, Timeline(delta), Timeline(reasoning),
        // Timeline(tool_call), UsageDelta, Timeline(tool_result), Log(warning),
        // TurnCompleted.
        let kinds: Vec<Kind> = evs.iter().map(|e| e.kind()).collect();
        assert_eq!(kinds[0], Kind::SessionStarted);
        assert_eq!(kinds[1], Kind::TurnStarted);
        assert_eq!(kinds[2], Kind::Timeline); // streaming delta
        assert_eq!(kinds[3], Kind::Timeline); // reasoning
        assert_eq!(kinds[4], Kind::Timeline); // tool_call
        assert_eq!(kinds[5], Kind::UsageDelta);
        assert_eq!(kinds[6], Kind::Timeline); // tool_result
        assert_eq!(kinds[7], Kind::Log);
        assert_eq!(kinds[8], Kind::TurnCompleted);

        // After the delta was seen, we must NOT emit a final (non-streaming)
        // AssistantMessage for the same messageId — same contract as Go.
        let assistants: Vec<_> = evs
            .iter()
            .filter_map(|e| match &e.payload {
                Payload::Timeline(t) if t.item.ty == TimelineType::AssistantMessage => {
                    Some(&t.item)
                }
                _ => None,
            })
            .collect();
        assert_eq!(assistants.len(), 1);
        let a = assistants[0].assistant_message.as_ref().unwrap();
        assert_eq!(a.text, "pong");
        assert!(a.streaming);
    }
}
