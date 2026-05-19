# Common Attachment Output Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add one provider-agnostic `Attachment` output path so any agent can emit generated images/files and Telegram/WeChat deliver them with native media APIs.

**Architecture:** Provider dialects parse provider-owned artifact shapes into `Event::Attachment(Attachment)`. Core turn draining records `TimelineItemKind::Attachment` and calls `Channel::send_attachment`; channels choose native delivery from MIME type. WeChat keeps its own attachment retry path and uses `wechat_ilink::SendContent` at the provider boundary.

**Tech Stack:** Rust, Tokio, serde, base64, teloxide, wechat-ilink, lucarne control-plane timeline, lucarne-channel abstraction.

---

## File map

- `crates/lucarne/src/agent_runtime/events.rs`: define `Attachment` and add `Event::Attachment`.
- `crates/lucarne/src/agent_runtime/mod.rs`, `crates/lucarne/src/lib.rs`: re-export `Attachment`.
- `crates/lucarne/src/control_plane/types.rs`: add `TimelineItemKind::Attachment`.
- `crates/lucarne-channel/src/types.rs`, `crates/lucarne-channel/src/lib.rs`: add channel-level `Attachment` and `Channel::send_attachment` default fallback.
- `crates/lucarne/src/dialects/codex.rs`: map Codex live `imageGeneration` items into `Event::Attachment`.
- `agent-sessions/src/watch/event.rs`: add provider-neutral watch attachment event.
- `agent-sessions/src/providers/codex/types.rs`, `agent-sessions/src/providers/codex/mod.rs`, `agent-sessions/src/providers/codex/event.rs`: map Codex history/watch image-generation artifacts into watch attachments.
- `crates/lucarne/src/core_service/service.rs`: map watch attachments into runtime `Event::Attachment`.
- `crates/lucarne-telegram/src/turn.rs`: drain runtime and watched attachments, record timeline, enforce size/base64 rules, send attachment through channel, handle attachment-only turns.
- `crates/lucarne-telegram/src/channel.rs`: implement Telegram native `send_photo`, `send_video`, fallback `send_document`.
- `crates/lucarne-wechat/src/service.rs`: add pending attachment state, direct reply/notification delivery, retry, tests.
- `crates/lucarne-wechat/src/adapter.rs`: map WeChat transport attachment methods to `wechat_ilink::SendContent`.

## Task 1: Common attachment model and channel contract

**Files:**
- Modify: `crates/lucarne/src/agent_runtime/events.rs`
- Modify: `crates/lucarne/src/agent_runtime/mod.rs`
- Modify: `crates/lucarne/src/lib.rs`
- Modify: `crates/lucarne/src/control_plane/types.rs`
- Modify: `crates/lucarne-channel/src/types.rs`
- Modify: `crates/lucarne-channel/src/lib.rs`
- Test: `crates/lucarne-channel/src/lib.rs` unit test module or `crates/lucarne-channel/src/types.rs` test module

- [ ] **Step 1: Write failing channel fallback test**

Add a test-only fake channel that implements `send_file` and relies on default `send_attachment`:

```rust
#[cfg(test)]
mod attachment_tests {
    use super::*;
    use async_trait::async_trait;
    use futures::stream::{self, BoxStream};
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingChannel {
        files: Mutex<Vec<FileUpload>>,
    }

    #[async_trait]
    impl Channel for RecordingChannel {
        fn name(&self) -> &'static str { "recording" }
        fn message_char_limit(&self) -> usize { 4096 }
        async fn send(&self, _target: &WorkspaceHandle, _msg: OutgoingMessage) -> Result<MessageId> {
            Ok(MessageId::new("text-1"))
        }
        async fn edit(&self, _target: &WorkspaceHandle, _id: &MessageId, _msg: OutgoingMessage) -> Result<()> { Ok(()) }
        async fn create_workspace(&self, _parent: &ChatId, _title: &str) -> Result<WorkspaceHandle> {
            Err(ChannelError::Unsupported("create_workspace".into()))
        }
        async fn rename_workspace(&self, _handle: &WorkspaceHandle, _title: &str) -> Result<()> { Ok(()) }
        fn subscribe(&self) -> BoxStream<'static, ChannelEvent> { stream::empty().boxed() }
        async fn download_attachment(&self, _att: &IncomingAttachment) -> Result<Vec<u8>> { Ok(Vec::new()) }
        async fn send_file(&self, _target: &WorkspaceHandle, file: FileUpload) -> Result<MessageId> {
            self.files.lock().unwrap().push(file);
            Ok(MessageId::new("file-1"))
        }
    }

    #[tokio::test]
    async fn send_attachment_default_falls_back_to_file_upload() {
        let channel = RecordingChannel::default();
        let target = WorkspaceHandle::new(ChatId::new("1"), WorkspaceId::new("2"));
        let attachment = Attachment {
            filename: "logo.png".into(),
            media_type: "image/png".into(),
            bytes: vec![1, 2, 3],
            caption: Some("Logo".into()),
            reply_to: Some(MessageId::new("source")),
        };

        let id = channel.send_attachment(&target, attachment).await.unwrap();

        assert_eq!(id.as_str(), "file-1");
        let files = channel.files.lock().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "logo.png");
        assert_eq!(files[0].bytes, vec![1, 2, 3]);
        assert_eq!(files[0].caption.as_deref(), Some("Logo"));
        assert_eq!(files[0].reply_to.as_ref().map(|id| id.as_str()), Some("source"));
    }
}
```

