## Context
During design for tsq-r0c1ses7.2/.4/.5 we chose strict JSONL runtime event replay for `events.jsonl`: malformed nonblank committed lines must return a visible error because cursor replay needs exact runtime state.

Pi and Codex also have tolerant product-history loaders that salvage conversation/session history:
- Pi stores message/compaction/label/custom entries in session JSONL and rebuilds active context from the entry tree; malformed lines are skipped/reported rather than making all history unusable.
- Codex has tolerant rollout history loaders that can continue past bad rollout items, while its rollout-trace path is strict for trace/event replay.

## Problem
Roci currently needs strict `AgentRuntimeEventStore` semantics for runtime replay, but later needs a separate tolerant history/repair/import layer so users can recover useful session data if a product history file or runtime log is partially corrupt.

## Design Direction
Keep `events.jsonl` strict. Add a separate product-history layer later, e.g. `SessionHistoryStore` / `ConversationStore`, backed by `history.jsonl` or snapshots. Loader should return `{ items, warnings }` rather than fail all-or-nothing for item-level corruption. Provide repair/export/import APIs or CLI commands that salvage messages, summaries, resources, metadata, and workspace references.

## Acceptance Sketch
- Strict runtime event replay remains unchanged: corrupt committed `events.jsonl` line errors with path + line.
- Tolerant history loader skips malformed product-history records, records warnings with line numbers, and returns salvageable items.
- Repair/import path can export recovered conversation items and resources into a fresh durable session.
- Docs explain distinction: exact runtime event log vs tolerant product history/recovery layer.

## Non-goals
- Do not make `AgentRuntimeEventStore` tolerant.
- Do not silently ignore corruption in runtime replay.
- Do not store durable sessions in project cwd by default.