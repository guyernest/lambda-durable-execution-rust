//! Checkpoint management for durable executions.
//!
//! This module provides the [`CheckpointManager`] which handles persisting operation
//! state to AWS Lambda's durable execution checkpoint service.
//!
//! # Overview
//!
//! Checkpointing is the core mechanism that enables durable execution. When an
//! operation completes (successfully or with an error), its result is checkpointed
//! to durable storage. If the Lambda function is later restarted, it can replay
//! from these checkpoints rather than re-executing completed operations.
//!
//! # Architecture
//!
//! The checkpoint system consists of:
//!
//! - **Queue**: Checkpoint requests are queued to enable batching
//! - **Batching**: Multiple checkpoints are combined into single API calls
//! - **Size limits**: Payloads are split to respect the 750KB limit
//! - **Async flushing**: Checkpoints are flushed in the background
//!
//! # Usage
//!
//! The checkpoint manager is typically used internally by the SDK. Users interact
//! with checkpointing indirectly through durable operations:
//!
//! ```rust,no_run
//! # use lambda_durable_execution_rust::prelude::*;
//! async fn example(ctx: DurableContextHandle) -> DurableResult<()> {
//!     // This step is automatically checkpointed
//!     let _result: String = ctx
//!         .step(Some("my-step"), |_| async { Ok("done".to_string()) }, None)
//!         .await?;
//!     Ok(())
//! }
//! ```
//!
//! # Implementation Details
//!
//! - Checkpoints are sent to AWS Lambda's `checkpoint_durable_execution` API
//! - The manager uses tokio channels for async communication
//! - Flush operations wait for pending checkpoints to complete before returning

mod manager;

pub use manager::*;
