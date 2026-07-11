# Grok Build ships with fixture smoke and Pi/Codex capability parity

Done means **Fixture smoke** in CI (fakeagent / recorded wire and on-disk fixtures), not live logged-in Grok runs. The capability surface is **Provider capability parity** with Pi and Codex: registration, Dialogue event recognition, commands/control, Parse/history, and live + external-watch **Notification**. The living checklist is `docs/agents/grok-provider-parity.md`. Gaps are allowed only when Grok's protocol has no equivalent, and each gap must be recorded in that file's gap log with evidence.

**Why:** Live E2E as the definition of done is flaky, credential-bound, and slow; peer Providers already prove correctness with scenario fixtures. A written parity checklist prevents long implementation runs from silently dropping command, permission, resume, or watch paths.

**Considered options:** minimal basic-only smoke; live-only CI bar; deliberately smaller MVP without gap tracking.
