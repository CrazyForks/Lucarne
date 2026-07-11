# Lucarne

Lucarne is an agent multiplexer: it normalizes multiple AI agent CLIs into one runtime, history surface, and channel notification path.

## Language

### Integration

**Provider**:
A first-class agent product Lucarne integrates end-to-end: session discovery/parse, live multi-turn dialogue, and history watch for notifications. Peers include Claude, Codex, Gemini, Copilot, Pi; Cursor is history/watch-only and is not a full Provider.
_Avoid_: backend, integration target, agent type (as a public term)

**Grok Build**:
xAI's coding agent product (CLI binary `grok`, home under `~/.grok` / `GROK_HOME`). In Lucarne it is integrated as a full **Provider** with stable id `grok` and display name "Grok Build", compiled in by default like other full Providers — not as a model inside another Provider (e.g. Pi's `xai/grok-*` models).
_Avoid_: Grok (bare, ambiguous with models), xAI API, Pi model catalog entry, provider id `grok-build`

**Session**:
One persistent conversation belonging to a **Provider**, with an id and on-disk artifacts Lucarne can discover, parse, resume, and watch.
_Avoid_: chat, thread (unless a channel topic is meant)

**Session ref**:
The provider-native opaque id Lucarne stores to resume a **Session**. For **Grok Build**, this is the session UUID (same id as on-disk session identity), not a host filesystem path.
_Avoid_: absolute path as the public resume contract, thread id (unless that Provider uses it)

**Session meta**:
Identity and listing fields for a **Session** (id, title/summary, cwd, timestamps, model). Distinct from the conversation body.
_Avoid_: header-only dump, index row (unless speaking of storage casually)

**Session transcript**:
The authoritative conversation body of a **Session** (user/assistant turns, tools, thoughts as mapped into Lucarne's common model). **Parse** and watch read this; listing uses **Session meta**.
_Avoid_: raw model prompt dump, internal telemetry log

**Parse**:
Reading a **Provider**'s on-disk **Session meta** and **Session transcript** into Lucarne's common history model (bounded; hot paths must not scan whole large artifacts).
_Avoid_: scrape, import, dump

**Dialogue**:
Live multi-turn interaction Lucarne drives by spawning the **Provider** process and speaking its protocol (prompts, stream, tools, permissions, resume). For **Grok Build**, Dialogue is ACP over `grok agent stdio`, not headless one-shot prompts.
_Avoid_: chat API, headless-only integration as the primary path

**Notification**:
Outbound channel delivery of **Session** outcomes (assistant text, attachments, approvals) after common-layer events exist. Channel-owned; not Provider-specific after projection. For a full **Provider**, this includes both live **Dialogue** outcomes and watched external **Sessions** written by the same product outside Lucarne.
_Avoid_: push, alert, webhook (unless those are the channel mechanism); treating only live or only watch as "notifications done"

**Fixture smoke**:
Deterministic end-to-end tests that drive Lucarne against recorded **Provider** protocol/transcript fixtures (via fakeagent or file fixtures), without requiring a live logged-in **Provider** binary. This is the default bar for "完备冒烟" in CI.
_Avoid_: live-only smoke as the definition of done, manual click-through only

**Provider capability parity**:
A new full **Provider** must cover the same Lucarne capability surface already expected of peers (especially Pi and Codex): probe/open/resume, multi-turn **Dialogue**, tool and reasoning streams, permission/question intervention, interrupt, usage, command/status/model catalog surfaces the peer exposes, **Parse**/history list+transcript, watch **Notification**, and matching **Fixture smoke**. Gaps are allowed only when the product protocol has no equivalent — and each gap must be written down with evidence, not silently dropped.
_Avoid_: "MVP subset", "we'll add later" without a tracked gap list

## Flagged ambiguities

- **Grok vs Grok Build**: "Grok" alone can mean a chat model, an API, or the Build product. Prefer **Grok Build** for the Provider product; reserve bare "Grok" only when the user means the model brand generically.
- **Cursor**: present in `agent-sessions` as history/watch without a live adapter; do not call it a full **Provider** in the same sense as Claude/Codex/Pi unless an adapter ships.

## Example dialogue

> **Dev:** We're adding Grok Build — is that just another model under Pi?
>
> **Expert:** No. **Grok Build** is its own **Provider**. Pi can already call `xai/grok-*` models; that is not Grok Build session **Parse**, **Dialogue**, or **Notification**.
>
> **Dev:** So full Provider means history files plus spawning the CLI?
>
> **Expert:** Yes: **Parse** and watch for **Session** history/**Notification**, and **Dialogue** for live multi-turn. Cursor is the counterexample — history/watch only.