- [ ] **Step 2: Run failing test**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-channel send_attachment_default_falls_back_to_file_upload
```

Expected: compile failure because `Attachment` and `Channel::send_attachment` do not exist.

- [ ] **Step 3: Add runtime `Attachment` event**

In `crates/lucarne/src/agent_runtime/events.rs` add:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: SmolStr,
    pub filename: SmolStr,
    pub media_type: SmolStr,
    pub data_base64: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<SmolStr>,
}
```

Add `Attachment(Attachment),` to `Event` and make `matches_filter` treat it as assistant output:

```rust
Self::Attachment(_) => filter.assistant_messages,
```

Re-export `Attachment` from `crates/lucarne/src/agent_runtime/mod.rs` and `crates/lucarne/src/lib.rs`.

- [ ] **Step 4: Add timeline kind**

In `crates/lucarne/src/control_plane/types.rs` add enum variant:

```rust
Attachment,
```

Place it near `Assistant` because it is assistant-visible output.

- [ ] **Step 5: Add channel `Attachment` and default delivery**

In `crates/lucarne-channel/src/types.rs` add:

```rust
#[derive(Debug, Clone)]
pub struct Attachment {
    pub filename: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
    pub caption: Option<String>,
    pub reply_to: Option<MessageId>,
}
```

Add constructor helpers:

```rust
impl Attachment {
    pub fn new(filename: impl Into<String>, media_type: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self { filename: filename.into(), media_type: media_type.into(), bytes, caption: None, reply_to: None }
    }
    pub fn with_caption(mut self, caption: impl Into<String>) -> Self {
        self.caption = Some(caption.into());
        self
    }
    pub fn reply_to(mut self, id: MessageId) -> Self {
        self.reply_to = Some(id);
        self
    }
}
```

In `crates/lucarne-channel/src/lib.rs` export `Attachment` and add default method to `Channel`:

```rust
async fn send_attachment(&self, target: &WorkspaceHandle, attachment: Attachment) -> Result<MessageId> {
    let mut upload = FileUpload::new(attachment.filename, attachment.bytes);
    if let Some(caption) = attachment.caption {
        upload = upload.with_caption(caption);
    }
    if let Some(reply_to) = attachment.reply_to {
        upload = upload.reply_to(reply_to);
    }
    self.send_file(target, upload).await
}
```

- [ ] **Step 6: Run test**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-channel send_attachment_default_falls_back_to_file_upload
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lucarne/src/agent_runtime/events.rs crates/lucarne/src/agent_runtime/mod.rs crates/lucarne/src/lib.rs crates/lucarne/src/control_plane/types.rs crates/lucarne-channel/src/types.rs crates/lucarne-channel/src/lib.rs
git commit -m "feat(core): Add common attachment event"
```

## Task 2: Codex imageGeneration to Attachment

**Files:**
- Modify: `crates/lucarne/src/dialects/codex.rs`
- Test: `crates/lucarne/src/dialects/codex.rs`

- [ ] **Step 1: Add failing Codex dialect tests**

Add tests near existing Codex item parsing tests:

```rust
#[test]
fn codex_image_generation_completed_emits_attachment() {
    let mut dialect = Codex::new();
    dialect.init(&SessionParams::default());
    let png = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nbody");
    let frame = format!(
        r#"{{"method":"item/completed","params":{{"item":{{"type":"imageGeneration","id":"ig_abc123","status":"generating","revisedPrompt":"short caption","result":"{png}"}},"threadId":"t1","turnId":"turn-1","completedAtMs":1}}}}"#
    );

    let events = dialect.on_frame(frame.as_bytes()).unwrap();

    let attachment = events
        .into_iter()
        .find_map(|event| match event.payload {
            Payload::Attachment(attachment) => Some(attachment),
            _ => None,
        })
        .expect("attachment event");
    assert_eq!(attachment.id.as_str(), "ig_abc123");
    assert_eq!(attachment.filename.as_str(), "codex-image-ig_abc123.png");
    assert_eq!(attachment.media_type.as_str(), "image/png");
    assert_eq!(attachment.data_base64, png);
    assert_eq!(attachment.caption.as_deref(), Some("short caption"));
}

#[test]
fn codex_image_generation_in_progress_without_result_emits_nothing() {
    let mut dialect = Codex::new();
    dialect.init(&SessionParams::default());
    let frame = br#"{"method":"item/started","params":{"item":{"type":"imageGeneration","id":"ig_empty","status":"in_progress","revisedPrompt":null,"result":""},"threadId":"t1","turnId":"turn-1","startedAtMs":1}}"#;

    let events = dialect.on_frame(frame).unwrap();

    assert!(events.iter().all(|event| !matches!(event.payload, Payload::Attachment(_))));
}
```

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarne codex_image_generation
```

Expected: compile failure or failure because `Payload::Attachment` and mapping are missing.

- [ ] **Step 3: Implement Codex mapping**

