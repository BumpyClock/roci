# Task Spec: stream_transform tests

## Deliverables
- Add full tests for filter/map/buffer/throttle transforms.
- Validate ordering, flush behavior, and error propagation.

## Acceptance Criteria
- All transforms have deterministic, focused tests.
- Time-sensitive tests are stable in CI.

## Tests
- Unit matrix for each transform.
- Stream error propagation tests.
