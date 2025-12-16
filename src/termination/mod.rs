//! Termination management for durable executions.
//!
//! This module provides the [`TerminationManager`] which controls when a Lambda
//! function should suspend execution and return control to the service.
//!
//! # Overview
//!
//! Durable executions can be terminated (suspended) for several reasons:
//!
//! - **Wait**: Waiting for a time duration to elapse
//! - **Callback**: Waiting for an external system to complete a callback
//! - **Retry**: An operation failed and needs to be retried later
//!
//! When termination is triggered, the Lambda function completes its current
//! checkpoints and returns. The Lambda service will re-invoke the function
//! when appropriate (e.g., after the wait duration or when a callback completes).
//!
//! # Architecture
//!
//! The termination manager uses tokio watch channels to coordinate between:
//!
//! - The handler task (runs user code)
//! - The runtime wrapper (monitors for termination)
//! - Durable operations (may trigger termination)
//!
//! When an operation triggers termination, the runtime receives the signal
//! and gracefully stops the handler after checkpoints complete.
//!
//! # Usage
//!
//! Users typically don't interact with the termination manager directly.
//! It's used internally by durable operations:
//!
//! ```rust,no_run
//! # use lambda_durable_execution_rust::prelude::*;
//! async fn example(ctx: DurableContextHandle) -> DurableResult<()> {
//!     // Internally, wait triggers termination:
//!     ctx.wait(Some("delay"), Duration::hours(1)).await?;
//!     // Lambda suspends here and resumes after 1 hour
//!     Ok(())
//! }
//! ```
//!
//! # Termination Reasons
//!
//! The [`TerminationReason`] enum captures why execution was terminated:
//!
//! - `Wait(operation_id, duration)` - Waiting for time duration
//! - `Callback(operation_id)` - Waiting for external callback
//! - `Retry(operation_id, error, delay)` - Retrying after failure

mod manager;

pub use manager::*;