In `handle_item_completed`, add a match arm before `_ => Vec::new()`:

```rust
"image_generation" => self.handle_image_generation_item(&id, item),
```

Confirm `normalize_item_type` maps camelCase to snake_case; if it does not, match both `"imagegeneration"` and `"image_generation"` locally inside provider code.

Add helper in `impl Codex`:

```rust
fn handle_image_generation_item(&mut self, id: &str, item: &Value) -> Vec<Event> {
    let result = item.get("result").and_then(|v| v.as_str()).unwrap_or("");
    if result.is_empty() {
        return Vec::new();
    }
    let short_id = if id.is_empty() { "image" } else { id };
    let caption = item
        .get("revisedPrompt")
        .and_then(|v| v.as_str())
        .filter(|text| !text.trim().is_empty() && text.chars().count() <= 256)
        .map(SmolStr::from);
    vec![Event::new(Payload::Attachment(crate::agent_runtime::Attachment {
        id: SmolStr::from(short_id),
        filename: SmolStr::from(format!("codex-image-{short_id}.png")),
        media_type: SmolStr::from("image/png"),
        data_base64: result.to_string(),
        caption,
    }))]
}
```

Use the existing `Payload` enum imported in `codex.rs`; no common-layer provider branch is added.

- [ ] **Step 4: Run tests**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarne codex_image_generation
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/lucarne/src/dialects/codex.rs
git commit -m "feat(codex): Emit attachments for generated images"
```

## Task 3: History/watch attachments project to runtime attachments

**Files:**
- Modify: `agent-sessions/src/watch/event.rs`
- Modify: `agent-sessions/src/providers/codex/types.rs`
- Modify: `agent-sessions/src/providers/codex/mod.rs`
- Modify: `agent-sessions/src/providers/codex/event.rs`
- Modify: `crates/lucarne/src/core_service/service.rs`
- Test: `agent-sessions/src/providers/codex/event.rs`
- Test: `crates/lucarne/src/core_service/service.rs`

- [ ] **Step 1: Add failing Codex watch parser test**

Add a test near Codex watch projection tests that feeds an appended Codex image-generation event and expects a watch attachment:

```rust
#[test]
fn codex_watch_image_generation_emits_attachment() {
    let png = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nbody");
    let raw = format!(
        r#"{{"timestamp":"2026-05-19T03:14:51.660Z","type":"event_msg","payload":{{"type":"imageGeneration","id":"ig_watch","status":"generating","revisedPrompt":"watch caption","result":"{png}"}}}}"#
    );
    let session = super::parse_codex_reader(raw.as_bytes(), crate::ParseSelection::full()).unwrap();
    let events = super::event::watch_events_from_codex_entries(&session.entries, crate::ParseSelection::full());

    let attachment = events.iter().find_map(|event| match event {
        crate::watch::WatchEvent::Attachment(attachment) => Some(attachment),
        _ => None,
    }).expect("watch attachment");

    assert_eq!(attachment.id.as_deref(), Some("ig_watch"));
    assert_eq!(attachment.filename.as_str(), "codex-image-ig_watch.png");
    assert_eq!(attachment.media_type.as_str(), "image/png");
    assert_eq!(attachment.data_base64.as_str(), png);
    assert_eq!(attachment.caption.as_deref(), Some("watch caption"));
}
```

Place this test in `agent-sessions/src/providers/codex/event.rs` so it can access `watch_events_from_codex_entries` and provider-private parse helpers. The test must assert `WatchEvent::Attachment`, not `Unknown`.

- [ ] **Step 2: Add failing core projection test**

In `crates/lucarne/src/core_service/service.rs`, add a test near existing history-watch projection tests:

```rust
#[tokio::test]
async fn history_watch_attachment_projects_to_core_event() {
    let provider = Arc::new(FakeProvider::default());
    let core = test_core_with_provider(provider);
    let mut events = core.watch_events();
    let update = WatchUpdate {
        provider: WatchProvider::Codex,
        path: PathBuf::from("/tmp/codex-session.jsonl"),
        session_id: Some("session-watch-attachment".into()),
        cwd: Some("/tmp/project".into()),
        change: WatchChange::Updated,
        events: vec![WatchEvent::Attachment(WatchAttachment {
            meta: WatchEventMeta { id: Some("ig_watch".into()), ..Default::default() },
            id: Some("ig_watch".into()),
            filename: "codex-image-ig_watch.png".into(),
            media_type: "image/png".into(),
            data_base64: base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3]),
            caption: Some("watch caption".into()),
        })].into_boxed_slice(),
        error: None,
    };

    core.handle_history_watch_update(update).expect("watch update");

    let event = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let CoreEvent::TimelineEvent { event: AgentEvent::Attachment(attachment), .. } = events.recv().await.unwrap() {
                break attachment;
            }
        }
    }).await.expect("attachment event");

    assert_eq!(event.id.as_str(), "ig_watch");
    assert_eq!(event.filename.as_str(), "codex-image-ig_watch.png");
    assert_eq!(event.media_type.as_str(), "image/png");
}
```

Place this test near `history_watch_update_emits_core_event_for_external_provider_session` and reuse the same `EnvGuard`, `write_codex_history_session`, `AgentRuntime::new()`, and `CatalogProvider` setup style from that test. Keep the assertion target as `AgentEvent::Attachment`.

- [ ] **Step 3: Run failing tests**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p agent-sessions codex_watch_image_generation_emits_attachment
cargo +nightly test -Zbuild-dir-new-layout -p lucarne history_watch_attachment_projects_to_core_event
```

