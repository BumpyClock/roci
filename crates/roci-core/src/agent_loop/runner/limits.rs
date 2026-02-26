use std::collections::HashMap;

use super::RunRequest;

const DEFAULT_MAX_ITERATIONS: usize = 20;
const DEFAULT_MAX_TOOL_FAILURES: usize = 8;
const DEFAULT_ITERATION_EXTENSION: usize = 20;
const DEFAULT_MAX_ITERATION_EXTENSIONS: usize = 3;
const RUNNER_MAX_ITERATIONS_ENV: &str = "HOMIE_ROCI_RUNNER_MAX_ITERATIONS";
const RUNNER_MAX_TOOL_FAILURES_ENV: &str = "HOMIE_ROCI_RUNNER_MAX_TOOL_FAILURES";
const RUNNER_ITERATION_EXTENSION_ENV: &str = "HOMIE_ROCI_RUNNER_ITERATION_EXTENSION";
const RUNNER_MAX_ITERATION_EXTENSIONS_ENV: &str = "HOMIE_ROCI_RUNNER_MAX_ITERATION_EXTENSIONS";
const RUNNER_MAX_ITERATIONS_KEYS: [&str; 3] = [
    "runner.max_iterations",
    "agent_loop.max_iterations",
    "max_iterations",
];
const RUNNER_MAX_TOOL_FAILURES_KEYS: [&str; 3] = [
    "runner.max_tool_failures",
    "agent_loop.max_tool_failures",
    "max_tool_failures",
];
const RUNNER_ITERATION_EXTENSION_KEYS: [&str; 3] = [
    "runner.iteration_extension",
    "agent_loop.iteration_extension",
    "iteration_extension",
];
const RUNNER_MAX_ITERATION_EXTENSIONS_KEYS: [&str; 3] = [
    "runner.max_iteration_extensions",
    "agent_loop.max_iteration_extensions",
    "max_iteration_extensions",
];
const PARALLEL_SAFE_TOOL_NAMES: [&str; 6] =
    ["read", "ls", "find", "grep", "web_search", "web_fetch"];

#[derive(Debug, Clone, Copy)]
pub(super) struct RunnerLimits {
    pub(super) max_iterations: usize,
    pub(super) max_tool_failures: usize,
    pub(super) iteration_extension: usize,
    pub(super) max_iteration_extensions: usize,
}

impl RunnerLimits {
    pub(super) fn from_request(request: &RunRequest) -> Self {
        Self {
            max_iterations: parse_runner_limit(
                &request.metadata,
                &RUNNER_MAX_ITERATIONS_KEYS,
                RUNNER_MAX_ITERATIONS_ENV,
                DEFAULT_MAX_ITERATIONS,
            ),
            max_tool_failures: parse_runner_limit(
                &request.metadata,
                &RUNNER_MAX_TOOL_FAILURES_KEYS,
                RUNNER_MAX_TOOL_FAILURES_ENV,
                DEFAULT_MAX_TOOL_FAILURES,
            ),
            iteration_extension: parse_runner_limit(
                &request.metadata,
                &RUNNER_ITERATION_EXTENSION_KEYS,
                RUNNER_ITERATION_EXTENSION_ENV,
                DEFAULT_ITERATION_EXTENSION,
            ),
            max_iteration_extensions: parse_runner_limit(
                &request.metadata,
                &RUNNER_MAX_ITERATION_EXTENSIONS_KEYS,
                RUNNER_MAX_ITERATION_EXTENSIONS_ENV,
                DEFAULT_MAX_ITERATION_EXTENSIONS,
            ),
        }
    }
}

fn parse_runner_limit(
    metadata: &HashMap<String, String>,
    keys: &[&str],
    env_key: &str,
    default: usize,
) -> usize {
    for key in keys {
        if let Some(value) = metadata.get(*key) {
            if let Some(parsed) = parse_positive_usize(value) {
                return parsed;
            }
        }
    }
    if let Ok(value) = std::env::var(env_key) {
        if let Some(parsed) = parse_positive_usize(&value) {
            return parsed;
        }
    }
    default
}

fn parse_positive_usize(value: &str) -> Option<usize> {
    let parsed = value.trim().parse::<usize>().ok()?;
    if parsed == 0 {
        None
    } else {
        Some(parsed)
    }
}

pub(super) fn is_parallel_safe_tool(tool_name: &str) -> bool {
    PARALLEL_SAFE_TOOL_NAMES
        .iter()
        .any(|candidate| candidate == &tool_name)
}
