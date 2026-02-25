# Define Agent Runtime State + API Surface

## Goal
Specify and scaffold `Agent` runtime state, method signatures, and invariants for the new runtime.

## Scope
- Define internal state model (`idle/running/aborting` and active run handle ownership).
- Add method signatures + docs for `prompt`, `continue_run`, `steer`, `follow_up`, `abort`, `reset`, `wait_for_idle`.
- Preserve compatibility wrappers for existing `execute/stream` as needed.

## Acceptance Criteria
- API compiles and state transitions are explicit in code.
- Public docs/comments define call ordering and concurrency expectations.
