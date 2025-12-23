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
    BatchCompletionReason, BatchItem, BatchItemStatus, BatchResult, CallbackConfig,
    ChainedInvokeUpdateOptions, ChildContextConfig, ContextUpdateOptions, Duration, InvokeConfig,
    MapConfig, NamedParallelBranch, OperationAction, OperationStatus, OperationType,
    OperationUpdate, ParallelConfig, Serdes, SerdesContext, StepConfig, StepSemantics,
    WaitConditionConfig, WaitConditionDecision,
};
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

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

async fn safe_serialize<T>(
    serdes: Option<Arc<dyn Serdes<T>>>,
    value: Option<&T>,
    entity_id: &str,
    name: Option<&str>,
    execution_ctx: &ExecutionContext,
) -> Option<String>
where
    T: Serialize + Send + Sync,
{
    if let Some(serdes) = serdes {
        match serdes
            .serialize(
                value,
                SerdesContext {
                    entity_id: entity_id.to_string(),
                    durable_execution_arn: execution_ctx.durable_execution_arn.clone(),
                },
            )
            .await
        {
            Ok(v) => v,
            Err(e) => {
                let msg = format!(
                    "Serialization failed for {}({}): {}",
                    name.unwrap_or("operation"),
                    entity_id,
                    e
                );
                execution_ctx
                    .termination_manager
                    .terminate_for_serdes_failure(msg)
                    .await;
                std::future::pending::<()>().await;
                unreachable!()
            }
        }
    } else {
        match value {
            Some(v) => match serde_json::to_string(v) {
                Ok(s) => Some(s),
                Err(e) => {
                    let msg = format!(
                        "Serialization failed for {}({}): {}",
                        name.unwrap_or("operation"),
                        entity_id,
                        e
                    );
                    execution_ctx
                        .termination_manager
                        .terminate_for_serdes_failure(msg)
                        .await;
                    std::future::pending::<()>().await;
                    unreachable!()
                }
            },
            None => None,
        }
    }
}

async fn safe_deserialize<T>(
    serdes: Option<Arc<dyn Serdes<T>>>,
    data: Option<&str>,
    entity_id: &str,
    name: Option<&str>,
    execution_ctx: &ExecutionContext,
) -> Option<T>
where
    T: DeserializeOwned + Send + Sync + 'static,
{
    if let Some(serdes) = serdes {
        match serdes
            .deserialize(
                data,
                SerdesContext {
                    entity_id: entity_id.to_string(),
                    durable_execution_arn: execution_ctx.durable_execution_arn.clone(),
                },
            )
            .await
        {
            Ok(v) => v,
            Err(e) => {
                let msg = format!(
                    "Deserialization failed for {}({}): {}",
                    name.unwrap_or("operation"),
                    entity_id,
                    e
                );
                execution_ctx
                    .termination_manager
                    .terminate_for_serdes_failure(msg)
                    .await;
                std::future::pending::<()>().await;
                unreachable!()
            }
        }
    } else {
        match data {
            Some(d) => match serde_json::from_str::<T>(d) {
                Ok(v) => Some(v),
                Err(e) => {
                    let msg = format!(
                        "Deserialization failed for {}({}): {}",
                        name.unwrap_or("operation"),
                        entity_id,
                        e
                    );
                    execution_ctx
                        .termination_manager
                        .terminate_for_serdes_failure(msg)
                        .await;
                    std::future::pending::<()>().await;
                    unreachable!()
                }
            },
            None => None,
        }
    }
}

async fn safe_serialize_required_with_serdes<T>(
    serdes: Arc<dyn Serdes<T>>,
    value: &T,
    entity_id: &str,
    name: Option<&str>,
    execution_ctx: &ExecutionContext,
) -> String
where
    T: Send + Sync,
{
    match serdes
        .serialize(
            Some(value),
            SerdesContext {
                entity_id: entity_id.to_string(),
                durable_execution_arn: execution_ctx.durable_execution_arn.clone(),
            },
        )
        .await
    {
        Ok(Some(v)) => v,
        Ok(None) => {
            let msg = format!(
                "Serialization returned None for {}({})",
                name.unwrap_or("operation"),
                entity_id
            );
            execution_ctx
                .termination_manager
                .terminate_for_serdes_failure(msg)
                .await;
            std::future::pending::<()>().await;
            unreachable!()
        }
        Err(e) => {
            let msg = format!(
                "Serialization failed for {}({}): {}",
                name.unwrap_or("operation"),
                entity_id,
                e
            );
            execution_ctx
                .termination_manager
                .terminate_for_serdes_failure(msg)
                .await;
            std::future::pending::<()>().await;
            unreachable!()
        }
    }
}

