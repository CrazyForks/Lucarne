# Watch deltas read back to JSONL record boundaries

## Context

The session watcher used a fixed 2 KiB tail window when an appended JSONL delta
was larger than the normal watch read size. If that window started in the middle
of a large JSONL record, the watcher treated the leading bytes as a partial
line, dropped them, advanced the baseline to EOF, and lost the provider event.

The same boundary problem exists when a file is first discovered: there is no
previous offset that can prove which completed records are new, but a trailing
partial record must still be preserved so the next append can complete it.

## Decision

Read JSONL deltas from EOF backwards in 2 KiB chunks until the buffer starts at
a semantic record boundary or reaches the previous baseline. Then parse only
complete records and advance the baseline after the parse.

For newly discovered files, read the tail the same way. A completed tail record
may be emitted as the latest watch event, but older records before the tail
boundary are not replayed. If the file ends with an incomplete record, store
that trailing partial in the baseline so the next append continues the same
record instead of losing it.

## Rationale

The root bug was treating a byte window as if it were a JSONL boundary. Expanding
backwards keeps each physical read bounded while making the logical window end
at complete records. It avoids whole-file reparses, avoids text-based duplicate
checks, and lets provider parsers continue to own provider-specific transcript
semantics.
