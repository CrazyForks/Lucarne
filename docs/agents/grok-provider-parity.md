# Grok Build (`grok`) Provider — capability parity checklist

Definition of done for the full **Provider** `grok` (display name **Grok Build**).
Bar: **Provider capability parity** with **Pi** and **Codex** (see root `CONTEXT.md`).
Tests: **Fixture smoke** by default (fakeagent / recorded wire + on-disk fixtures). Live tests optional, not the CI bar.

**Status (2026-07-11):** Shipped in `14f98e1`. All **Must** rows are **Done** or listed in **§H** with evidence. Issue #39 closed against this checklist.

Legend:

- **Must** — Lucarne surface that peers implement; ship unless Grok protocol has no equivalent (gap + evidence required).
- **Peer-only** — exists for Pi or Codex because of that product's protocol; map if Grok has it, else document N/A.
- **Skip** — explicitly out of Lucarne scope for this integration (product choice).

Technical locks (implementation agents must not re-litigate without ADR):

| Topic | Lock |
|-------|------|
| Provider id | `grok` |
| Display name | Grok Build |
| Default feature | on (with other full Providers) |
| Dialogue transport | `grok agent stdio` (ACP JSON-RPC) |
| Session ref | session UUID |
| Session meta | `summary.json` (under `$GROK_HOME` / `~/.grok` sessions tree) |
| Session transcript / watch | `updates.jsonl` (ACP `session/update` stream) |
| Discovery root | `$GROK_HOME/sessions` or `~/.grok/sessions` (respect `GROK_HOME`) |
| Notifications | live Dialogue **and** external Session watch |
| Binary | `grok`; honor `LUCARNE_GROK_BIN` if present; prefer `~/.grok/bin/grok` when exists |

---

## A. Registration & runtime shell

| # | Capability | Peer anchor | Status |
|---|------------|-------------|--------|
| A1 | Cargo feature `grok` + default features | pi/codex/claude | **Done** |
| A2 | `agent-sessions` feature `grok` + descriptor in `agent_providers()` | all history providers | **Done** |
| A3 | `linkme` `AgentDescriptor` + adapter factory | pi/codex | **Done** |
| A4 | Opaque `ProviderId::from_static("grok")` only at boundary | AGENTS.md | **Done** |
| A5 | Probe (binary available + version when available) | all | **Done** |
| A6 | Open new session (cwd, model, system prompt if supported) | pi/codex | **Done** |
| A7 | Resume via **Session ref** (UUID → ACP `session/load`) | pi/codex | **Done** |
| A8 | Capabilities flags truthful | Spec/Capabilities | **Done** |
| A9 | Arg profile / blocked extra-args for flags Lucarne owns | pi/codex | **Done** |
| A10 | Config schema fields (binary, model, …) | peers | **Done** |

## B. Dialogue event recognition (ACP → Lucarne events)

| # | Capability | Peer / Grok wire | Status |
|---|------------|------------------|--------|
| B1 | User prompt encode (`session/prompt`) | ACP | **Done** |
| B2 | Assistant text stream (`agent_message_chunk`) | Pi/Codex message | **Done** |
| B3 | Reasoning / thought stream (`agent_thought_chunk`) | Pi/Codex thinking | **Done** |
| B4 | Tool call start (`tool_call`) | peers | **Done** |
| B5 | Tool call update / result (`tool_call_update`) | peers | **Done** |
| B6 | Turn completed (`turn_completed` / stop reason) | peers | **Done** |
| B7 | Turn failed / error surfaces | pi error, codex abort | **Done** |
| B8 | Multi-turn same process | peers | **Done** |
| B9 | Usage / usage delta when protocol provides | pi/codex | **Done** (turn usage); mid-stream UsageDelta → **§H** |
| B10 | SessionStarted with native session id | peers | **Done** |
| B11 | SessionClosed + ResumeHandle (UUID) | peers | **Done** |
| B12 | Permission request + response round-trip | pi permission_intercept; codex approvals | **Done** |
| B13 | User question / multi-option intervention if Grok emits | codex question_*, claude ask_user | **Done** — `_x.ai/ask_user_question` |
| B14 | Interrupt / cancel in-flight turn | pi/codex interrupt | **Done** |
| B15 | Interrupt recovery (next turn succeeds) | codex interrupt_recovery | **Done** — `interrupt_recovery.fixture` + TurnFailed(`cancelled`) then second turn |
| B16 | Image / multimodal user input if Grok ACP accepts | live image tests peers | **Done** (encode + live recording); ACP image unit fixture optional |
| B17 | Attachment / media outbound if Grok emits | watch attachment path | **§H** — N/A |
| B18 | Foreign session noise ignored (wrong sessionId) | codex foreign_thread | **Done** |
| B19 | Initialize handshake (`initialize` + capabilities) | ACP | **Done** |
| B20 | `session/new` with cwd | ACP | **Done** |

