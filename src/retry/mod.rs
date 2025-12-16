//! Retry strategies for durable operations.
//!
//! This module provides configurable retry strategies for handling transient
//! failures in durable operations.
//!
//! # Available Strategies
//!
//! | Strategy | Description |
//! |----------|-------------|
//! | [`ExponentialBackoff`] | Exponential delay with jitter (recommended) |
//! | [`ConstantDelay`] | Fixed delay between attempts |
//! | [`FixedRetry`] | Fixed number of immediate retries |
//! | [`NoRetry`] | Never retry (fail immediately) |
//!
//! # ExponentialBackoff
//!
//! The [`ExponentialBackoff`] strategy is recommended for most use cases:
//!
//! ```rust,no_run
//! use lambda_durable_execution_rust::retry::{ExponentialBackoff, JitterStrategy};
//! use lambda_durable_execution_rust::types::Duration;
//!
//! let strategy = ExponentialBackoff::builder()
//!     .max_attempts(5)           // Total attempts (including first)
//!     .initial_delay(Duration::seconds(1))  // First retry delay
//!     .max_delay(Duration::minutes(5))      // Cap on delay growth
//!     .backoff_rate(2.0)         // Multiply delay by this each attempt
//!     .jitter(JitterStrategy::Full)         // Randomize delays
//!     .retryable_pattern("timeout")         // Only retry these errors
//!     .build();
//! ```
//!
//! # Jitter Strategies
//!
//! Jitter prevents thundering herd problems when many operations retry:
//!
//! - [`JitterStrategy::None`] - Use exact calculated delay
//! - [`JitterStrategy::Full`] - Random between 0 and calculated delay (default)
//! - [`JitterStrategy::Half`] - Random between half and full delay
//! - [`JitterStrategy::Equal`] - Half delay plus random half
//!
//! # Custom Strategies
//!
//! Implement the [`RetryStrategy`] trait for custom behavior:
//!
//! ```rust,no_run
//! use lambda_durable_execution_rust::retry::{RetryDecision, RetryStrategy};
//! use lambda_durable_execution_rust::types::Duration;
//!
//! #[derive(Debug)]
//! struct MyStrategy {
//!     max_attempts: u32,
//! }
//!
//! impl RetryStrategy for MyStrategy {
//!     fn should_retry(
//!         &self,
//!         error: &(dyn std::error::Error + Send + Sync),
//!         attempts_made: u32,
//!     ) -> RetryDecision {
//!         if attempts_made >= self.max_attempts {
//!             return RetryDecision::no_retry();
//!         }
//!
//!         // Custom logic based on error type
//!         if error.to_string().contains("rate limit") {
//!             RetryDecision::retry_after(Duration::seconds(60))
//!         } else {
//!             RetryDecision::retry_immediately()
//!         }
//!     }
//! }
//! ```
//!
//! # Using with Steps
//!
//! Apply retry strategies to steps via [`StepConfig`](crate::types::StepConfig):
//!
//! ```rust,no_run
//! # use lambda_durable_execution_rust::prelude::*;
//! use std::sync::Arc;
//!
//! async fn example(ctx: DurableContextHandle) -> DurableResult<u32> {
//!     let config = StepConfig::<u32>::new().with_retry_strategy(Arc::new(
//!         ExponentialBackoff::builder().max_attempts(3).build(),
//!     ));
//!
//!     ctx.step(Some("risky-op"), |_| async { Ok(1u32) }, Some(config))
//!         .await
//! }
//! ```
//!
//! # Preset Strategies
//!
//! The [`presets`] module provides common configurations:
//!
//! ```rust,no_run
//! use lambda_durable_execution_rust::retry::presets;
//!
//! let strategy = presets::default_exponential_backoff();  // 3 attempts, 1s initial
//! let strategy = presets::aggressive_retry();             // 10 attempts, 100ms initial
//! let strategy = presets::patient_retry();                // 5 attempts, 5s initial, 5m max
//! ```

pub mod presets;
mod strategy;

pub use strategy::*;
