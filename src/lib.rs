//! Roci — Rust AI SDK
//!
//! Spiritual port of Tachikoma (Swift) to Rust. Provides a unified interface
//! for multiple AI providers with support for text generation, streaming,
//! structured output, tool calling, and agents.
//!
//! # Quick Start
//!
//! ```no_run
//! use roci::prelude::*;
//! use roci::models::LanguageModel;
//!
//! # async fn example() -> roci::error::Result<()> {
//! let model: LanguageModel = "openai:gpt-4o".parse()?;
//! let response = roci::generation::generate(&model, "Hello!").await?;
//! println!("{response}");
//! # Ok(())
//! # }
//! ```

pub mod config;
pub mod auth;
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

#[cfg(feature = "cli")]
pub mod cli;
