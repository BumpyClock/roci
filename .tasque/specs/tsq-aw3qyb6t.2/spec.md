# Design extensible message conversion path

## Objective
Design parity with pi-agent `convertToLlm` style extensibility while maintaining roci loop safety.

## Acceptance Criteria
1. API proposal for agent-side message abstraction + conversion before provider requests.
2. Compatibility plan with `transform_context` and tool result persistence hooks.
3. Migration strategy with minimal breakage and clear test plan (unit + runner).
4. Safety/performance constraints documented (no provider role leakage).
