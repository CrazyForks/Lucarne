# Grok Build (`grok`) Provider — capability parity checklist

Definition of done for the full **Provider** `grok` (display name **Grok Build**).
Bar: **Provider capability parity** with **Pi** and **Codex** (see root `CONTEXT.md`).
Tests: **Fixture smoke** by default (fakeagent / recorded wire + on-disk fixtures). Live tests optional, not the CI bar.

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
| A1 | Cargo feature `grok` + default features | pi/codex/claude | Must |
| A2 | `agent-sessions` feature `grok` + descriptor in `agent_providers()` | all history providers | Must |
| A3 | `linkme` `AgentDescriptor` + adapter factory | pi/codex | Must |
| A4 | Opaque `ProviderId::from_static("grok")` only at boundary | AGENTS.md | Must |
| A5 | Probe (binary available + version when available) | all | Must |
| A6 | Open new session (cwd, model, system prompt if supported) | pi/codex | Must |
| A7 | Resume via **Session ref** (UUID → ACP `session/load` if capability; else documented gap + best path) | pi/codex | Must |
| A8 | Capabilities flags truthful (resume, multi_turn, thinking, tool_stream, usage, structured_intervention, command_catalog, permission_intercept) | Spec/Capabilities | Must |
| A9 | Arg profile / blocked extra-args for flags Lucarne owns | pi/codex | Must |
| A10 | Config schema fields (binary, model, …) | peers | Must |

## B. Dialogue event recognition (ACP → Lucarne events)

| # | Capability | Peer / Grok wire | Status |
|---|------------|------------------|--------|
| B1 | User prompt encode (`session/prompt`) | ACP | Must |
| B2 | Assistant text stream (`agent_message_chunk`) | Pi/Codex message | Must |
| B3 | Reasoning / thought stream (`agent_thought_chunk`) | Pi/Codex thinking | Must |
| B4 | Tool call start (`tool_call`) | peers | Must |
| B5 | Tool call update / result (`tool_call_update`) | peers | Must |
| B6 | Turn completed (`turn_completed` / stop reason) | peers | Must |
| B7 | Turn failed / error surfaces | pi error, codex abort | Must |
| B8 | Multi-turn same process | peers | Must |
| B9 | Usage / usage delta when protocol provides | pi/codex | Must if present in Grok; else gap |
| B10 | SessionStarted with native session id | peers | Must |
| B11 | SessionClosed + ResumeHandle (UUID) | peers | Must |
| B12 | Permission request + response round-trip | pi permission_intercept; codex approvals | Must |
| B13 | User question / multi-option intervention if Grok emits | codex question_*, claude ask_user | Must if present; else gap |
| B14 | Interrupt / cancel in-flight turn | pi/codex interrupt | Must |
| B15 | Interrupt recovery (next turn succeeds) | codex interrupt_recovery | Must |
| B16 | Image / multimodal user input if Grok ACP accepts | live image tests peers | Must if protocol supports; else gap |
| B17 | Attachment / media outbound if Grok emits | watch attachment path | Must if present; else gap |
| B18 | Foreign session noise ignored (wrong sessionId) | codex foreign_thread | Must |
| B19 | Initialize handshake (`initialize` + capabilities) | ACP | Must |
| B20 | `session/new` with cwd | ACP | Must |

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
| C9 | Auto-retry / retry exhausted mapping | pi auto_retry | gap if Grok does not signal retry |
| C10 | list_commands / quit | peers | **Done** |
| C11 | ProviderNative dispatch (e.g. compact) | gemini slash | **Done** — `/name` via session/prompt |

## D. Parse / history (agent-sessions + lucarne history)

