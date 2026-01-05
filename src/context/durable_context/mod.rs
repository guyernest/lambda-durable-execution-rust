//! DurableContext implementation.
//!
//! This module provides the core [`DurableContextHandle`] type, which is the main
//! interface for interacting with durable execution operations.
//!
//! # Overview
//!
//! The [`DurableContextHandle`] wraps an underlying implementation and provides
//! methods for executing durable operations. Each operation is automatically
//! checkpointed, allowing the function to resume from where it left off after
//! interruptions.
//!
//! # Example
//!
//! ```rust,no_run
//! # use lambda_durable_execution_rust::prelude::*;
//! # use serde::{Deserialize, Serialize};
//! # #[derive(Clone, Deserialize)]
//! # struct MyEvent;
//! # #[derive(Serialize)]
//! # struct MyResponse { result: String }
//! # async fn fetch_data() -> Result<String, Box<dyn std::error::Error + Send + Sync>> { Ok("data".to_string()) }
//! # async fn process(_data: String) -> Result<String, Box<dyn std::error::Error + Send + Sync>> { Ok("ok".to_string()) }
//! async fn my_handler(
//!     _event: MyEvent,
//!     ctx: DurableContextHandle,
//! ) -> DurableResult<MyResponse> {
//!     // Execute a step - automatically checkpointed
//!     let data = ctx
//!         .step(
//!             Some("fetch-data"),
//!             |step_ctx| async move {
//!                 step_ctx.info("Fetching data");
//!                 fetch_data().await
//!             },
//!             None,
//!         )
//!         .await?;
//!
//!     // Wait 5 minutes - Lambda suspends (no compute cost)
//!     ctx.wait(Some("delay"), Duration::minutes(5)).await?;
//!
//!     // Process the data
//!     let result = ctx
//!         .step(Some("process"), |_| async move { process(data).await }, None)
//!         .await?;
//!
//!     Ok(MyResponse { result })
//! }
//! ```

use crate::checkpoint::CheckpointManager;
use crate::context::{ExecutionContext, ExecutionMode, StepContext};
use crate::error::{DurableError, DurableResult, ErrorObject};
use crate::retry::presets;
use crate::types::{
    BatchItem, BatchItemStatus, BatchResult, CallbackConfig, ChainedInvokeUpdateOptions,
    ChildContextConfig, ContextUpdateOptions, DurableLogData, DurableLogLevel, Duration,
    InvokeConfig, MapConfig, NamedParallelBranch, OperationAction, OperationStatus, OperationType,
    OperationUpdate, ParallelConfig, Serdes, StepConfig, StepSemantics, WaitConditionConfig,
    WaitConditionDecision,
};
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

mod batch;
mod serdes;
use batch::*;
use serdes::*;
mod callback;
mod child;
mod invoke;
mod map;
mod parallel;
mod step;
mod wait;
mod wait_condition;

#[cfg(test)]
mod tests;

/// Checkpoint payload size limit in bytes (256KB).
///
/// Matches the durable execution backend maximum payload size for operation updates.
const CHECKPOINT_SIZE_LIMIT_BYTES: usize = 256 * 1024;

/// Handle for an external callback.
///
/// Created by [`DurableContextHandle::create_callback`] when you need to
/// send a callback ID to an external system before waiting for completion.
///
/// # Example
///
/// ```rust,no_run
/// # use lambda_durable_execution_rust::prelude::*;
/// # use serde::{Deserialize, Serialize};
/// # #[derive(Clone)]
/// # struct ApprovalRequest;
/// # #[derive(Deserialize, Serialize)]
/// # struct ApprovalDecision { approved: bool }
/// # async fn send_approval_request(
/// #     _callback_id: &str,
/// #     _request: &ApprovalRequest,
/// # ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
/// #     Ok(())
/// # }
/// async fn example(
///     ctx: DurableContextHandle,
///     request: ApprovalRequest,
/// ) -> DurableResult<ApprovalDecision> {
///     // Create callback handle
///     let handle: CallbackHandle<ApprovalDecision> =
///         ctx.create_callback(Some("approval"), None).await?;
///
///     // Send callback ID to external system
///     send_approval_request(handle.callback_id(), &request)
///         .await
///         .map_err(|e| DurableError::step_failed_msg("send-approval-request", 1, e.to_string()))?;
///
///     // Later, wait for the callback
///     let decision = handle.wait().await?;
///     Ok(decision)
/// }
/// ```
#[derive(Clone)]
pub struct CallbackHandle<T> {
    /// The callback ID to provide to external systems.
    callback_id: String,
    /// Original (unhashed) step ID for debugging/errors.
    step_id: String,
    /// Hashed operation ID used in durable execution state.
    hashed_id: String,
    /// Execution context for fetching state and triggering suspension.
    execution_ctx: ExecutionContext,
    /// Optional Serdes for callback result payloads.
    serdes: Option<Arc<dyn Serdes<T>>>,
    /// Phantom data for the result type.
    _phantom: std::marker::PhantomData<T>,
}

