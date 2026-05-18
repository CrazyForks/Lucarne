# Chunked watch metadata reads for large Codex session_meta

## Context

Codex 0.130 can write `session_meta` JSONL records larger than 2 KiB because
the payload includes fields such as `base_instructions`, `memory_mode`, and
`git`. The watcher used to read a bounded prefix for metadata and then ask the
provider parser for `session_id` and `cwd`. A single 2 KiB prefix can split the
first JSONL record in the middle and produce an EOF parse error.

## Decision

Keep the per-read watch buffer at 2 KiB. For metadata, pass providers a
`BufReader` with a 2 KiB capacity over a `Take<File>` capped at 16 chunks, so
provider metadata parsers can stream until they have the first complete JSONL
record without a larger fixed prefix allocation.

## Rationale

The failure is caused by truncating a valid first JSONL record at exactly the
2 KiB read boundary. A capped streaming reader preserves the hot-path memory
constraint better than a larger fixed prefix allocation, while still avoiding
full-file scans and provider-specific fallback parsing in the common watch
layer.
