# Grok Build is a default full Provider with live and watch notifications

Grok Build ships as a default-enabled full **Provider** (stable id `grok`, display name "Grok Build"): `agent-sessions` discovery/parse/watch, Lucarne adapter+dialect for live Dialogue, and channel **Notification** for both Lucarne-driven sessions and external sessions written by the Grok TUI/other clients under the same home. It is not history-only (Cursor-like) and not "a model under Pi".

**Why:** Users expect the same end-to-end surface as Claude/Codex/Pi. Watch-only or runtime-only would leave either Telegram/WeChat notify or history resume half-broken relative to how people actually use Grok Build.

**Considered options:** history/watch-only first; runtime-only first; optional feature off by default.