## C. Commands / control surface (Dialect)

| # | Capability | Peer anchor | Status |
|---|------------|-------------|--------|
| C1 | Command catalog (ProviderNative from `available_commands_update`) | pi get_commands, gemini catalog | **Done** — AdapterMapped excluded |
| C2 | Status | pi/codex status | **Done** — model/cwd/session/permissions/version |
| C3 | Set model / model catalog | pi/codex/gemini | **Done** — list from session models + `_x.ai/models/update`; set via `session/set_model` (slash fallback) |
| C4 | Set permissions / permission catalog | pi/codex | **Done** — default / always-approve / auto |
| C5 | Fork session | pi fork; x.ai/session/fork | **Done** — list targets + fork RPC/slash |
| C6 | New session / new thread | pi new_session | **Done** — `session/new` |
| C7 | Skills list | codex list_skills | **Done** — from availableCommands with SKILL.md meta |
| C8 | Confirm/deny structured intervention | pi confirm_deny | **Done** — permission path |
| C9 | Auto-retry / retry exhausted mapping | pi auto_retry | **§H** — N/A |
| C10 | list_commands / quit | peers | **Done** |
| C11 | ProviderNative dispatch (e.g. compact) | gemini slash | **Done** — `/name` via session/prompt |

## D. Parse / history (agent-sessions + lucarne history)

| # | Capability | Peer anchor | Status |
|---|------------|-------------|--------|
| D1 | Discover sessions under default roots | all | **Done** |
| D2 | Respect `GROK_HOME` for roots | Grok sessions doc | **Done** |
| D3 | Session meta: id, cwd, title/summary, mtime, model | summary.json | **Done** |
| D4 | Transcript parse from updates.jsonl → semantic messages | pi/codex parse fixtures | **Done** |
| D5 | User / assistant visible text | peers | **Done** |
| D6 | Tool calls in history projection where peers show them | peers | **Done** |
| D7 | Reasoning blocks if stored in updates | peers | **Done** |
| D8 | Bounded / selection-aware parse (meta-only, messages) | ParseSelection | **Done** |
| D9 | Visible transcript user offsets / visibility filter | descriptor | **Done** |
| D10 | History list + recent ranking works via descriptor only | journey_53/58 style | **Done** |
| D11 | No concrete `Grok` type in `lucarne::history` | AGENTS.md | **Done** |
| D12 | Primary source path conventions stay provider-private | multi-file session dir | **Done** |
| D13 | Subagent sessions: include/exclude policy explicit | cursor subagents; grok subagents/ | **Done** — exclude paths under `subagents/` |
| D14 | Session file format: LineDelimitedJson on updates.jsonl | most providers | **Done** |

## E. Watch → Notification

| # | Capability | Peer anchor | Status |
|---|------------|-------------|--------|
| E1 | Watch session transcript growth (updates.jsonl) | watch provider | **Done** |
| E2 | Incremental append (no whole-file reparse) | supports_incremental_watch_events | **Done** — true for grok |
| E3 | Dedupe / normalize watch events | codex/pi watch | **Done** — terminal response synthesize |
| E4 | Seed watch state when required | peers | **Done** — needs_watch_state_seed |
| E5 | Project final assistant text to core events | notification path | **Done** — core history watch |
| E6 | Live Dialogue notifications (runtime bus → channel) | telegram policy | **Done** — dialogue bus |
| E7 | External Session watch notifications | idle live history watch | **Done** — SessionWatcher + core |
| E8 | Attachment notifications if attachments exist | watch-attachment decision | **§H** — N/A (same as B17) |
| E9 | Permission/approval notify when user action required | telegram outbound policy | **Done** — common intervention path |
| E10 | Recent-session discovery depth/roots for watch targets | sessions/`<enc-cwd>`/`<uuid>` depth 3 | **Done** |

