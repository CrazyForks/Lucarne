# Watch Attachment Notifications

## Context

Watched provider sessions can emit `AgentEvent::Attachment` after history/watch
projection maps provider artifacts into the common attachment event. Telegram
direct notifications previously handled only final assistant text messages in
`Bot::handle_core_event`, so attachment-only watched turns were silently ignored
even though the provider and core layers had already parsed the image.

## Decision

Handle watched `AgentEvent::Attachment` in the Telegram bot event router beside
assistant text. Delivery reuses the existing turn attachment helpers for base64
decode, size limits, caption splitting, upload retries, and visible delivery
failure messages.

The fix stays channel-owned and provider-agnostic: Telegram consumes the common
`Attachment` event and does not inspect Codex, image generation item types, or
provider-specific transcript fields.

Codex Desktop now records completed image generation as
`event_msg.payload.type = "image_generation_end"` with the image id in
`call_id`. The Codex provider treats that event as a provider-owned image
generation message and maps `call_id` into the attachment id before the common
watch layer sees it.

Some watched completions create deterministic local image files and mention them
only as standalone Markdown image links in `task_complete.last_agent_message`.
Those links are not provider-native media events, but they are local artifacts
the notification layer cannot render by URL. Core watch projection therefore
splits standalone local image links into bounded attachment events while keeping
unreadable or oversized links in the text.

## Consequences

- Attachment-only watched turns send a native channel attachment instead of
  disappearing from the notification topic.
- Attachment notification message ids are bound to the provider session, so a
  reply to the image can continue the same session.
- Missing notification or workspace topics are repaired through the same topic
  recreation flow used for watched text notifications.
- Local Markdown image links that point at readable image files are uploaded as
  native attachments instead of being shown only as local filesystem paths.