Expected: compile failures because watch attachment types and projection do not exist.

- [ ] **Step 4: Add watch attachment type**

In `agent-sessions/src/watch/event.rs`, add `WatchEvent::Attachment(WatchAttachment)` and struct:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchAttachment {
    pub meta: WatchEventMeta,
    pub id: Option<SmolStr>,
    pub filename: SmolStr,
    pub media_type: SmolStr,
    pub data_base64: SmolStr,
    pub caption: Option<SmolStr>,
}
```

- [ ] **Step 5: Parse Codex watch image-generation artifacts**

In `agent-sessions/src/providers/codex/types.rs`, add an `ImageGeneration(ImageGenerationEventMsg)` variant and struct:

```rust
ImageGeneration(ImageGenerationEventMsg),

#[derive(Debug)]
pub(crate) struct ImageGenerationEventMsg {
    pub id: Option<SmolStr>,
    pub status: Option<SmolStr>,
    pub revised_prompt: Option<SmolStr>,
    pub result_base64: Option<SmolStr>,
}
```

In `agent-sessions/src/providers/codex/mod.rs`, map payload kinds `"imageGeneration"`, `"image_generation"`, and `"image_generation_result"` to this variant. Populate fields from `payload.id`, `payload.status`, `payload.revised_prompt` or `payload.revisedPrompt`, and `payload.result`. Keep this in Codex provider code only.

In `agent-sessions/src/providers/codex/event.rs`, add helper:

```rust
#[cfg(feature = "watch")]
fn codex_watch_image_generation(
    meta: crate::watch::WatchEventMeta,
    data: &super::ImageGenerationEventMsg,
) -> Option<crate::watch::WatchEvent> {
    let result = data.result_base64.as_deref()?.trim();
    if result.is_empty() {
        return None;
    }
    let id = data.id.clone().or_else(|| meta.id.clone());
    let short_id = id.as_deref().unwrap_or("image");
    let caption = data
        .revised_prompt
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty() && text.chars().count() <= 256)
        .map(crate::watch::watch_smol);
    Some(crate::watch::WatchEvent::Attachment(crate::watch::WatchAttachment {
        meta,
        id,
        filename: crate::watch::watch_smol(format!("codex-image-{short_id}.png")),
        media_type: crate::watch::watch_smol("image/png"),
        data_base64: crate::watch::watch_smol(result.to_string()),
        caption,
    }))
}
```

In `watch_events_from_codex_entries`, handle `EventMsgData::ImageGeneration(data)` by pushing the helper result when present. Ignore empty `result` values.

- [ ] **Step 6: Map watch attachment to runtime attachment**

In `crates/lucarne/src/core_service/service.rs`, add a `WatchEvent::Attachment(attachment)` branch in `handle_history_watch_update`:

```rust
WatchEvent::Attachment(attachment) => AgentEvent::Attachment(crate::agent_runtime::Attachment {
    id: attachment
        .id
        .clone()
        .unwrap_or_else(|| attachment.meta.id.clone().unwrap_or_else(|| "attachment".into())),
    filename: attachment.filename.clone(),
    media_type: attachment.media_type.clone(),
    data_base64: attachment.data_base64.to_string(),
    caption: attachment.caption.clone(),
}),
```

This branch must be provider-neutral and must not inspect `update.provider`.

- [ ] **Step 7: Run tests**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p agent-sessions codex_watch_image_generation_emits_attachment
cargo +nightly test -Zbuild-dir-new-layout -p lucarne history_watch_attachment_projects_to_core_event
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add agent-sessions/src/watch/event.rs agent-sessions/src/providers/codex/types.rs agent-sessions/src/providers/codex/mod.rs agent-sessions/src/providers/codex/event.rs crates/lucarne/src/core_service/service.rs
git commit -m "feat(watch): Project history attachments"
```

## Task 4: Turn drain records and sends attachments

**Files:**
- Modify: `crates/lucarne-telegram/src/turn.rs`
- Test: `crates/lucarne-telegram/src/turn.rs`

- [ ] **Step 1: Add failing turn-drain test**

Extend `TestChannel` with attachment recording:

```rust
attachments: StdMutex<Vec<lucarne_channel::Attachment>>,
```

Implement `send_attachment` in `impl Channel for TestChannel`:

```rust
async fn send_attachment(
    &self,
    _target: &WorkspaceHandle,
    attachment: lucarne_channel::Attachment,
) -> Result<MessageId> {
    let mut attachments = self.attachments.lock().unwrap();
    attachments.push(attachment);
    Ok(MessageId::new(format!("att-{}", attachments.len())))
}
```

Add test:

```rust
#[tokio::test]
async fn attachment_only_turn_uploads_attachment_and_records_timeline() {
    let test_channel = Arc::new(TestChannel::default());
    let channel: Arc<dyn Channel> = test_channel.clone();
    let target = test_target();
    let (tx, rx) = test_event_stream(&target.workspace, 4);
    let data = base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3, 4]);
    send_test_event(
        &tx,
        &target,
        Event::Attachment(lucarne::agent_runtime::Attachment {
            id: "ig_1".into(),
            filename: "logo.png".into(),
            media_type: "image/png".into(),
            data_base64: data,
            caption: Some("Logo".into()),
        }),
    );
    send_test_event(
        &tx,
        &target,
        Event::TurnCompleted(lucarne::agent_runtime::events::TurnCompletedEvent {
            turn_id: "provider-turn".into(),
            usage: None,
        }),
    );
    drop(tx);
    let live = LiveSession {
        session: Arc::new(SilentSession {
            id: SessionId("session-attachment".into()),
            instance_id: InstanceId("instance-attachment".into()),
        }),
        events: tokio::sync::Mutex::new(rx),
        pending_intv: StdMutex::new(std::collections::HashMap::new()),
    };
    let shared = Shared::new();
    let mut drafts = DraftStream::new();
    let recorder = Arc::new(NormalizingTimelineRecorder::default());

    drain_events(
        &channel,
        &target,
        &live,
        &shared,
        &mut drafts,
        DrainMode::Turn(TurnRunOptions {
            recording: Some(TurnRecording {
                turn_id: ControlTurnId::new("turn-attachment"),
                recorder: recorder.clone(),
            }),
            ..Default::default()
        }),
    )
    .await
    .expect("turn should drain");

    let attachments = test_channel.attachments.lock().unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].filename, "logo.png");
    assert_eq!(attachments[0].media_type, "image/png");
    assert_eq!(attachments[0].bytes, vec![1, 2, 3, 4]);
    assert_eq!(attachments[0].caption.as_deref(), Some("Logo"));

    let items = recorder.items.lock().unwrap();
    assert!(items.iter().any(|(kind, payload)| {
        *kind == TimelineItemKind::Attachment
            && payload.get("filename").and_then(|v| v.as_str()) == Some("logo.png")
            && payload.get("byte_len").and_then(|v| v.as_u64()) == Some(4)
    }));
}
```

- [ ] **Step 2: Run failing test**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-telegram attachment_only_turn_uploads_attachment_and_records_timeline
```

Expected: compile failure until drain handles attachments.

- [ ] **Step 3: Implement attachment drain helper**

In `turn.rs`, add constant:

```rust
const MAX_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;
```

Add helper:

```rust
fn attachment_payload(attachment: &lucarne::agent_runtime::Attachment, byte_len: usize) -> serde_json::Value {
    serde_json::json!({
        "id": attachment.id.as_str(),
        "filename": attachment.filename.as_str(),
        "media_type": attachment.media_type.as_str(),
        "data_base64": attachment.data_base64,
        "caption": attachment.caption.as_deref(),
        "byte_len": byte_len,
    })
}
```

Add decode/send helper:

```rust
async fn send_runtime_attachment(
    channel: &Arc<dyn Channel>,
    target: &WorkspaceHandle,
    attachment: &lucarne::agent_runtime::Attachment,
) -> Result<MessageId, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&attachment.data_base64)
        .map_err(|err| format!("attachment {} is invalid base64: {err}", attachment.filename))?;
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "attachment {} is {} bytes, above {} byte limit",
            attachment.filename,
            bytes.len(),
            MAX_ATTACHMENT_BYTES
        ));
    }
    let mut channel_attachment = lucarne_channel::Attachment::new(
        attachment.filename.to_string(),
        attachment.media_type.to_string(),
        bytes,
    );
    if let Some(caption) = attachment.caption.as_ref() {
        channel_attachment = channel_attachment.with_caption(caption.to_string());
    }
    channel.send_attachment(target, channel_attachment).await.map_err(|err| err.to_string())
}
```

- [ ] **Step 4: Handle `Event::Attachment` in `drain_events`**

Add a match arm before `Event::Usage`:

```rust
Ok(Ok(Event::Attachment(attachment))) => {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&attachment.data_base64)
        .map_err(|err| format!("attachment {} is invalid base64: {err}", attachment.filename))?;
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "attachment {} is {} bytes, above {} byte limit",
            attachment.filename,
            bytes.len(),
            MAX_ATTACHMENT_BYTES
        ));
    }
    let item = record_timeline(
        &mode,
        target,
        TimelineItemKind::Attachment,
        attachment_payload(&attachment, bytes.len()),
    )?;
    let mut channel_attachment = lucarne_channel::Attachment::new(
        attachment.filename.to_string(),
        attachment.media_type.to_string(),
        bytes,
    );
    if let Some(caption) = attachment.caption.as_ref() {
        channel_attachment = channel_attachment.with_caption(caption.to_string());
    }
    let _message_id = channel
        .send_attachment(target, channel_attachment)
        .await
        .map_err(|err| err.to_string())?;
    awaiting_intervention = false;
    has_message = true;
    shared.set_activity(format!("📎 sent {}", item.payload["filename"].as_str().unwrap_or("attachment")));
}
```

- [ ] **Step 5: Run test**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-telegram attachment_only_turn_uploads_attachment_and_records_timeline
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/lucarne-telegram/src/turn.rs
git commit -m "feat(telegram): Drain common attachments"
```