/// Boxed future type alias for async operations.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Type alias for step functions.
pub type StepFn<T> = Box<
    dyn FnOnce(
            StepContext,
        ) -> BoxFuture<'static, Result<T, Box<dyn std::error::Error + Send + Sync>>>
        + Send,
>;

/// Type alias for callback submitter functions.
pub type SubmitterFn = Box<
    dyn FnOnce(
            String,
            StepContext,
        ) -> BoxFuture<'static, Result<(), Box<dyn std::error::Error + Send + Sync>>>
        + Send,
>;

/// Type alias for child context functions.
pub type ChildContextFn<T> =
    Box<dyn FnOnce(DurableContextHandle) -> BoxFuture<'static, DurableResult<T>> + Send>;

/// A handle to a DurableContext for use in handler functions.
///
/// This is the main interface for durable execution operations. It provides
/// methods for executing operations that are automatically checkpointed,
/// allowing your function to resume from where it left off after interruptions.
///
/// # Operations
///
/// | Method | Description |
/// |--------|-------------|
/// | [`step`](Self::step) | Execute a function with checkpointing |
/// | [`wait`](Self::wait) | Suspend for a duration |
/// | [`invoke`](Self::invoke) | Call another Lambda function |
/// | [`wait_for_callback`](Self::wait_for_callback) | Wait for external completion |
/// | [`create_callback`](Self::create_callback) | Create callback handle for later waiting |
/// | [`run_in_child_context`](Self::run_in_child_context) | Group operations in child context |
///
/// # Thread Safety
///
/// `DurableContextHandle` implements `Clone` and can be safely shared across
/// async tasks. It internally uses `Arc` for shared state.
///
/// # Example
///
/// ```rust,no_run
/// # use lambda_durable_execution_rust::prelude::*;
/// # use lambda_durable_execution_rust::retry::ExponentialBackoff;
/// # use serde::{Deserialize, Serialize};
/// # #[derive(Clone, Deserialize)]
/// # struct MyEvent;
/// # #[derive(Serialize)]
/// # struct MyResponse { value: u32, data: String }
/// # async fn fetch_data() -> Result<String, Box<dyn std::error::Error + Send + Sync>> { Ok("data".to_string()) }
/// async fn my_handler(
///     _event: MyEvent,
///     ctx: DurableContextHandle,
/// ) -> DurableResult<MyResponse> {
///     // Simple step
///     let value: u32 = ctx
///         .step(Some("get-value"), |_| async { Ok(42u32) }, None)
///         .await?;
///
///     // Step with retry configuration
///     let retry = ExponentialBackoff::builder().max_attempts(5).build();
///     let config = StepConfig::<String>::new().with_retry_strategy(Arc::new(retry));
///     let data: String = ctx
///         .step(
///             Some("fetch-data"),
///             |step_ctx| async move {
///                 step_ctx.info("Fetching...");
///                 fetch_data().await
///             },
///             Some(config),
///         )
///         .await?;
///
///     Ok(MyResponse { value, data })
/// }
/// ```
#[derive(Clone)]
pub struct DurableContextHandle {
    inner: Arc<DurableContextImpl>,
}

