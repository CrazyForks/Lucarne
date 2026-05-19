# Common Attachment Output Design

## Context

Agent providers can produce non-text outputs. A current Codex image-generation turn emitted an `imageGeneration` item whose `result` contained PNG bytes as base64, while its final `agentMessage.text` was empty. Lucarne treated the turn as successful but dropped the image because the public event model and channel output model only expose text messages, tool events, usage, intervention requests, and turn lifecycle.

This is not a Codex- or imagegen-specific feature. Any provider may eventually return generated images, files, reports, archives, or other artifacts. Lucarne needs one common attachment output path that all providers can target and all channels can render or degrade.

## Goals

- Add a provider-agnostic output artifact model named `Attachment`.
- Allow all agent providers to emit attachments through the same public event and timeline path.
- Allow provider history/watch parsing to emit the same attachment events for saved transcripts and file-appended updates.
- Deliver attachments to Telegram, WeChat, and future channels without provider-specific branches in channel/common layers.
- Use channel-native media APIs for known media types: photos/images as photos/images, videos as videos, and only unknown or generic files as documents/files.
- Support turns with attachments and no text as successful visible output.
- Preserve provider responsibility boundaries: provider-specific item parsing stays inside provider dialects/descriptors.
- Keep attachment reads and delivery bounded by explicit size limits.

## Non-goals

- Do not add a public `ImageGeneration` event or any Codex-specific common event.
- Do not add provider-id switches in `lucarne::history`, channel code, or common control-plane logic.
- Do not build a full remote artifact hosting service.
- Do not require every channel to render inline previews; upload-as-file is acceptable only as fallback when no native media API exists or the media type is generic.
- Do not redesign incoming user attachment ingestion.
- Do not fetch arbitrary remote attachment URLs during history/watch projection in the initial implementation; only inline data and bounded local files are materialized.

## Data model

Add one shared type in the agent runtime event model:

```rust
pub struct Attachment {
    pub id: SmolStr,
    pub filename: SmolStr,
    pub media_type: SmolStr,
    pub data_base64: String,
    pub caption: Option<SmolStr>,
}
```

Add `Event::Attachment(Attachment)` to the public agent runtime event enum.

Field rules:

- `id` is provider item id when available, otherwise a stable provider-local generated id.
- `filename` must be safe for upload and include an extension when the media type implies one.
- `media_type` uses MIME types such as `image/png`, `image/svg+xml`, or `application/pdf`; unknown data uses `application/octet-stream`.
- `data_base64` carries bytes in-process so channel delivery does not depend on expiring provider URLs.
- `caption` is optional short text. Long textual explanation remains a normal assistant `Message` event.

Add `TimelineItemKind::Attachment`. Timeline payload stores the same fields plus decoded byte length. The runtime rejects attachments above the configured max size before recording them; it never truncates binary payloads.

## Provider mapping

Each provider maps its own artifact shapes to `Attachment` inside its provider-owned dialect/descriptor code. The same rule applies to `agent-sessions` provider history/watch parsing: provider modules extract provider-specific artifact shapes and expose them as a semantic watch attachment event, not as provider-name checks in core/history code.

Initial Codex live and history/watch mapping:

- `item.type == "imageGeneration"`
- Require non-empty `result` base64.
- Infer `media_type = "image/png"` for PNG bytes.
- Use `item.id` as `id`.
- Use a deterministic filename such as `codex-image-<short-id>.png`.
- Use `revisedPrompt` only as optional caption if it is short enough; otherwise omit it from delivery and keep it available in logs/timeline metadata if needed.
- Ignore in-progress image-generation items with empty `result`.
- When a transcript references a local artifact path instead of inline data, read it only through a provider-owned bounded reader and reject files above the attachment size limit.
- When a transcript contains only a remote URL and no inline/local bytes, record attachment metadata for history display but do not attempt channel upload in watch notification.

Future providers map their own generated files/images to the same `Attachment` type. Common layers must not inspect provider ids to decide parsing rules.

## Delivery flow

Text and attachments are separate events:

1. Live provider dialects emit zero or more `Message` events and zero or more `Attachment` events.
2. History/watch provider parsers emit zero or more semantic watch attachment events for appended transcript artifacts.
3. Core history watch projection maps watch attachments into the same runtime `Event::Attachment` used by live sessions.
4. Turn drain records each `Attachment` as `TimelineItemKind::Attachment`.
5. Turn drain immediately delivers each attachment through the active channel.
6. Turn completion with only attachments and no text is treated as visible output; final status still changes to complete.
7. If delivery fails, the turn is not re-run. The channel should send a visible error or fallback message when possible.