## Task 5: Telegram native photo/video/document delivery

**Files:**
- Modify: `crates/lucarne-telegram/src/channel.rs`
- Test: `crates/lucarne-telegram/src/channel.rs`

- [ ] **Step 1: Add MIME routing unit test**

Add helper tests near channel tests:

```rust
#[cfg(test)]
mod attachment_delivery_tests {
    use super::*;

    #[test]
    fn telegram_attachment_kind_uses_native_media_types() {
        assert_eq!(telegram_attachment_kind("image/png"), TelegramAttachmentKind::Photo);
        assert_eq!(telegram_attachment_kind("image/jpeg"), TelegramAttachmentKind::Photo);
        assert_eq!(telegram_attachment_kind("video/mp4"), TelegramAttachmentKind::Video);
        assert_eq!(telegram_attachment_kind("application/pdf"), TelegramAttachmentKind::Document);
        assert_eq!(telegram_attachment_kind(""), TelegramAttachmentKind::Document);
    }
}
```

- [ ] **Step 2: Run failing test**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-telegram telegram_attachment_kind_uses_native_media_types
```

Expected: compile failure because helper enum/function do not exist.

- [ ] **Step 3: Implement Telegram attachment kind helper**

In `channel.rs` add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TelegramAttachmentKind {
    Photo,
    Video,
    Document,
}

fn telegram_attachment_kind(media_type: &str) -> TelegramAttachmentKind {
    let media_type = media_type.trim().to_ascii_lowercase();
    if media_type.starts_with("image/") {
        TelegramAttachmentKind::Photo
    } else if media_type.starts_with("video/") {
        TelegramAttachmentKind::Video
    } else {
        TelegramAttachmentKind::Document
    }
}
```

- [ ] **Step 4: Implement `send_attachment` for `TelegramChannel`**

Add imports:

```rust
SendPhotoSetters, SendVideoSetters,
```

Add `Attachment` to lucarne-channel imports.

First extract the existing document upload body from `send_file` into a private helper on `TelegramChannel`:

```rust
async fn send_document_upload(
    &self,
    target: &WorkspaceHandle,
    filename: String,
    bytes: Vec<u8>,
    caption: Option<String>,
    reply_to: Option<MessageId>,
) -> Result<MessageId> {
    let chat = parse_tg_chat_id(&target.chat)?;
    let thread = parse_tg_thread(&target.workspace);
    info!(
        target: "lucarne_telegram",
        chat = chat.0,
        thread = ?thread.map(|t| t.0.0),
        filename = %filename,
        bytes = bytes.len(),
        "uploading document"
    );
    let input = InputFile::memory(bytes).file_name(filename);
    let mut req = self.bot.send_document(chat, input);
    if let Some(t) = thread {
        req = req.message_thread_id(t);
    }
    if let Some(caption) = caption {
        req = req.caption(caption);
    }
    if let Some(reply_to) = reply_to.as_ref() {
        req = req.reply_parameters(
            ReplyParameters::new(parse_tg_message_id(reply_to)?).allow_sending_without_reply(),
        );
    }
    let m = req.await.map_err(|e| {
        let m = map_err(e);
        warn!(target: "lucarne_telegram", error = %m, "send_document failed");
        m
    })?;
    Ok(MessageId::new(m.id.0.to_string()))
}
```

Then make existing `send_file` call this helper:

```rust
async fn send_file(&self, target: &WorkspaceHandle, file: FileUpload) -> Result<MessageId> {
    self.send_document_upload(target, file.filename, file.bytes, file.caption, file.reply_to)
        .await
}
```

Implement `send_attachment` in `impl Channel for TelegramChannel`:

```rust
async fn send_attachment(&self, target: &WorkspaceHandle, attachment: Attachment) -> Result<MessageId> {
    match telegram_attachment_kind(&attachment.media_type) {
        TelegramAttachmentKind::Photo => {
            let chat = parse_tg_chat_id(&target.chat)?;
            let thread = parse_tg_thread(&target.workspace);
            let input = InputFile::memory(attachment.bytes).file_name(attachment.filename);
            let mut req = self.bot.send_photo(chat, input);
            if let Some(t) = thread { req = req.message_thread_id(t); }
            if let Some(caption) = attachment.caption { req = req.caption(caption); }
            if let Some(reply_to) = attachment.reply_to.as_ref() {
                req = req.reply_parameters(ReplyParameters::new(parse_tg_message_id(reply_to)?).allow_sending_without_reply());
            }
            let message = req.await.map_err(map_err)?;
            Ok(MessageId::new(message.id.0.to_string()))
        }
        TelegramAttachmentKind::Video => {
            let chat = parse_tg_chat_id(&target.chat)?;
            let thread = parse_tg_thread(&target.workspace);
            let input = InputFile::memory(attachment.bytes).file_name(attachment.filename);
            let mut req = self.bot.send_video(chat, input);
            if let Some(t) = thread { req = req.message_thread_id(t); }
            if let Some(caption) = attachment.caption { req = req.caption(caption); }
            if let Some(reply_to) = attachment.reply_to.as_ref() {
                req = req.reply_parameters(ReplyParameters::new(parse_tg_message_id(reply_to)?).allow_sending_without_reply());
            }
            let message = req.await.map_err(map_err)?;
            Ok(MessageId::new(message.id.0.to_string()))
        }
        TelegramAttachmentKind::Document => {
            self.send_document_upload(
                target,
                attachment.filename,
                attachment.bytes,
                attachment.caption,
                attachment.reply_to,
            )
            .await
        }
    }
}
```