impl DurableContextHandle {
    /// Create a new handle from an implementation.
    pub fn new(inner: Arc<DurableContextImpl>) -> Self {
        Self { inner }
    }

    /// Get the execution context.
    pub fn execution_context(&self) -> &ExecutionContext {
        &self.inner.execution_ctx
    }

    /// Get a durable logger for context-level messages.
    ///
    /// The logger uses the configured durable logger and respects mode-aware logging.
    pub fn logger(&self) -> DurableContextLogger {
        DurableContextLogger::new(self.inner.execution_ctx.clone())
    }
}

impl std::fmt::Debug for DurableContextHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DurableContextHandle")
            .field("inner", &self.inner)
            .finish()
    }
}

/// Implementation of durable context operations.
pub struct DurableContextImpl {
    /// Execution context.
    execution_ctx: ExecutionContext,
}

impl DurableContextImpl {
    /// Create a new DurableContext implementation.
    pub fn new(execution_ctx: ExecutionContext) -> Self {
        Self { execution_ctx }
    }

    /// Get the execution context.
    pub fn execution_context(&self) -> &ExecutionContext {
        &self.execution_ctx
    }

    /// Hash an operation ID.
    pub fn hash_id(id: &str) -> String {
        CheckpointManager::hash_id(id)
    }
}

impl std::fmt::Debug for DurableContextImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DurableContextImpl")
            .field("execution_ctx", &self.execution_ctx)
            .finish()
    }
}

/// Mode-aware logger for durable context operations.
#[derive(Clone)]
pub struct DurableContextLogger {
    execution_ctx: ExecutionContext,
}

impl DurableContextLogger {
    pub(crate) fn new(execution_ctx: ExecutionContext) -> Self {
        Self { execution_ctx }
    }

    fn should_log(&self) -> bool {
        if !self.execution_ctx.mode_aware_logging {
            return true;
        }

        match self.execution_ctx.mode.try_lock() {
            Ok(mode) => *mode != ExecutionMode::Replay,
            Err(_) => true,
        }
    }

    fn log_data(&self) -> DurableLogData {
        DurableLogData {
            durable_execution_arn: self.execution_ctx.durable_execution_arn.clone(),
            operation_id: self.execution_ctx.current_parent_id.clone(),
            step_name: None,
            attempt: None,
        }
    }

    /// Log a debug message.
    pub fn debug(&self, message: &str) {
        if !self.should_log() {
            return;
        }
        self.execution_ctx
            .logger
            .log(DurableLogLevel::Debug, &self.log_data(), message, None);
    }

    /// Log an info message.
    pub fn info(&self, message: &str) {
        if !self.should_log() {
            return;
        }
        self.execution_ctx
            .logger
            .log(DurableLogLevel::Info, &self.log_data(), message, None);
    }

    /// Log a warning message.
    pub fn warn(&self, message: &str) {
        if !self.should_log() {
            return;
        }
        self.execution_ctx
            .logger
            .log(DurableLogLevel::Warn, &self.log_data(), message, None);
    }

    /// Log an error message.
    pub fn error(&self, message: &str) {
        if !self.should_log() {
            return;
        }
        self.execution_ctx
            .logger
            .log(DurableLogLevel::Error, &self.log_data(), message, None);
    }

    /// Log a debug message with additional fields.
    pub fn debug_with<F>(&self, message: &str, fields: F)
    where
        F: FnOnce() -> Vec<(&'static str, String)>,
    {
        if !self.should_log() {
            return;
        }
        let extra = fields();
        self.execution_ctx.logger.log(
            DurableLogLevel::Debug,
            &self.log_data(),
            message,
            Some(&extra),
        );
    }

    /// Log an info message with additional fields.
    pub fn info_with<F>(&self, message: &str, fields: F)
    where
        F: FnOnce() -> Vec<(&'static str, String)>,
    {
        if !self.should_log() {
            return;
        }
        let extra = fields();
        self.execution_ctx.logger.log(
            DurableLogLevel::Info,
            &self.log_data(),
            message,
            Some(&extra),
        );
    }
}