Attachment delivery does not replace text drafting. Text continues to use the existing draft/finalize pipeline.

## Channel behavior

Add `Channel::send_attachment` as a semantic channel capability, with a default implementation that falls back to `send_file` for generic upload support. `Attachment` stays provider-agnostic; channels choose their native API from `media_type`.

Telegram:

- Decode `data_base64` to bytes.
- `image/*` -> `send_photo` so Telegram shows the image inline.
- `video/*` -> `send_video`.
- Other media types -> `send_document` through the existing file upload path.
- Use `caption` when present and short enough for Telegram.
- Bind returned message ids to the provider session like text notifications, so replies can route to the session.

WeChat:

- Extend `WechatTransport` with attachment send/reply methods that accept bytes, filename, media type, and optional caption.
- Implement with `wechat_ilink::SendContent`:
  - `image/*` -> `SendContent::Image`
  - `video/*` -> `SendContent::Video`
  - all others -> `SendContent::File`
- Send caption according to iLink support; if a channel sends caption as a separate text message, bind both visible ids.
- Respect existing rate-limit handling. Rate-limited attachments are retained for delayed retry the same way text notifications are retained.

Unsupported channels:

- If a channel cannot upload attachments or files, send a text fallback: `Attachment omitted: <filename> (<media_type>, <size>)`.
- If base64 decode fails, record an error timeline item and show a concise failure message.

## Control-plane and history

Control-plane timeline records attachment metadata and bounded data. History replay renders attachment entries as downloadable files when channel support exists, or as metadata lines otherwise.

`agent-sessions` watch updates add a provider-neutral attachment event carrying the same fields needed by runtime `Attachment`. `lucarne::core_service` maps that watch event into `Event::Attachment` and broadcasts it to core subscribers and workspace subscribers exactly like watched assistant text.

Attachment payloads must be bounded. The default max decoded size is 8 MiB. Oversized attachments produce a visible error explaining that the artifact exceeded Lucarne's configured attachment size limit.

## Error handling

- Empty `result` / incomplete provider artifact: ignore until a completed item with bytes arrives.
- Invalid base64: emit no attachment, record provider parse warning/error, continue turn drain.
- Unknown media type: use `application/octet-stream` and upload as file.
- Channel upload failure: show visible upload failure in the current workspace; do not drop silently.
- Rate-limited WeChat send: retain pending attachment and retry after backoff.
- Attachment-only turn: never display only `✓ 完成` if attachment delivery succeeded or failed visibly.

## Testing

- Codex live dialect unit test: `imageGeneration` with PNG base64 emits `Event::Attachment` and no provider-specific common branch is needed.
- Codex history/watch parser test: appended `imageGeneration` with PNG base64 emits a provider-neutral watch attachment.
- Core history watch projection test: watch attachment becomes `Event::Attachment` and reaches core subscribers.
- Codex dialect/parser tests: in-progress `imageGeneration` with empty result emits nothing.
- Runtime/turn drain test: attachment-only turn records `TimelineItemKind::Attachment` and finalizes as successful visible output.
- Telegram test: image attachment calls native photo send, video attachment calls native video send, and generic file attachment calls document send.
- WeChat test: image attachment uses `SendContent::Image`, video attachment uses `SendContent::Video`, generic file attachment uses `SendContent::File`, and visible message ids are bound.
- WeChat rate-limit test: rate-limited attachment is retained and retried.
- Unsupported-channel test: attachment degrades to visible text fallback.
- Size-limit test: oversized attachment is rejected with visible error and no unbounded SQLite payload.
- History/timeline test: attachment payload round-trips through persistence with metadata intact.
- History replay test: transcript attachment renders as downloadable output when channel support exists, or metadata fallback otherwise.

## Acceptance criteria

- A Codex `$imagegen` turn with empty final text sends the generated PNG as a native Telegram photo instead of only showing `✓ 完成`.
- The same generated PNG is sent through WeChat as an image when a valid context exists.
- Common runtime/channel/control-plane code exposes only `Attachment`, not `ImageGeneration` or provider-specific types.
- Adding a future provider artifact requires changes only in that provider's live dialect or history/watch parser plus shared tests if needed.
- Oversized or invalid attachments produce visible errors instead of silent drops.
