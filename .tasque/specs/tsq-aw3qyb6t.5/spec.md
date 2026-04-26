# Add fine-grained queue management APIs

## Objective
Manage steering/follow-up queues without full runtime reset.

## Acceptance Criteria
1. Add `clear_steering_queue`, `clear_follow_up_queue`, `clear_all_queues`, `has_queued_messages`.
2. Preserve `QueueDrainMode` semantics.
3. Add tests for queue state transitions and interactions with `continue_without_input`.
4. Keep `reset` behavior backward-compatible.
