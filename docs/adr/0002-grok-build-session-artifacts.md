# Grok Build Session meta and transcript are separate artifacts

On-disk Grok Build **Sessions** live under `$GROK_HOME/sessions` or `~/.grok/sessions`, grouped by URL-encoded cwd, one directory per session UUID. Lucarne treats **Session meta** as `summary.json` (id, cwd, title/summary, timestamps, model) and **Session transcript** / watch tail as `updates.jsonl` (ACP `session/update` stream). The public resume contract is the session UUID (**Session ref**), not a host filesystem path.

**Why:** Grok documents `updates.jsonl` as the authoritative conversation log for resume; `summary.json` is the index entry. Splitting meta vs transcript matches how Lucarne already separates listing from body for other Providers, and keeps hot paths from scanning the wrong file. Paths and multi-file layout stay provider-private inside the Grok provider boundary.

**Considered options:** chat_history.jsonl as primary; events.jsonl as primary; absolute path as Session ref; single combined file abstraction in common layers.