Do not duplicate document upload logic outside `send_document_upload`.

- [ ] **Step 5: Run tests**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-telegram telegram_attachment_kind_uses_native_media_types
cargo +nightly check -Zbuild-dir-new-layout -p lucarne-telegram
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/lucarne-telegram/src/channel.rs
git commit -m "feat(telegram): Send attachments with native media APIs"
```

## Task 6: WeChat attachment send/retry path

**Files:**
- Modify: `crates/lucarne-wechat/src/service.rs`
- Modify: `crates/lucarne-wechat/src/adapter.rs`
- Test: `crates/lucarne-wechat/src/service.rs`

- [ ] **Step 1: Add WeChat service tests**

Add `AttachmentRecord` to test module and extend `FakeTransport` with `attachments: StdMutex<Vec<AttachmentRecord>>` and rate-limit controls for attachment sends.

Test notification attachment:

```rust
#[tokio::test]
async fn wechat_notification_attachment_sends_image_media() {
    let provider = Arc::new(FakeProvider::default());
    let core = test_core(Arc::clone(&provider));
    let opened = core.open_workspace_with_events(OpenWorkspaceRequest {
        provider_id: "codex",
        project_path: Some("/tmp/wechat-attachment".into()),
        title: "wechat-attachment".into(),
    }).await.expect("open workspace");
    let workspace_id = opened.workspace.workspace_id.clone();
    let transport = Arc::new(FakeTransport::default());
    transport.store_context(wechat_ilink::WechatContext {
        account_key: "account-1".into(),
        user_id: "user-1".into(),
        context_token: "ctx-1".into(),
        observed_at_unix_ms: 1,
        source_message_id: Some("msg-1".into()),
    });
    let service = WechatNotificationService::new(
        core,
        Arc::clone(&transport),
        WechatServiceOptions { initial_user_ids: vec!["user-1".into()], ..Default::default() },
    );

    service.handle_core_event(CoreEvent::TimelineEvent {
        workspace_id,
        turn_id: None,
        event: AgentEvent::Attachment(lucarne::agent_runtime::Attachment {
            id: "ig-1".into(),
            filename: "logo.png".into(),
            media_type: "image/png".into(),
            data_base64: base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3]),
            caption: Some("Logo".into()),
        }),
    }).await.unwrap();

    let attachments = transport.attachments();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].user_id, "user-1");
    assert_eq!(attachments[0].filename, "logo.png");
    assert_eq!(attachments[0].media_type, "image/png");
    assert_eq!(attachments[0].bytes, vec![1, 2, 3]);
}
```

Test rate limit:

```rust
#[tokio::test]
async fn wechat_rate_limited_attachment_waits_before_retrying() {
    let provider = Arc::new(FakeProvider::default());
    let core = test_core(Arc::clone(&provider));
    let opened = core.open_workspace_with_events(OpenWorkspaceRequest {
        provider_id: "codex",
        project_path: Some("/tmp/wechat-attachment-rate-limit".into()),
        title: "wechat-attachment-rate-limit".into(),
    }).await.expect("open workspace");
    let workspace_id = opened.workspace.workspace_id.clone();
    let transport = Arc::new(FakeTransport::default());
    transport.store_context(wechat_ilink::WechatContext {
        account_key: "account-1".into(),
        user_id: "user-1".into(),
        context_token: "ctx-1".into(),
        observed_at_unix_ms: 1,
        source_message_id: Some("msg-1".into()),
    });
    let service = WechatNotificationService::new(
        core,
        Arc::clone(&transport),
        WechatServiceOptions { initial_user_ids: vec!["user-1".into()], ..Default::default() },
    );

    transport.fail_attachment_sends_rate_limited(Duration::from_millis(50));
    service.handle_core_event(CoreEvent::TimelineEvent {
        workspace_id: workspace_id.clone(),
        turn_id: None,
        event: AgentEvent::Attachment(lucarne::agent_runtime::Attachment {
            id: "ig-1".into(),
            filename: "logo.png".into(),
            media_type: "image/png".into(),
            data_base64: base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3]),
            caption: Some("Logo".into()),
        }),
    }).await.unwrap();
    assert!(transport.attachments().is_empty());
    assert_eq!(service.pending_attachment_count(), 1);

    transport.clear_attachment_failures();
    service.retry_pending_attachments().await.unwrap();
    assert!(transport.attachments().is_empty());
    assert_eq!(service.pending_attachment_count(), 1);

    tokio::time::sleep(Duration::from_millis(60)).await;
    service.retry_pending_attachments().await.unwrap();
    let attachments = transport.attachments();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].filename, "logo.png");
    assert_eq!(service.pending_attachment_count(), 0);
}
```

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-wechat wechat_notification_attachment_sends_image_media wechat_rate_limited_attachment_waits_before_retrying
```