### E live / fixture coverage

| Test | Location |
|------|----------|
| parse_watch_reader full fixture | `agent-sessions` unit (`provider.rs`) |
| delta-only window | `agent-sessions` unit |
| append / nested / large baseline / created | `agent-sessions/tests/watch_live.rs` |
| core history watch existing + new session | `lucarne::core_service` (macos FSEvents) |

## F. Fixture smoke matrix (CI)

Mirror peer scenario *roles* (names may differ). Each row needs at least one fixture test.

### F1 Dialogue / dialect (like `pi_scenarios` + relevant `codex_scenarios`)

| # | Scenario role | Status | Coverage |
|---|---------------|--------|----------|
| F1.01 | basic conversation | **Done** | `basic.fixture` + `grok_scenarios::basic` |
| F1.02 | tool call + tool result | **Done** | `tool_fs.fixture` + reverse `fs/*` unit |
| F1.03 | multi-turn | **Done** | `multi_turn.fixture` |
| F1.04 | resume propagates session ref | **Done** | `resume.fixture` |
| F1.05 | resume missing / fallback | **Done** | `resume_load_error` (load error → close); `resume_empty_session_id` (keep UUID); no Codex thread/start fallback |
| F1.06 | permission allow | **Done** | `permission.fixture` |
| F1.07 | permission deny | **Done** | live reject + permission path; dialect confirm/deny |
| F1.08 | interrupt | **Done** | `interrupt.fixture` |
| F1.09 | interrupt recovery | **Done** | `interrupt_recovery.fixture` |
| F1.10 | error → TurnFailed | **Done** | `error.fixture` |
| F1.11 | usage when present | **§H** | code maps prompt-result usage; fixtures omit usage payload |
| F1.12 | model catalog / set model | **Done** | dialect unit + live command round-trip |
| F1.13 | status / commands | **Done** | `commands.fixture` + catalog unit |
| F1.14 | fork | **Done** | dialect unit + live command (incl. fork with args) |
| F1.15 | new session | **Done** | dialect unit + live command |
| F1.16 | confirm/deny intervention | **Done** | permission allow/deny path |
| F1.17 | question / multi-option | **Done** | `ask_user_question.fixture` |
| F1.18 | foreign session ignored | **Done** | dialect unit `foreign_session_update_ignored` |
| F1.19 | start missing id closes cleanly | **Done** | `start_missing_id.fixture` |
| F1.20 | reasoning stream | **Done** | `agent_thought_chunk` mapping |

### F2 Parse / agent-sessions

| # | Scenario role | Status |
|---|---------------|--------|
| F2.01 | Fixture `updates.jsonl` (+ sibling `summary.json`) → semantic session | **Done** |
| F2.02 | Meta-only selection does not require full transcript | **Done** |
| F2.03 | Discovery finds real layout under temp GROK_HOME | **Done** |
| F2.04 | Title/cwd/id from summary | **Done** |
| F2.05 | Watch parse produces incremental-friendly events | **Done** |

### F3 History + notification projection

| # | Scenario role | Status |
|---|---------------|--------|
| F3.01 | History list includes `grok` via descriptor | **Done** |
| F3.02 | Transcript window / cursor behavior for jsonl | **Done** — LineDelimitedJson + history path |
| F3.03 | Watch events project to assistant notification payload (common layer) | **Done** — core projection; live Telegram send not required for fixture bar |
| F3.04 | Journey-style: descriptor registers runtime + history | **Done** — journey + history assert `grok` |

### F4 Live (optional for CI; recorded under `tests/data/live_recordings/**/grok/`)

Re-record with `LUCARNE_LIVE_E2E=1 LUCARNE_LIVE_RERECORD=1 LUCARNE_LIVE_PROVIDERS=grok`.
Replay without live flags uses fixtures only.