| # | Capability | Peer anchor | Status |
|---|------------|-------------|--------|
| D1 | Discover sessions under default roots | all | Must |
| D2 | Respect `GROK_HOME` for roots | Grok sessions doc | Must |
| D3 | Session meta: id, cwd, title/summary, mtime, model | summary.json | Must |
| D4 | Transcript parse from updates.jsonl → semantic messages | pi/codex parse fixtures | Must |
| D5 | User / assistant visible text | peers | Must |
| D6 | Tool calls in history projection where peers show them | peers | Must |
| D7 | Reasoning blocks if stored in updates | peers | Must |
| D8 | Bounded / selection-aware parse (meta-only, messages) | ParseSelection | Must |
| D9 | Visible transcript user offsets / visibility filter | descriptor | Must |
| D10 | History list + recent ranking works via descriptor only | journey_53/58 style | Must |
| D11 | No concrete `Grok` type in `lucarne::history` | AGENTS.md | Must |
| D12 | Primary source path conventions stay provider-private | multi-file session dir | Must |
| D13 | Subagent sessions: include/exclude policy explicit | cursor subagents; grok subagents/ | Document + implement policy |
| D14 | Session file format: LineDelimitedJson on updates.jsonl | most providers | Must |

## E. Watch → Notification

| # | Capability | Peer anchor | Status |
|---|------------|-------------|--------|
| E1 | Watch session transcript growth (updates.jsonl) | watch provider | Must — implemented |
| E2 | Incremental append (no whole-file reparse) | supports_incremental_watch_events | Must — true for grok |
| E3 | Dedupe / normalize watch events | codex/pi watch | Must — terminal response synthesize |
| E4 | Seed watch state when required | peers | Must — needs_watch_state_seed |
| E5 | Project final assistant text to core events | notification path | Must — core history watch |
| E6 | Live Dialogue notifications (runtime bus → channel) | telegram policy | Must — dialogue bus |
| E7 | External Session watch notifications | idle live history watch | Must — SessionWatcher + core |
| E8 | Attachment notifications if attachments exist | watch-attachment decision | Must if applicable |
| E9 | Permission/approval notify when user action required | telegram outbound policy | Must |
| E10 | Recent-session discovery depth/roots for watch targets | sessions/`<enc-cwd>`/`<uuid>` depth 3 | Must |

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

| # | Scenario role | Pi / Codex anchors |
|---|---------------|--------------------|
| F1.01 | basic conversation | pi/codex `basic` |
| F1.02 | tool call + tool result | pi `tool_flow` / `tool_results` |
| F1.03 | multi-turn | pi `multi_turn_flow` |
| F1.04 | resume propagates session ref | pi `resume_*`; codex `resume_success` |
| F1.05 | resume missing / fallback | codex `resume_missing_*`, `resume_fallback_*` |
| F1.06 | permission allow | pi `permission_select`; codex `permission_allow` |
| F1.07 | permission deny | codex deny/decline fixtures |
| F1.08 | interrupt | pi/codex interrupt |
| F1.09 | interrupt recovery | codex `interrupt_recovery` |
| F1.10 | error → TurnFailed | pi `error`, codex abort |
| F1.11 | usage when present | codex `file_change_usage`; pi usage in basic |
| F1.12 | model catalog / set model | pi `model_catalog`, `set_model` |
| F1.13 | status / commands | pi `status_flow`, `get_commands`; codex commands |
| F1.14 | fork | pi `fork_*`; codex fork |
| F1.15 | new session | pi `new_session_flow`; codex `new_thread` |
| F1.16 | confirm/deny intervention | pi `confirm_deny_flow` |
| F1.17 | question / multi-option | codex `question_*` (if Grok has equivalent) |
| F1.18 | foreign session ignored | codex `foreign_thread_*` |
| F1.19 | start missing id closes cleanly | codex `start_missing_id` |
| F1.20 | reasoning stream | pi reasoning in basic; codex thinking |

### F2 Parse / agent-sessions

| # | Scenario role |
|---|---------------|
| F2.01 | Fixture `updates.jsonl` (+ sibling `summary.json`) → semantic session |
| F2.02 | Meta-only selection does not require full transcript |
| F2.03 | Discovery finds real layout under temp GROK_HOME |
| F2.04 | Title/cwd/id from summary |
| F2.05 | Watch parse produces incremental-friendly events |

### F3 History + notification projection

