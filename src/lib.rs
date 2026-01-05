//! Durable Execution SDK for AWS Lambda (Rust).
//!
//! This crate provides an experimental, community-maintained Rust SDK for building
//! durable functions on AWS Lambda.
//! Durable functions automatically checkpoint their state and can resume from
//! exactly where they left off after failures, timeouts, or other interruptions.
//!
//! # Overview
//!
//! Durable execution solves several challenging problems in serverless computing:
//!
//! - **Long-running workflows**: Execute operations that span hours or days without
//!   paying for idle compute time
//! - **Reliable retries**: Automatically retry failed operations with configurable
//!   exponential backoff
//! - **Human-in-the-loop**: Wait for external systems or human approvals
//! - **Replay-safe semantics**: Operations are checkpointed and replayed deterministically,
//!   with configurable step semantics (at-least-once or at-most-once per retry)
//!
//! # Quick Start
//!
//! Add the dependency to your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! lambda-durable-execution-rust = "0.1.0"
//! lambda_runtime = "1"
//! tokio = { version = "1", features = ["full"] }
//! serde = { version = "1.0", features = ["derive"] }
//! ```
//!
//! Create a durable handler:
//!
//! ```rust,no_run
//! use lambda_durable_execution_rust::prelude::*;
//! use lambda_durable_execution_rust::runtime::with_durable_execution_service;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Debug, Serialize, Deserialize)]
//! struct MyEvent {
//!     name: String,
//! }
//!
//! #[derive(Debug, Serialize, Deserialize)]
//! struct MyResponse {
//!     message: String,
//! }
//!
//! async fn my_handler(
//!     event: MyEvent,
//!     ctx: DurableContextHandle,
//! ) -> DurableResult<MyResponse> {
//!     let name = event.name.clone();
//!     // Execute a step with automatic checkpointing
//!     let greeting = ctx.step(
//!         Some("create-greeting"),
//!         move |_step_ctx| {
//!             let name = name.clone();
//!             async move {
//!                 Ok(format!("Hello, {}!", name))
//!             }
//!         },
//!         None,
//!     ).await?;
//!
//!     Ok(MyResponse { message: greeting })
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<(), lambda_runtime::Error> {
//!     let handler = with_durable_execution_service(my_handler, None);
//!     lambda_runtime::run(handler).await
//! }
//! ```
//!
//! # Core Concepts
//!
//! ## Durable Context
//!
//! The [`DurableContextHandle`](context::DurableContextHandle) is your interface to
//! durable operations. It provides methods for:
//!
//! - [`step`](context::DurableContextHandle::step): Execute a function with checkpointing
//! - [`wait`](context::DurableContextHandle::wait): Suspend execution for a duration
//! - [`wait_for_callback`](context::DurableContextHandle::wait_for_callback): Wait for external completion
//! - [`wait_for_condition`](context::DurableContextHandle::wait_for_condition): Poll until a condition is met
//! - [`invoke`](context::DurableContextHandle::invoke): Call another Lambda function
//! - [`parallel`](context::DurableContextHandle::parallel): Execute branches concurrently
//! - [`map`](context::DurableContextHandle::map): Process items with controlled concurrency
//! - [`run_in_child_context`](context::DurableContextHandle::run_in_child_context): Group operations
//!
//! ## Checkpointing
//!
//! Every durable operation is automatically checkpointed. When a Lambda function
//! restarts (due to timeout, failure, or explicit suspension), it replays from
//! the checkpointed state rather than re-executing completed operations.
//!
//! ```rust,no_run
//! # use lambda_durable_execution_rust::prelude::*;
//! # async fn fetch_data() -> Result<String, Box<dyn std::error::Error + Send + Sync>> { Ok("data".to_string()) }
//! async fn example(ctx: DurableContextHandle) -> DurableResult<String> {
//!     // First execution:
//!     let data = ctx
//!         .step(Some("fetch"), |_| async { fetch_data().await }, None)
//!         .await?;
//!     // ^ This executes and checkpoints the result
//!
//!     ctx.wait(Some("delay"), Duration::hours(1)).await?;
//!     // ^ Lambda suspends here, no compute cost
//!
//!     // After 1 hour, Lambda resumes:
//!     let data = ctx
//!         .step(Some("fetch"), |_| async { fetch_data().await }, None)
//!         .await?;
//!     // ^ This returns the checkpointed value, doesn't re-execute
//!     Ok(data)
//! }
//! ```
//!
//! ## Retry Strategies
//!
//! Steps can be configured with retry strategies for handling transient failures:
//!
//! ```rust,no_run
//! # use lambda_durable_execution_rust::prelude::*;
//! use lambda_durable_execution_rust::retry::ExponentialBackoff;
//!
//! async fn example(ctx: DurableContextHandle) -> DurableResult<u32> {
//!     let strategy = ExponentialBackoff::builder()
//!         .max_attempts(5)
//!         .initial_delay(Duration::seconds(1))
//!         .max_delay(Duration::minutes(5))
//!         .build();
//!
//!     let config = StepConfig::<u32>::new().with_retry_strategy(Arc::new(strategy));
//!
//!     ctx.step(Some("risky-operation"), |_| async { Ok(1u32) }, Some(config))
//!         .await
//! }
//! ```
//!
//! ## Callbacks
//!
//! Wait for external systems to complete operations:
//!
//! ```rust,no_run
//! # use lambda_durable_execution_rust::prelude::*;
//! # async fn send_approval_request(_id: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> { Ok(()) }
//! async fn example(ctx: DurableContextHandle) -> DurableResult<String> {
//!     let result: String = ctx
//!         .wait_for_callback(
//!             Some("await-approval"),
//!             |callback_id, _step_ctx| async move {
//!                 // Send callback_id to external system
//!                 send_approval_request(&callback_id).await
//!             },
//!             Some(CallbackConfig::new().with_timeout(Duration::hours(24))),
//!         )
//!         .await?;
//!     Ok(result)
//! }
//! ```
//!
//! The external system completes the callback using the AWS Lambda API.
//!
//! # Module Organization
//!
//! - [`context`]: The main [`DurableContextHandle`](context::DurableContextHandle) and related types
//! - [`checkpoint`]: Internal checkpoint management
//! - [`termination`]: Lambda lifecycle management
//! - [`retry`]: Retry strategies including [`ExponentialBackoff`](retry::ExponentialBackoff)
//! - [`types`]: Configuration types like [`Duration`](types::Duration), [`StepConfig`](types::StepConfig)
//! - [`error`]: Error types including [`DurableError`](error::DurableError)
//! - [`runtime`]: Handler wrapper functions
//!
//! # Error Handling
//!
//! The SDK uses the [`DurableResult<T>`](error::DurableResult) type alias for all
//! operations. Errors are categorized in [`DurableError`](error::DurableError):
//!
//! - [`StepFailed`](error::DurableError::StepFailed): Step exhausted all retries
//! - [`CallbackTimeout`](error::DurableError::CallbackTimeout): Callback wasn't completed in time
//! - [`CheckpointFailed`](error::DurableError::CheckpointFailed): Failed to save checkpoint
//! - [`SerializationFailed`](error::DurableError::SerializationFailed): JSON serialization error
//!
//! # Examples
//!
//! See the `examples/` directory for complete working examples:
//!
//! - `hello_world`: Basic usage with a simple step + durable wait
//! - `step_retry`: Configuring retry behavior
//! - `callback_example`: Waiting for external approvals
//! - `child_context`: Grouping steps under a child context
//! - `parallel`: Concurrent processing patterns
//!
//! # Best Practices
//!
//! 1. **Name your operations**: Use descriptive names for debugging and replay validation
//!
//!    ```rust,no_run
//!    # use lambda_durable_execution_rust::prelude::*;
//!    async fn example(ctx: DurableContextHandle) -> DurableResult<()> {
//!        ctx.step(Some("validate-payment"), |_| async { Ok(()) }, None)
//!            .await?;
//!        Ok(())
//!    }
//!    ```
//!
//! 2. **Keep step functions pure**: Steps should be deterministic for replay correctness
//!
//! 3. **Use appropriate retry strategies**: Configure retries based on the operation type
//!
//! 4. **Handle partial failures**: Use `BatchResult` to handle partial success in parallel ops
//!
//! 5. **Set timeouts on callbacks**: Always configure reasonable timeouts for external waits

#![warn(missing_docs)]
#![warn(rustdoc::missing_crate_level_docs)]

pub mod checkpoint;
pub mod context;
pub mod error;
pub mod retry;
pub mod runtime;
pub mod termination;
pub mod types;

// Include README.md in doctests so its Rust snippets stay in sync.
#[cfg(doctest)]
mod readme_doctest {
    #![doc = include_str!("../README.md")]
}

/// Test utilities and mocks (requires the `testutils` feature or test builds).
#[cfg(any(test, feature = "testutils"))]
pub mod mock {
    pub use crate::types::mock::*;
}

/// Prelude module for convenient imports.
///
/// Import everything you need with a single use statement:
///
/// ```rust,no_run
/// use lambda_durable_execution_rust::prelude::*;
/// ```
///
/// This includes:
/// - [`DurableContextHandle`](crate::context::DurableContextHandle) - Main context for operations
/// - [`DurableResult`](crate::error::DurableResult) - Result type alias
/// - [`DurableError`](crate::error::DurableError) - Error type
/// - [`Duration`](crate::types::Duration) - Time duration type
/// - Configuration types ([`StepConfig`](crate::types::StepConfig), etc.)
/// - Retry strategies
/// - `Arc` for sharing retry strategies
pub mod prelude {
    pub use crate::context::{
        BatchCompletionReason, BatchItem, BatchItemStatus, BatchResult, CallbackHandle,
        DurableContextHandle, DurableContextImpl, DurableContextLogger, StepContext,
    };
    pub use crate::error::{DurableError, DurableResult, ErrorObject};
    pub use crate::retry::{
        presets as retry_presets, ExponentialBackoff, RetryDecision, RetryStrategy,
    };
    pub use crate::types::{
        BatchResultSerdes, CallbackConfig, ChildContextConfig, DurableLogData, DurableLogLevel,
        DurableLogger, Duration, JsonSerdes, MapConfig, ParallelConfig, Serdes, SerdesContext,
        StepConfig, StepSemantics, TracingLogger, WaitConditionConfig, WaitConditionDecision,
    };

    // Re-export Arc for DurableContext usage
    pub use std::sync::Arc;
}

// Re-export proc macros when the macros crate is available
