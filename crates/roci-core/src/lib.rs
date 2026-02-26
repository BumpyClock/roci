//! Roci Core -- Provider-agnostic SDK kernel
//!
//! This crate contains everything provider-agnostic: traits, types, config,
//! auth orchestration, generation API, agent loop, and shared utilities.
//!
//! Concrete provider implementations live in `roci-providers`.
//! The `roci` meta-crate re-exports both with default wiring.

pub mod auth;
pub mod config;
pub mod error;
pub mod generation;
pub mod models;
pub mod prelude;
pub mod provider;
pub mod stop;
pub mod stream_transform;
pub mod tools;
pub mod types;
pub mod util;

#[cfg(feature = "agent")]
pub mod agent;

#[cfg(feature = "agent")]
pub mod agent_loop;

#[cfg(feature = "audio")]
pub mod audio;

#[cfg(feature = "mcp")]
pub mod mcp;