| # | Scenario role |
|---|---------------|
| F3.01 | History list includes `grok` via descriptor |
| F3.02 | Transcript window / cursor behavior for jsonl |
| F3.03 | Watch events project to assistant notification payload (common layer) |
| F3.04 | Journey-style: descriptor registers runtime + history (extend journey_53/58 expectations for `grok`) |

### F4 Live (optional for CI; recorded under `tests/data/live_recordings/**/grok/`)

Re-record with `LUCARNE_LIVE_E2E=1 LUCARNE_LIVE_RERECORD=1 LUCARNE_LIVE_PROVIDERS=grok`.
Replay without live flags uses fixtures only.

| # | Scenario | Suite |
|---|----------|-------|
| F4.01 | basic conversation | `live_e2e` + `agent_runtime_live_e2e` |
| F4.02 | multi-turn | `agent_runtime_live_e2e` |
| F4.03 | resume (seed + load UUID) | `agent_runtime_live_e2e` |
| F4.04 | tool write flow (ACP reverse `fs/*`) | both |
| F4.05 | tool failure | both |
| F4.06 | interrupt | both |
| F4.07 | command round-trip (`/status`) | `agent_runtime_live_e2e` |
| F4.08 | approval / reject | `agent_runtime_live_e2e` (+ `live_e2e` reject) |
| F4.09 | delete via shell tool | `live_e2e` |
| F4.10 | image input | `agent_runtime_live_e2e` |
| F4.11 | watch append / nested / large delta / created | `agent-sessions` `watch_live` |
| F4.12 | core history watch → TimelineEvent | `lucarne` core_service (macos) |

**Note:** tool flows require dialect handling of reverse `fs/read_text_file` and
`fs/write_text_file` (client capabilities advertised at `initialize`). Covered by
unit tests, `tool_fs.fixture` smoke, and live recordings.

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

---

## H. Gap log (fill during implementation)

| Capability id | Why N/A or deferred | Evidence (doc/fixture/probe) |
|---------------|---------------------|------------------------------|
| B13 multi-option question UI | Grok ACP stream uses `session/request_permission` for tool approval; no separate multi-option "question" notification observed in `updates.jsonl` / ACP docs beyond permission options | Real session updates sample + `15-agent-mode.md` sessionUpdate table |
| B16 multimodal image input | Dialect encodes image prompt blocks when runtime supplies images; no dedicated fixture yet (peer live image tests optional) | `GrokAcp::encode_user_message` image branch; no ACP image fixture recorded |
| B17 attachment outbound | No provider-native attachment/media events observed in Grok `updates.jsonl` sample set | Sample session under `~/.grok/sessions` (tool_call/tool_call_update/agent_message only) |
| B9 usage mid-stream | Grok turn_completed / prompt result may carry usage; partial UsageDelta not observed in disk samples; TurnCompleted.usage mapped when present on prompt result | Fixture `error`/`basic` omit usage; code path in `SessionPrompt` result handling |

| C9 auto-retry mapping | No auto-retry sessionUpdate observed | Disk samples |
| F1.05 resume missing/fallback | Resume uses `session/load`; missing id closes via empty sessionId path — dedicated fallback fixtures not ported from Codex thread resume quirks | `session/load` + empty sessionId → SessionClosed |
| F1.09 interrupt recovery | Interrupt cancels turn; second-turn recovery not fixture-covered (interrupt fixture only) | `interrupt.fixture` |
| F1.11–F1.17 peer command density | status/new system commands shipped; model/fork/skills/confirm specialty deferred per above gaps | `command_catalog` + system handlers |
| F3.03 watch→channel Telegram send | Channel-agnostic watch events produced; live Telegram send not required for fixture bar | Watch normalize + parse tests |

---

## I. Definition of done

1. All **Must** rows done or moved to **H** with evidence.
2. All **F1–F3** rows have green **Fixture smoke** (or H entry).
3. No provider-specific branching in `lucarne::history` / public layers (AGENTS.md).
4. `CONTEXT.md` language used in issues/PRs for this work.
