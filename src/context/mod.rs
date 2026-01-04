//! Context types for durable execution.
//!
//! This module provides the core context types that users interact with when
//! building durable functions.
//!
//! # Key Types
//!
//! - [`DurableContextHandle`] - Main user-facing handle for durable operations
//! - [`StepContext`] - Limited context available inside step functions
//! - [`BatchResult`] - Results from parallel/map operations
//! - [`CallbackHandle`] - Handle for waiting on external callbacks
//!
//! # DurableContextHandle
//!
//! The [`DurableContextHandle`] is the primary interface for durable operations.
//! It is passed to your handler function and provides methods for:
//!
//! - Executing steps with automatic checkpointing
//! - Waiting for durations (Lambda suspends during wait)
//! - Waiting for external callbacks
//! - Invoking other Lambda functions
//! - Running operations in parallel
//! - Running operations in child contexts
//!
//! ## Example
//!
//! ```rust,no_run
//! use lambda_durable_execution_rust::prelude::*;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Clone, Deserialize)]
//! struct MyEvent {
//!     name: String,
//! }
//!
//! #[derive(Serialize)]
//! struct Response {
//!     result: String,
//! }
//!
//! async fn fetch() -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
//!     Ok("data".to_string())
//! }
//! async fn process(data: String) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
//!     Ok(format!("processed:{data}"))
//! }
//!
//! async fn my_handler(_event: MyEvent, ctx: DurableContextHandle) -> DurableResult<Response> {
//!     // Execute a step
//!     let data = ctx.step(Some("fetch"), |_| async { fetch().await }, None).await?;
//!
//!     // Wait for 5 minutes (no compute cost)
//!     ctx.wait(Some("delay"), Duration::minutes(5)).await?;
//!
//!     // Process the data
//!     let result = ctx
//!         .step(Some("process"), |_| async { process(data).await }, None)
//!         .await?;
//!
//!     Ok(Response { result })
//! }
//! ```
//!
//! # StepContext
//!
//! The [`StepContext`] is a limited context passed to step functions. It provides
//! logging capabilities but does not allow calling other durable operations
//! (to ensure step functions remain focused and deterministic).
//!
//! ```rust,no_run
//! # use lambda_durable_execution_rust::prelude::*;
//! async fn do_work() -> Result<u32, Box<dyn std::error::Error + Send + Sync>> {
//!     Ok(1)
//! }
//!
//! async fn example(ctx: DurableContextHandle) -> DurableResult<u32> {
//!     ctx.step(
//!         Some("my-step"),
//!         |step_ctx| async move {
//!             step_ctx.info("Starting processing");
//!             let result = do_work().await?;
//!             step_ctx.info("Processing complete");
//!             Ok(result)
//!         },
//!         None,
//!     )
//!     .await
//! }
//! ```
//!
//! # BatchResult
//!
//! The [`BatchResult`] type is returned from parallel and map operations. It
//! contains both successful results and errors, allowing you to handle partial
//! failures appropriately.
//!
//! ```rust,no_run
//! # use lambda_durable_execution_rust::prelude::*;
//! async fn example(ctx: DurableContextHandle) -> DurableResult<()> {
//!     let items = vec![1u32, 2u32, 3u32];
//!
//!     let batch = ctx
//!         .map(
//!             Some("process-all"),
//!             items,
//!             |item, item_ctx, _idx| async move {
//!                 item_ctx
//!                     .step(Some("process-one"), move |_| async move { Ok(item) }, None)
//!                     .await
//!             },
//!             None,
//!         )
//!         .await?;
//!
//!     let failure_count = batch.errors().len();
//!     let successes = batch.values();
//!     println!("{} succeeded, {} failed", successes.len(), failure_count);
//!     Ok(())
//! }
//! ```

mod durable_context;
mod execution_context;
mod step_context;

pub use crate::types::{BatchCompletionReason, BatchItem, BatchItemStatus, BatchResult};
pub use durable_context::{
    BoxFuture, CallbackHandle, ChildContextFn, DurableContextHandle, DurableContextImpl,
    DurableContextLogger, StepFn, SubmitterFn,
};
pub use execution_context::*;
pub use step_context::*;
