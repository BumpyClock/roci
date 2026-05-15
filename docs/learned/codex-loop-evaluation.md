# Codex Loop Evaluation Decisions

`read_when`: revisiting prompt assembly, planning-tool surface, or compaction strategy (tsq-wag94sf4).

`tsq-wag94sf4` is the Tasque tracker epic for Codex-loop parity evaluation work.
"Closed evaluation decisions" means these questions were answered for current roci scope;
do not reopen them without new provider, host, or product evidence.

Source material: `.ai_agents/codex_agent_loop.md` (now deleted) and the Codex agent loop article.
Implementation tasks were not created unless noted.

---

## tsq-wag94sf4.1 — Prompt itemization

**Question**: Should roci split the merged system prompt into discrete developer/user/environment
items the way the Codex loop does?

**Decision: defer to a typed prompt builder effort; do not split now.**

Current path merges SYSTEM.md + APPEND_SYSTEM.md + AGENTS/CLAUDE context + skills + MCP
instructions into one system prompt string before the provider call. Splitting into
per-item envelopes would only pay off when:

- a `SystemPromptBuilder` / typed prompt struct provides a stable place for itemized fields,
- multiple provider adapters need to map items onto provider-specific message roles, or
- prompt-debug tooling needs to inspect individual parts independently.

None of those conditions currently hold. The merge path is tested, the ordering is
deterministic, and breaking it apart before having a typed abstraction would add surface area
without measurable benefit. Re-evaluate when `SystemPromptBuilder` is introduced.

### AGENTS.override.md / fallback filename support

**Decision: reject AGENTS.override.md as a distinct discovery filename; keep the current
AGENTS.md → CLAUDE.md fallback pair.**

Context discovery in `crates/roci-core/src/resource/context.rs` walks each scope (global
`~/.roci/agent/`, ancestor dirs, cwd) and looks for `AGENTS.md`, falling back to `CLAUDE.md`
when absent. An explicit `AGENTS.override.md` filename was considered for parity with the
Codex loop's override convention.

Reasons to reject:

- Per-scope override is already expressible: a more-specific scope (cwd or nested dir)
  shadows ancestor scopes through ordering, and `APPEND_SYSTEM.md` covers tail-append needs.
  Adding a third filename multiplies the discovery matrix (scopes × filenames) without
  unlocking a new capability.
- The two-name fallback (`AGENTS.md` → `CLAUDE.md`) exists only for ecosystem compatibility
  with Claude-style repos. Introducing `AGENTS.override.md` would invite a parallel
  `CLAUDE.override.md`, doubling the surface again.
- No current host or provider adapter requests an override filename; the Codex parity
  argument alone is not sufficient (see scope guardrail).

Follow-up if this is revisited: design a single explicit override mechanism (e.g., a
config-declared override path or a `SystemPromptBuilder` layer) rather than another
implicit filename. Re-open only when a concrete host need appears.

---

## tsq-wag94sf4.2 — Built-in planning tool

**Question**: Should roci add an `update_plan` built-in tool (parity with Codex `update_plan`)?

**Decision: planning is a runtime projection, not a built-in tool.**

roci already handles planning at the runtime layer:

- `CollaborationMode::Plan` drives structured plan turns via `EnqueueTurnRequest`.
- `AgentRuntimeEventPayload::PlanUpdated` / `AgentRuntimeEventPayload::DiffUpdated` carry plan/diff state
  through the semantic event stream.
- `ThreadSnapshot.plans` and `.diffs` let reconnecting hosts recover the latest plan without
  replaying raw loop events.

A model-callable `update_plan` tool would duplicate this projection and split plan state
between the tool-result ledger and the runtime's own plan channel. The runtime projection
approach keeps plan lifecycle owned by the runtime, not modeled as tool I/O, which avoids
double-tracking and keeps host rendering paths simple.

If a future agent needs to explicitly emit plan updates as tool calls (e.g., for structured
output formats), introduce a thin adapter that routes tool results into the existing
`PlanUpdated` channel rather than bypassing it.

---

## tsq-wag94sf4.5 — Compaction tradeoffs

Numbering note: closure rationale for `tsq-wag94sf4.3` lives in Tasque history for
"Fix shell approval semantics" and its related tracker entry `tsq-1av9jz0z.1`;
closure rationale for `tsq-wag94sf4.4` lives in Tasque history for "Align OpenAI
Responses session semantics with pi-mono" and child tasks under that tracker entry.

**Question**: Should roci adopt provider-specific compaction (e.g. `/responses/compact`)
or improve reasoning/prefix preservation after compaction?

**Decision: keep provider-agnostic compaction; defer reasoning/prefix improvements unless
justified by concrete pain.**

### Why provider-agnostic compaction is the right default

- roci spans many providers. A compaction path that requires `/responses/compact` or similar
  would force per-provider branches in the compaction pipeline and break the model for
  providers without that API.
- The current approach (`CompactionStrategy::Summary` via LLM + `CompactionStrategy::Micro`
  for fine-grained tool-result/image pruning) works across all providers with the same code
  path.
- `session_before_compact` gives hosts a hook to cancel, override, or inspect the compaction
  payload before it executes. Provider-specific behavior can live there without changing core.

### Reasoning / thinking preservation

Reasoning blocks (`thinking` content parts, Anthropic extended thinking) are **not**
preserved as durable transcript items in the current compaction path. The `Micro` strategy
explicitly elides thinking blocks (by design — they are transient provider artifacts).
`Summary` compaction generates a new summary message without inlining the original chain-of-
thought. This is acceptable because:

- Reasoning tokens are not part of the durable conversation context for future turns.
- Persisting thinking blocks across compaction boundaries would bloat context, not compress it.
- Providers that surface reasoning output already expose it via `AgentEvent::ReasoningUpdated`
  and `ReasoningSnapshot`; hosts that need a reasoning audit trail should record those events.

### Prompt-prefix stability

Exact prompt-prefix stability is **not guaranteed** after compaction. Compaction rewrites the
message list, and context hooks / sanitization may further transform the resulting list before
the next provider call. This matches the Codex evaluation finding and is the accepted tradeoff.

If prefix stability becomes a concrete problem (e.g., cached prompt tokens are invalidated on
every turn for a specific provider), the right fix is a targeted prefix-pinning mechanism, not
a full compaction redesign. No such fix is scheduled.

### G1 relationship

Where the concern was "Codex's provider-specific compaction preserves prompt-prefix cache
hits better," G1 (the planned provider-agnostic context-window management layer, if
introduced) would be the correct place to add prefix-aware compaction. The current
`PreparedCompaction` / `CompactionSpan` / `CompactionSuffix` types already carry span
metadata that a prefix-aware strategy could use.