Expected: compile failure until trait/service support exists.

- [ ] **Step 3: Extend `WechatTransport`**

Add to trait:

```rust
async fn send_attachment(
    &self,
    context: &WechatContext,
    attachment: &lucarne::agent_runtime::Attachment,
) -> Result<WechatSendReceipt, WechatError>;

async fn reply_attachment(
    &self,
    message: &WechatIncoming,
    attachment: &lucarne::agent_runtime::Attachment,
) -> Result<WechatSendReceipt, WechatError>;
```

- [ ] **Step 4: Add pending attachment state**

Add struct:

```rust
#[derive(Clone)]
struct WechatPendingAttachment {
    workspace_id: WorkspaceId,
    user_id: String,
    attachment: lucarne::agent_runtime::Attachment,
    provider_session_id: ProviderSessionId,
}
```

Add `pending_attachments: VecDeque<WechatPendingAttachment>` to `WechatState`, max constant `MAX_PENDING_ATTACHMENTS: usize = 10`, and helper methods mirroring pending notifications:

```rust
fn remember_pending_attachment(
    &self,
    workspace_id: WorkspaceId,
    user_id: String,
    attachment: lucarne::agent_runtime::Attachment,
    provider_session_id: ProviderSessionId,
)

fn take_pending_attachments(&self) -> Vec<WechatPendingAttachment>

fn pending_attachment_count(&self) -> usize

async fn retry_pending_attachments(&self) -> Result<(), WechatError>
```

Call `retry_pending_attachments()` in the retry tick next to `retry_pending_notifications()`.

- [ ] **Step 5: Add send helpers and core-event arm**

Add `try_send_attachment_notification` mirroring `try_send_notification`, using `transport.send_attachment` and binding receipt ids to provider session.

Add `deliver_assistant_attachment` mirroring `deliver_assistant_message` with these rules:

- if direct notifications suppressed, skip background notification;
- if notifications disabled for workspace/session, skip;
- if no users, warn and skip;
- for each user, try send; if false, remember pending attachment.

Add `CoreEvent::TimelineEvent { event: AgentEvent::Attachment(attachment), .. }` arm in `handle_core_event`.

- [ ] **Step 6: Implement adapter media mapping**

In `adapter.rs`, import `SendContent` and `base64::Engine`. Implement helper:

```rust
fn attachment_send_content(attachment: &lucarne::agent_runtime::Attachment) -> Result<SendContent, WechatError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&attachment.data_base64)
        .map_err(|err| WechatError::Transport(format!("attachment {} invalid base64: {err}", attachment.filename)))?;
    let caption = attachment.caption.as_ref().map(|value| value.to_string());
    let media_type = attachment.media_type.as_str();
    if media_type.starts_with("image/") {
        Ok(SendContent::Image { data: bytes, caption })
    } else if media_type.starts_with("video/") {
        Ok(SendContent::Video { data: bytes, caption })
    } else {
        Ok(SendContent::File { data: bytes, file_name: attachment.filename.to_string(), caption })
    }
}
```

Implement trait methods by calling `bot.send_media_with_context(context, content)` and `bot.reply_media(&message.sdk_message, content)`.

- [ ] **Step 7: Run WeChat tests**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-wechat wechat_notification_attachment_sends_image_media wechat_rate_limited_attachment_waits_before_retrying
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/lucarne-wechat/src/service.rs crates/lucarne-wechat/src/adapter.rs crates/lucarne-wechat/Cargo.toml Cargo.lock
git commit -m "feat(wechat): Send common attachments"
```

## Task 7: Full verification and cleanup

**Files:**
- Modify only files touched by Tasks 1-6 if verification reveals compile or test issues.

- [ ] **Step 1: Run package tests**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-channel -p lucarne -p lucarne-telegram -p lucarne-wechat
```

Expected: all tests pass.

- [ ] **Step 2: Run check**

Run:

```bash
cargo +nightly check -Zbuild-dir-new-layout
```

Expected: PASS.

- [ ] **Step 3: Manual smoke with recovered image fixture**

Use the recovered image-generation log shape from `~/.lucarned/logs/lucarned.2026-05-19.log` line containing `"type":"imageGeneration"` and non-empty `"result"` to ensure Codex emits an attachment. If a live run is available, send `$imagegen` and verify Telegram displays a photo, not just a status message.

- [ ] **Step 4: Commit verification-only fixes when verification changed code**

If verification required code changes, add the exact files changed in Tasks 1-6 and commit them:

```bash
git status --short
git add crates/lucarne/src/agent_runtime/events.rs crates/lucarne/src/agent_runtime/mod.rs crates/lucarne/src/lib.rs crates/lucarne/src/control_plane/types.rs crates/lucarne-channel/src/types.rs crates/lucarne-channel/src/lib.rs crates/lucarne/src/dialects/codex.rs crates/lucarne-telegram/src/turn.rs crates/lucarne-telegram/src/channel.rs crates/lucarne-wechat/src/service.rs crates/lucarne-wechat/src/adapter.rs crates/lucarne-wechat/Cargo.toml Cargo.lock
git commit -m "fix: Polish attachment output integration"
```

If `git status --short` shows no code changes after verification, do not create an empty commit.
