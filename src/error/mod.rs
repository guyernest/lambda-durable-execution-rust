//! Error types for the AWS Durable Execution SDK.
//!
//! This module provides error types for all operations in the durable execution
//! SDK, following Rust conventions with the `thiserror` crate.
//!
//! # Error Types
//!
//! The main error type is [`DurableError`], an enum that covers all possible
//! failure modes:
//!
//! | Variant | Description |
//! |---------|-------------|
//! | [`StepFailed`](DurableError::StepFailed) | Step exhausted all retry attempts |
//! | [`CallbackTimeout`](DurableError::CallbackTimeout) | Callback wasn't completed in time |
//! | [`CallbackFailed`](DurableError::CallbackFailed) | External system failed the callback |
//! | [`InvocationFailed`](DurableError::InvocationFailed) | Lambda invocation failed |
//! | [`CheckpointFailed`](DurableError::CheckpointFailed) | Failed to save checkpoint |
//! | [`SerializationFailed`](DurableError::SerializationFailed) | JSON serialization error |
//! | [`ReplayValidationFailed`](DurableError::ReplayValidationFailed) | Non-deterministic behavior detected |
//! | [`ChildContextFailed`](DurableError::ChildContextFailed) | Child context operation failed |
//! | [`BatchOperationFailed`](DurableError::BatchOperationFailed) | Parallel/map didn't meet requirements |
//! | [`WaitConditionExceeded`](DurableError::WaitConditionExceeded) | Wait condition exceeded max attempts |
//! | [`InvalidConfiguration`](DurableError::InvalidConfiguration) | Invalid configuration provided |
//! | [`ContextValidationError`](DurableError::ContextValidationError) | Context used incorrectly |
//! | [`Internal`](DurableError::Internal) | Internal SDK error |
//! | [`AwsSdk`](DurableError::AwsSdk) | AWS SDK error |
//!
//! # Result Type
//!
//! The [`DurableResult<T>`] type alias is used throughout the SDK:
//!
//! ```rust,no_run
//! use lambda_durable_execution_rust::prelude::*;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Deserialize)]
//! struct MyEvent;
//!
//! #[derive(Serialize)]
//! struct Response {
//!     data: String,
//! }
//!
//! async fn fetch() -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
//!     Ok("data".to_string())
//! }
//!
//! async fn my_handler(_event: MyEvent, ctx: DurableContextHandle) -> DurableResult<Response> {
//!     let data = ctx.step(Some("fetch"), |_| async { fetch().await }, None).await?;
//!     Ok(Response { data })
//! }
//! ```
//!
//! # Error Recovery
//!
//! Some errors are recoverable (Lambda will retry), others are not:
//!
//! ```rust,no_run
//! # use lambda_durable_execution_rust::prelude::*;
//! fn example(error: DurableError) {
//!     match error {
//!         DurableError::CheckpointFailed { recoverable: true, .. } => {
//!             // Lambda will automatically retry
//!         }
//!         DurableError::StepFailed { .. } => {
//!             // Step exhausted its own retries, not recoverable at Lambda level
//!         }
//!         DurableError::ReplayValidationFailed { .. } => {
//!             // Non-determinism detected, requires code fix
//!         }
//!         _ => {}
//!     }
//!
//!     // Helper methods:
//!     let _ = error.is_recoverable();
//!     let _ = error.should_terminate_lambda();
//! }
//! ```
//!
//! # Error Serialization
//!
//! The [`ErrorObject`] type provides a serializable representation of errors
//! for checkpoint storage:
//!
//! ```rust,no_run
//! # use lambda_durable_execution_rust::prelude::*;
//! fn example(error: DurableError) -> Result<String, serde_json::Error> {
//!     let error_obj = ErrorObject::from_durable_error(&error);
//!     serde_json::to_string(&error_obj)
//! }
//! ```

mod types;

pub use types::*;
