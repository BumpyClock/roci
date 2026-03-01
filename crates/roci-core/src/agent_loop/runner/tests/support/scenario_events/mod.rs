use super::ProviderScenario;
use crate::error::RociError;
use crate::types::TextStreamDelta;

mod basic;
mod schema;
mod tooling;

pub(super) fn events_for_scenario(
    scenario: ProviderScenario,
    call_index: usize,
) -> Result<Vec<Result<TextStreamDelta, RociError>>, RociError> {
    match scenario {
        ProviderScenario::MissingOptionalFields
        | ProviderScenario::TextThenStreamError
        | ProviderScenario::RepeatedToolFailure
        | ProviderScenario::RateLimitedThenComplete
        | ProviderScenario::RateLimitedExceedsCap
        | ProviderScenario::RateLimitedWithoutRetryHint
        | ProviderScenario::RetryableTimeoutThenComplete
        | ProviderScenario::RetryableTimeoutExhausted
        | ProviderScenario::ContextOverflowThenComplete
        | ProviderScenario::ContextOverflowAlways
        | ProviderScenario::UntypedOverflowError => {
            basic::events_for_scenario(scenario, call_index)
        }
        ProviderScenario::ParallelSafeBatchThenComplete
        | ProviderScenario::MutatingBatchThenComplete
        | ProviderScenario::MixedTextAndParallelBatchThenComplete
        | ProviderScenario::DuplicateToolCallDeltaThenComplete
        | ProviderScenario::StreamEndsWithoutDoneThenComplete
        | ProviderScenario::ToolUpdateThenComplete => {
            tooling::events_for_scenario(scenario, call_index)
        }
        ProviderScenario::SchemaToolBadArgs
        | ProviderScenario::SchemaToolValidArgs
        | ProviderScenario::SchemaToolTypeMismatch => {
            schema::events_for_scenario(scenario, call_index)
        }
        ProviderScenario::PartialTextThenIdle => Err(RociError::InvalidState(
            "PartialTextThenIdle is generated directly by the stub stream".to_string(),
        )),
    }
}
