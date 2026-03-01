# Goal
Complete provider-request plumbing so capability-bearing fields are propagated and consumed end-to-end.

# Scope
- ProviderRequest fields: api key override, headers, metadata, session/transport, payload callback
- roci-core runtime -> runner -> provider adapters
- initial concrete implementation in OpenAI Responses provider

# Acceptance Criteria
- Request fields are not dropped in core pipeline.
- OpenAI Responses provider consumes relevant fields (`session_id` cache key, transport branch, header merge, metadata/pass-through as applicable).
- API key override path is explicit and tested.
- Unit/integration tests validate field propagation.

# Non-Goals
- Full parity implementation for every provider in same task.