async fn safe_deserialize_required_with_serdes<T>(
    serdes: Arc<dyn Serdes<T>>,
    data: &str,
    entity_id: &str,
    name: Option<&str>,
    execution_ctx: &ExecutionContext,
) -> T
where
    T: Send + Sync,
{
    match serdes
        .deserialize(
            Some(data),
            SerdesContext {
                entity_id: entity_id.to_string(),
                durable_execution_arn: execution_ctx.durable_execution_arn.clone(),
            },
        )
        .await
    {
        Ok(Some(v)) => v,
        Ok(None) => {
            let msg = format!(
                "Deserialization returned None for {}({})",
                name.unwrap_or("operation"),
                entity_id
            );
            execution_ctx
                .termination_manager
                .terminate_for_serdes_failure(msg)
                .await;
            std::future::pending::<()>().await;
            unreachable!()
        }
        Err(e) => {
            let msg = format!(
                "Deserialization failed for {}({}): {}",
                name.unwrap_or("operation"),
                entity_id,
                e
            );
            execution_ctx
                .termination_manager
                .terminate_for_serdes_failure(msg)
                .await;
            std::future::pending::<()>().await;
            unreachable!()
        }
    }
}

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

fn validate_completion_config(
    config: &crate::types::CompletionConfig,
    total_items: usize,
    operation_name: &str,
) -> DurableResult<()> {
    if let Some(min) = config.min_successful {
        if min > total_items {
            return Err(DurableError::InvalidConfiguration {
                message: format!(
                    "{operation_name}: min_successful ({min}) exceeds total items ({total_items})",
                ),
            });
        }
    }

    if let Some(tol) = config.tolerated_failure_count {
        if tol > total_items {
            return Err(DurableError::InvalidConfiguration {
                message: format!(
                    "{operation_name}: tolerated_failure_count ({tol}) exceeds total items ({total_items})",
                ),
            });
        }
    }

    if let Some(pct) = config.tolerated_failure_percentage {
        if !pct.is_finite() || !(0.0..=100.0).contains(&pct) {
            return Err(DurableError::InvalidConfiguration {
                message: format!(
                    "{operation_name}: tolerated_failure_percentage ({pct}) must be finite and between 0 and 100",
                ),
            });
        }
    }

    Ok(())
}

fn has_completion_criteria(config: &crate::types::CompletionConfig) -> bool {
    config.min_successful.is_some()
        || config.tolerated_failure_count.is_some()
        || config.tolerated_failure_percentage.is_some()
}

fn should_continue_batch(
    failure_count: usize,
    total_items: usize,
    config: &crate::types::CompletionConfig,
) -> bool {
    if !has_completion_criteria(config) {
        return failure_count == 0;
    }
    if let Some(tol) = config.tolerated_failure_count {
        if failure_count > tol {
            return false;
        }
    }
    if let Some(pct) = config.tolerated_failure_percentage {
        if total_items > 0 {
            let failure_pct = (failure_count as f64 / total_items as f64) * 100.0;
            if failure_pct > pct {
                return false;
            }
        }
    }
    true
}

fn compute_batch_completion_reason(
    failure_count: usize,
    success_count: usize,
    completed_count: usize,
    total_items: usize,
    config: &crate::types::CompletionConfig,
) -> BatchCompletionReason {
    if !should_continue_batch(failure_count, total_items, config) {
        BatchCompletionReason::FailureToleranceExceeded
    } else if completed_count == total_items {
        BatchCompletionReason::AllCompleted
    } else if let Some(min) = config.min_successful {
        if success_count >= min {
            BatchCompletionReason::MinSuccessfulReached
        } else {
            BatchCompletionReason::AllCompleted
        }
    } else {
        BatchCompletionReason::AllCompleted
    }
}

fn batch_completion_reason_str(reason: BatchCompletionReason) -> &'static str {
    match reason {
        BatchCompletionReason::AllCompleted => "ALL_COMPLETED",
        BatchCompletionReason::MinSuccessfulReached => "MIN_SUCCESSFUL_REACHED",
        BatchCompletionReason::FailureToleranceExceeded => "FAILURE_TOLERANCE_EXCEEDED",
    }
}

fn batch_status_str(failure_count: usize) -> &'static str {
    if failure_count > 0 {
        "FAILED"
    } else {
        "SUCCEEDED"
    }
}

fn map_summary_payload(
    total_count: usize,
    success_count: usize,
    failure_count: usize,
    completion_reason: BatchCompletionReason,
) -> serde_json::Value {
    serde_json::json!({
        "type": "MapResult",
        "totalCount": total_count,
        "successCount": success_count,
        "failureCount": failure_count,
        "completionReason": batch_completion_reason_str(completion_reason),
        "status": batch_status_str(failure_count),
    })
}

fn parallel_summary_payload(
    total_count: usize,
    success_count: usize,
    failure_count: usize,
    started_count: usize,
    completion_reason: BatchCompletionReason,
) -> serde_json::Value {
    serde_json::json!({
        "type": "ParallelResult",
        "totalCount": total_count,
        "successCount": success_count,
        "failureCount": failure_count,
        "startedCount": started_count,
        "completionReason": batch_completion_reason_str(completion_reason),
        "status": batch_status_str(failure_count),
    })
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
