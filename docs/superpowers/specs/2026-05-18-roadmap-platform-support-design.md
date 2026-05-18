# Roadmap Platform Support Design

## Goal

Add roadmap visibility for future Linux and Windows support.

## Scope

Change only `README.md` roadmap content. Do not change Rust code, packaging files, CI, install scripts, or platform behavior.

## Recommended change

Add two unchecked roadmap items near the release/publishing item:

- Linux support: cover install instructions, service management, release package, and smoke test.
- Windows support: cover install instructions, background execution, path/process compatibility, and release package.

## Rationale

Platform support is release/distribution work, so it belongs near the existing “更完整发布链路” roadmap item. The wording should be specific enough to clarify future work without implying support already exists.

## Testing

No code tests needed. Verify by reading the README diff and confirming the new roadmap items render as Markdown checklist entries.

## Out of scope

- Implementing Linux support.
- Implementing Windows support.
- Changing Homebrew instructions.
- Adding CI jobs or release artifacts.