| # | Scenario | Suite | Status |
|---|----------|-------|--------|
| F4.01 | basic conversation | `live_e2e` + `agent_runtime_live_e2e` | **Recorded** |
| F4.02 | multi-turn | `agent_runtime_live_e2e` | **Recorded** |
| F4.03 | resume (seed + load UUID) | `agent_runtime_live_e2e` | **Recorded** |
| F4.04 | tool write flow (ACP reverse `fs/*`) | both | **Recorded** |
| F4.05 | tool failure | both | **Recorded** |
| F4.06 | interrupt | both | **Recorded** |
| F4.07 | command round-trip (`/status` + peers) | `agent_runtime_live_e2e` | **Recorded** |
| F4.08 | approval / reject | `agent_runtime_live_e2e` (+ `live_e2e` reject) | **Recorded** |
| F4.09 | delete via shell tool | `live_e2e` | **Recorded** |
| F4.10 | image input | `agent_runtime_live_e2e` | **Recorded** |
| F4.11 | watch append / nested / large delta / created | `agent-sessions` `watch_live` | **Done** |
| F4.12 | core history watch → TimelineEvent | `lucarne` core_service (macos) | **Done** |

**Note:** Lucarne is **relay-only** for Grok ACP (same posture as Gemini):
`clientCapabilities.fs` / `terminal` are advertised **false**. Grok owns shell
and file tools in-process; Lucarne does **not** execute `fs/*` or `terminal/*`
reverse RPCs (only `session/request_permission` is answered).

**Note:** external session watch is incremental (`supports_incremental_watch_events`),
seeded from `summary.json` + bounded lookback, with depth-3 roots under
`sessions/<encoded-cwd>/<uuid>/`.

---

## G. Explicit non-goals (still list so they are not forgotten)

| # | Item | Why |
|---|------|-----|
| G1 | Treating Pi `xai/grok-*` model ids as this Provider | Different product surface |
| G2 | Grok WebSocket `agent serve` / relay as primary Dialogue | stdio ACP is primary |
| G3 | Headless `-p` as primary Dialogue | multi-turn/permissions weaker |
| G4 | Implementing Grok's full x.ai/* IDE extension surface in Lucarne | only what maps to Lucarne commands/events |
| G5 | Manual GitHub Release / process changes | unrelated |
| G6 | Hosting ACP client environment (`fs/*`, `terminal/*`) | Lucarned is channel/session relay, not an IDE workspace host |

---

## H. Gap log (remaining N/A or deferred; not blocking DoD)

| Capability id | Why N/A or deferred | Evidence (doc/fixture/probe) |
|---------------|---------------------|------------------------------|
| ~~B13 / F1.17 multi-option question UI~~ | **Done** — live dialogue: reverse RPC `_x.ai/ask_user_question`; history watch: notify-only projection of `tool_call` ask_user_question as assistant text (no answer path on disk) | `ask_user_question.fixture` + grok parse/watch unit tests |
| Watch large agent_message line drop | **Done** — `read_delta` no early-stop; Grok unix ts → RFC3339; `TurnCompleted.last_agent_message` | `grok_oversized_agent_message_line_reaches_watch_events` + timestamp/turn_completed unit tests |
| B16 ACP image unit fixture | Dialect encodes image blocks; live recording exists; optional fakeagent image fixture not added | `encode_user_message` image branch + `agent_runtime_live_image_input_grok` |
| B17 / E8 attachment outbound | No provider-native attachment/media events in Grok `updates.jsonl` samples | Sample sessions under `~/.grok/sessions` |
| B9 usage mid-stream | Prompt result may carry usage → `TurnCompleted.usage`; partial `UsageDelta` not observed | `SessionPrompt` result handling; basic/error fixtures omit usage |
| C9 auto-retry mapping | No auto-retry `sessionUpdate` observed | Disk samples |
| F1.11 usage fixture payload | Code path present; fixtures do not include usage-bearing prompt results | `error`/`basic` fixtures |
| Codex-style resume→fresh-start fallback | Grok ACP: `session/load` error closes; empty load result keeps requested UUID. No automatic `session/new` after failed load (unlike Codex thread/resume→thread/start) | `resume_load_error.fixture`, `resume_empty_session_id.fixture` |
| Channel bot live e2e (Telegram/WeChat × Grok) | Notifications use channel-agnostic core events; no provider-specific bot e2e required for fixture bar | Telegram/WeChat consume common timeline |

---

## I. Definition of done

1. All **Must** rows done or moved to **H** with evidence. ✅
2. All **F1–F3** rows have green **Fixture smoke** (or H entry). ✅
3. No provider-specific branching in `lucarne::history` / public layers (AGENTS.md). ✅
4. `CONTEXT.md` language used in issues/PRs for this work. ✅

Shipped: commit `14f98e1` — Grok Build full provider (ACP dialogue, parse/watch, command parity, fixtures + live recordings).
