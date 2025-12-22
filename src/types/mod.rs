//! Core types for the AWS Durable Execution SDK.
//!
//! This module contains common types used throughout the SDK including
//! configuration types, duration handling, and invocation types.
//!
//! # Duration
//!
//! The [`Duration`] type represents time durations for waits and timeouts:
//!
//! ```rust,no_run
//! use lambda_durable_execution_rust::types::Duration;
//!
//! let seconds = Duration::seconds(30);
//! let minutes = Duration::minutes(5);
//! let hours = Duration::hours(2);
//! let days = Duration::days(1);
//!
//! // Convert back to seconds
//! assert_eq!(minutes.to_seconds(), 300);
//! ```
//!
//! # Configuration Types
//!
//! Various configuration types for durable operations:
//!
//! ## StepConfig
//!
//! Configure step execution:
//!
//! ```rust,no_run
//! use lambda_durable_execution_rust::retry::{RetryDecision, RetryStrategy};
//! use lambda_durable_execution_rust::types::{Duration, StepConfig, StepSemantics};
//! use std::sync::Arc;
//!
//! #[derive(Debug)]
//! struct MyStrategy;
//!
//! impl RetryStrategy for MyStrategy {
//!     fn should_retry(
//!         &self,
//!         _error: &(dyn std::error::Error + Send + Sync),
//!         _attempts_made: u32,
//!     ) -> RetryDecision {
//!         RetryDecision::retry_after(Duration::seconds(1))
//!     }
//! }
//!
//! let config: StepConfig<String> = StepConfig::new()
//!     .with_retry_strategy(Arc::new(MyStrategy))
//!     .with_semantics(StepSemantics::AtLeastOncePerRetry);
//! ```
//!
//! ## CallbackConfig
//!
//! Configure callback operations:
//!
//! ```rust,no_run
//! use lambda_durable_execution_rust::types::{CallbackConfig, Duration};
//!
//! let config: CallbackConfig<String> = CallbackConfig::new().with_timeout(Duration::hours(24));
//! ```
//!
//! ## ParallelConfig and MapConfig
//!
//! Configure parallel execution:
//!
//! ```rust,no_run
//! use lambda_durable_execution_rust::types::{CompletionConfig, MapConfig, ParallelConfig};
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Clone, Serialize, Deserialize)]
//! struct MyItem {
//!     id: u32,
//! }
//!
//! // For fixed branches
//! let parallel_config: ParallelConfig<String> = ParallelConfig::new()
//!     .with_max_concurrency(4)
//!     .with_completion_config(
//!         CompletionConfig::new()
//!             .with_min_successful(2)
//!             .with_tolerated_failures(1),
//!     );
//!
//! // For mapping over items
//! let map_config: MapConfig<MyItem, String> = MapConfig::new()
//!     .with_max_concurrency(10)
//!     .with_completion_config(CompletionConfig::new());
//! ```
//!
//! ## ChildContextConfig
//!
//! Configure child context operations:
//!
//! ```rust,no_run
//! use lambda_durable_execution_rust::types::ChildContextConfig;
//!
//! let config = ChildContextConfig::<u32>::new().with_sub_type("batch-processor");
//! ```
//!
//! # Invocation Types
//!
//! Types for Lambda invocation input/output:
//!
//! - [`DurableExecutionInvocationInput`] - Input from Lambda service
//! - [`DurableExecutionInvocationOutput`] - Output to Lambda service
//! - [`OperationUpdate`] - Checkpoint update for an operation
//!
//! # Step Semantics
//!
//! [`StepSemantics`] controls execution guarantees:
//!
//! - `AtLeastOncePerRetry` (default) - May execute multiple times on interruption/failure
//! - `AtMostOncePerRetry` - Interrupted attempts move to the next retry cycle

mod batch;
mod config;
mod duration;
mod invocation;
mod lambda_service;
mod logger;
mod serdes;

pub use batch::*;
pub use config::*;
pub use duration::*;
pub use invocation::*;
pub use lambda_service::*;
pub use logger::*;
pub use serdes::*;

#[cfg(any(test, feature = "testutils"))]
pub use lambda_service::mock;
