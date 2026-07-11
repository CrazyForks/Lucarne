# Grok Build Dialogue uses ACP over `grok agent stdio`

Grok Build is integrated as a full Provider (`id = grok`). Live **Dialogue** is driven by spawning `grok agent stdio` and speaking ACP JSON-RPC (initialize, session/new or session/load, session/prompt, session/update streams, permissions), not by headless `grok -p` / streaming-json as the primary path.

**Why:** ACP is the multi-turn, tool-visible, permission-aware integration surface Grok documents for IDEs; headless is single-prompt oriented and weaker for interrupt, resume, and structured intervention. WebSocket serve/relay is a secondary transport we deliberately do not take as Lucarne's main Dialogue path.

**Considered options:** headless streaming-json only; dual primary (ACP + headless); WebSocket agent serve.
