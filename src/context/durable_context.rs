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

impl<T> CallbackHandle<T> {
    /// Get the callback ID to provide to external systems.
    ///
    /// The external system should use this ID when completing the callback
    /// via the AWS Lambda CompleteCallback API.
    pub fn callback_id(&self) -> &str {
        &self.callback_id
    }

    /// Wait for the callback to be completed by an external system.
    ///
    /// This suspends the Lambda function until the callback is completed.
    /// The external system must call the AWS Lambda CompleteCallback API
    /// with the callback ID.
    pub async fn wait(self) -> DurableResult<T>
    where
        T: serde::de::DeserializeOwned + Send + Sync + 'static,
    {
        if let Some(operation) = self.execution_ctx.get_step_data(&self.hashed_id).await {
            match operation.status {
                OperationStatus::Succeeded => {
                    if let Some(details) = operation.callback_details.as_ref() {
                        if let Some(payload) = details.result.as_ref() {
                            if let Some(val) = safe_deserialize(
                                self.serdes.clone(),
                                Some(payload.as_str()),
                                &self.hashed_id,
                                Some(&self.step_id),
                                &self.execution_ctx,
                            )
                            .await
                            {
                                return Ok(val);
                            }
                        }
                    }
                    return Err(DurableError::Internal(
                        "Missing callback result in replay".to_string(),
                    ));
                }
                OperationStatus::Failed => {
                    let error_msg = operation
                        .callback_details
                        .as_ref()
                        .and_then(|d| d.error.as_ref())
                        .map(|e| e.error_message.clone())
                        .unwrap_or_else(|| "Unknown error".to_string());
                    return Err(DurableError::CallbackFailed {
                        name: self.step_id,
                        message: error_msg,
                    });
                }
                _ => {
                    // Pending/started - suspend.
                }
            }
        }

        // Mark this operation as awaited before suspending
        self.execution_ctx
            .checkpoint_manager
            .mark_awaited(&self.hashed_id)
            .await;

        self.execution_ctx
            .termination_manager
            .terminate_for_callback()
            .await;

        std::future::pending::<()>().await;
        unreachable!()
    }

    /// Wait for the callback to complete and return the raw payload string.
    ///
    /// This is primarily used internally by `wait_for_callback` to mirror the
    /// JS SDK two-phase behavior (child context returns a string; parent
    /// deserializes with custom Serdes).
    pub async fn wait_raw(self) -> DurableResult<String> {
        if let Some(operation) = self.execution_ctx.get_step_data(&self.hashed_id).await {
            match operation.status {
                OperationStatus::Succeeded => {
                    if let Some(details) = operation.callback_details.as_ref() {
                        if let Some(payload) = details.result.as_ref() {
                            return Ok(payload.clone());
                        }
                    }
                    return Err(DurableError::Internal(
                        "Missing callback result in replay".to_string(),
                    ));
                }
                OperationStatus::Failed => {
                    let error_msg = operation
                        .callback_details
                        .as_ref()
                        .and_then(|d| d.error.as_ref())
                        .map(|e| e.error_message.clone())
                        .unwrap_or_else(|| "Unknown error".to_string());
                    return Err(DurableError::CallbackFailed {
                        name: self.step_id,
                        message: error_msg,
                    });
                }
                _ => {
                    // Pending/started - suspend.
                }
            }
        }

        // Mark this operation as awaited before suspending
        self.execution_ctx
            .checkpoint_manager
            .mark_awaited(&self.hashed_id)
            .await;

        self.execution_ctx
            .termination_manager
            .terminate_for_callback()
            .await;

        std::future::pending::<()>().await;
        unreachable!()
    }
}

impl<T> std::fmt::Debug for CallbackHandle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallbackHandle")
            .field("callback_id", &self.callback_id)
            .field("step_id", &self.step_id)
            .field("hashed_id", &self.hashed_id)
            .field("serdes", &self.serdes.is_some())
            .finish()
    }
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

impl DurableContextHandle {
    /// Create a new handle from an implementation.
    pub fn new(inner: Arc<DurableContextImpl>) -> Self {
        Self { inner }
    }

    /// Execute a durable step with automatic retry and checkpointing.
    ///
    /// Steps are the fundamental building blocks of durable functions. Each step
    /// is checkpointed after completion, providing replay-safe semantics across
    /// Lambda restarts. Use [`StepSemantics`](crate::types::StepSemantics) to
    /// choose at-least-once vs at-most-once per retry cycle.
    ///
    /// # Arguments
    ///
    /// * `name` - Optional name for tracking, debugging, and replay validation.
    ///   Providing meaningful names helps with debugging and observability.
    /// * `step_fn` - Async function to execute. Receives a [`StepContext`] for logging.
    /// * `config` - Optional [`StepConfig`] for retry strategy and execution semantics.
    ///
    /// # Returns
    ///
    /// The result of the step function, either fresh or from cache on replay.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use lambda_durable_execution_rust::prelude::*;
    /// # use lambda_durable_execution_rust::retry::ExponentialBackoff;
    /// # async fn fetch_data() -> Result<String, Box<dyn std::error::Error + Send + Sync>> { Ok("data".to_string()) }
    /// async fn example(ctx: DurableContextHandle) -> DurableResult<(u32, String, u32)> {
    ///     // Basic step
    ///     let a: u32 = ctx
    ///         .step(Some("compute-value"), |_| async { Ok(42u32) }, None)
    ///         .await?;
    ///
    ///     // Step with logging
    ///     let data: String = ctx
    ///         .step(
    ///             Some("fetch-data"),
    ///             |step_ctx| async move {
    ///                 step_ctx.info("Starting fetch");
    ///                 let result = fetch_data().await?;
    ///                 step_ctx.info("Fetch complete");
    ///                 Ok(result)
    ///             },
    ///             None,
    ///         )
    ///         .await?;
    ///
    ///     // Step with custom retry strategy
    ///     let retry = ExponentialBackoff::builder()
    ///         .max_attempts(5)
    ///         .initial_delay(Duration::seconds(1))
    ///         .build();
    ///     let config = StepConfig::<u32>::new().with_retry_strategy(Arc::new(retry));
    ///     let b: u32 = ctx
    ///         .step(Some("risky-operation"), |_| async { Ok(7u32) }, Some(config))
    ///         .await?;
    ///
    ///     Ok((a, data, b))
    /// }
    /// ```
    ///
    /// # Replay Behavior
    ///
    /// On replay (when Lambda restarts), if the step was previously completed,
    /// the cached result is returned without re-executing the step function.
    /// This ensures idempotency.
    pub async fn step<T, F, Fut>(
        &self,
        name: Option<&str>,
        step_fn: F,
        config: Option<StepConfig<T>>,
    ) -> DurableResult<T>
    where
        T: Serialize + DeserializeOwned + Send + Sync + 'static,
        F: FnOnce(StepContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, Box<dyn std::error::Error + Send + Sync>>> + Send + 'static,
    {
        self.inner.step(name, step_fn, config).await
    }

    /// Wait for a specified duration.
    ///
    /// The Lambda function suspends during the wait, so you don't pay for
    /// compute time while waiting. This is ideal for:
    ///
    /// - Implementing delays between operations
    /// - Rate limiting
    /// - Waiting for external processes to complete
    /// - Scheduled tasks
    ///
    /// # Arguments
    ///
    /// * `name` - Optional name for tracking and debugging
    /// * `duration` - How long to wait
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use lambda_durable_execution_rust::prelude::*;
    /// async fn example(ctx: DurableContextHandle) -> DurableResult<()> {
    ///     // Wait 5 minutes between operations
    ///     ctx.wait(Some("rate-limit-delay"), Duration::minutes(5)).await?;
    ///
    ///     // Wait 1 hour for a scheduled task
    ///     ctx.wait(Some("scheduled-wait"), Duration::hours(1)).await?;
    ///     Ok(())
    /// }
    /// ```
    ///
    /// # Cost Efficiency
    ///
    /// Unlike `tokio::time::sleep`, this wait is "free" in terms of compute cost.
    /// The Lambda function returns control to AWS, which will re-invoke it after
    /// the specified duration. You only pay for the brief execution time before
    /// and after the wait.
    pub async fn wait(&self, name: Option<&str>, duration: Duration) -> DurableResult<()> {
        self.inner.wait(name, duration).await
    }

    /// Invoke another durable Lambda function.
    ///
    /// Calls another Lambda function and waits for its result. The invocation
    /// is checkpointed, so on replay the cached result is returned without
    /// re-invoking the function.
    ///
    /// # Arguments
    ///
    /// * `name` - Optional name for tracking and debugging
    /// * `function_id` - Lambda function ARN or name
    /// * `input` - Optional input to pass to the function
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use lambda_durable_execution_rust::prelude::*;
    /// # use serde::{Deserialize, Serialize};
    /// # #[derive(Serialize)]
    /// # struct ProcessInput { data: &'static str }
    /// # #[derive(Deserialize)]
    /// # struct ProcessResult { ok: bool }
    /// # #[derive(Serialize)]
    /// # struct ValidationInput { value: u32 }
    /// # #[derive(Deserialize)]
    /// # struct ValidationResult { valid: bool }
    /// async fn example(ctx: DurableContextHandle) -> DurableResult<(ProcessResult, ValidationResult)> {
    ///     // Invoke by function name
    ///     let a: ProcessResult = ctx
    ///         .invoke(
    ///             Some("call-processor"),
    ///             "my-processor-function",
    ///             Some(ProcessInput { data: "hello" }),
    ///         )
    ///         .await?;
    ///
    ///     // Invoke by ARN
    ///     let b: ValidationResult = ctx
    ///         .invoke(
    ///             Some("validate"),
    ///             "arn:aws:lambda:us-east-1:123456789:function:validator",
    ///             Some(ValidationInput { value: 1 }),
    ///         )
    ///         .await?;
    ///     Ok((a, b))
    /// }
    /// ```
    pub async fn invoke<I, O>(
        &self,
        name: Option<&str>,
        function_id: &str,
        input: Option<I>,
    ) -> DurableResult<O>
    where
        I: Serialize + Send + Sync,
        O: DeserializeOwned + Send + Sync + 'static,
    {
        self.inner
            .invoke_with_config(name, function_id, input, None)
            .await
    }

    /// Invoke another durable Lambda function with custom configuration.
    ///
    /// Use this when you need custom Serdes for the input/result payloads.
    pub async fn invoke_with_config<I, O>(
        &self,
        name: Option<&str>,
        function_id: &str,
        input: Option<I>,
        config: Option<InvokeConfig<I, O>>,
    ) -> DurableResult<O>
    where
        I: Serialize + Send + Sync,
        O: DeserializeOwned + Send + Sync + 'static,
    {
        self.inner
            .invoke_with_config(name, function_id, input, config)
            .await
    }

    /// Wait for an external system to complete a callback.
    ///
    /// This is the primary way to integrate with external systems that need
    /// to signal completion asynchronously. Common use cases include:
    ///
    /// - Human approval workflows
    /// - Webhook-based integrations
    /// - Long-running external jobs
    /// - Payment confirmations
    ///
    /// # Arguments
    ///
    /// * `name` - Optional name for tracking and debugging
    /// * `submitter` - Function called with the callback ID to notify external system
    /// * `config` - Optional configuration including timeout
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use lambda_durable_execution_rust::prelude::*;
    /// # use serde::Deserialize;
    /// # #[derive(Deserialize)]
    /// # struct ApprovalDecision { approved: bool }
    /// # async fn send_approval_email(
    /// #     _callback_id: &str,
    /// #     _approver_email: &str,
    /// # ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    /// #     Ok(())
    /// # }
    /// async fn example(ctx: DurableContextHandle) -> DurableResult<()> {
    ///     let approver_email = "approver@example.com".to_string();
    ///
    ///     let approval: ApprovalDecision = ctx
    ///         .wait_for_callback(
    ///             Some("await-approval"),
    ///             |callback_id, step_ctx| async move {
    ///                 step_ctx.info(&format!("Callback ID: {}", callback_id));
    ///                 send_approval_email(&callback_id, &approver_email).await
    ///             },
    ///             Some(CallbackConfig::new().with_timeout(Duration::hours(24))),
    ///         )
    ///         .await?;
    ///
    ///     if approval.approved {
    ///         // Continue with approved action
    ///     }
    ///     Ok(())
    /// }
    /// ```
    ///
    /// # How It Works
    ///
    /// 1. The SDK generates a unique callback ID
    /// 2. Your submitter function is called with this ID
    /// 3. Lambda suspends (no compute cost while waiting)
    /// 4. External system calls AWS Lambda CompleteCallback API with the ID
    /// 5. Lambda resumes and returns the result
    pub async fn wait_for_callback<T, F, Fut>(
        &self,
        name: Option<&str>,
        submitter: F,
        config: Option<CallbackConfig<T>>,
    ) -> DurableResult<T>
    where
        T: DeserializeOwned + Send + Sync + 'static,
        F: FnOnce(String, StepContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send + 'static,
    {
        self.inner.wait_for_callback(name, submitter, config).await
    }

    /// Create a callback handle for external systems.
    ///
    /// Use this when you need more control over the callback workflow than
    /// [`wait_for_callback`](Self::wait_for_callback) provides. This creates
    /// a callback handle immediately, allowing you to:
    ///
    /// - Send the callback ID before starting to wait
    /// - Perform other operations between creating and waiting
    /// - Implement custom waiting logic
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use lambda_durable_execution_rust::prelude::*;
    /// # use serde::Deserialize;
    /// # #[derive(Deserialize)]
    /// # struct PaymentResult { ok: bool }
    /// # async fn initiate_payment(
    /// #     _callback_id: &str,
    /// #     _amount: u32,
    /// # ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> { Ok(()) }
    /// # async fn prepare_order_for_fulfillment() -> Result<(), Box<dyn std::error::Error + Send + Sync>> { Ok(()) }
    /// async fn example(ctx: DurableContextHandle) -> DurableResult<PaymentResult> {
    ///     let amount = 100u32;
    ///
    ///     // Create callback handle
    ///     let handle: CallbackHandle<PaymentResult> =
    ///         ctx.create_callback(Some("payment-callback"), None).await?;
    ///
    ///     // Send callback ID to payment processor
    ///     initiate_payment(handle.callback_id(), amount)
    ///         .await
    ///         .map_err(|e| DurableError::step_failed_msg("initiate-payment", 1, e.to_string()))?;
    ///
    ///     // Do other work while payment processes
    ///     ctx.step(
    ///         Some("prepare-fulfillment"),
    ///         |_| async move { prepare_order_for_fulfillment().await },
    ///         None,
    ///     )
    ///     .await?;
    ///
    ///     // Wait for payment completion
    ///     let result = handle.wait().await?;
    ///     Ok(result)
    /// }
    /// ```
    pub async fn create_callback<T>(
        &self,
        name: Option<&str>,
        config: Option<CallbackConfig<T>>,
    ) -> DurableResult<CallbackHandle<T>>
    where
        T: DeserializeOwned + Send + Sync + 'static,
    {
        let step_id = self.inner.execution_ctx.next_operation_id(name);
        let hashed_id = DurableContextImpl::hash_id(&step_id);
        let serdes = config.as_ref().and_then(|c| c.serdes.clone());

        // If this callback hasn't been started yet, checkpoint a START operation.
        if self
            .inner
            .execution_ctx
            .get_step_data(&hashed_id)
            .await
            .is_none()
        {
            let parent_id = self.inner.execution_ctx.get_parent_id().await;
            let mut builder = OperationUpdate::builder()
                .id(&hashed_id)
                .operation_type(OperationType::Callback)
                .sub_type("Callback")
                .action(OperationAction::Start);

            if let Some(pid) = parent_id {
                builder = builder.parent_id(pid);
            }
            if let Some(n) = name {
                builder = builder.name(n);
            }
            if let Some(ref cfg) = config {
                let cb_options = crate::types::CallbackUpdateOptions {
                    timeout_seconds: cfg.timeout.map(|d| d.to_seconds_i32_saturating()),
                    heartbeat_timeout_seconds: cfg
                        .heartbeat_timeout
                        .map(|d| d.to_seconds_i32_saturating()),
                };
                builder = builder.callback_options(cb_options);
            }

            self.inner
                .execution_ctx
                .checkpoint_manager
                .checkpoint(
                    step_id.clone(),
                    builder.build().map_err(|e| {
                        DurableError::Internal(format!(
                            "Failed to build callback START update: {e}",
                        ))
                    })?,
                )
                .await?;
        }

        // Try to use the service-generated callback id if available.
        let callback_id = self
            .inner
            .execution_ctx
            .get_step_data(&hashed_id)
            .await
            .and_then(|op| op.callback_details.and_then(|d| d.callback_id))
            .unwrap_or_else(|| {
                format!(
                    "{}:{}",
                    self.inner.execution_ctx.durable_execution_arn, hashed_id
                )
            });

        Ok(CallbackHandle {
            callback_id,
            step_id,
            hashed_id,
            execution_ctx: self.inner.execution_ctx.clone(),
            serdes,
            _phantom: std::marker::PhantomData,
        })
    }

    /// Run operations in an isolated child context.
    ///
    /// Child contexts group related operations together, providing:
    ///
    /// - Hierarchical checkpoint organization
    /// - Scoped operation naming
    /// - Atomic completion of grouped operations
    ///
    /// # Arguments
    ///
    /// * `name` - Optional name for tracking and debugging
    /// * `context_fn` - Function receiving a new context handle for child operations
    /// * `config` - Optional configuration for the child context
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use lambda_durable_execution_rust::prelude::*;
    /// # async fn process_step_1() -> Result<u32, Box<dyn std::error::Error + Send + Sync>> { Ok(1) }
    /// # async fn process_step_2(_x: u32) -> Result<u32, Box<dyn std::error::Error + Send + Sync>> { Ok(2) }
    /// async fn example(ctx: DurableContextHandle) -> DurableResult<u32> {
    ///     // Group related operations
    ///     let processed: u32 = ctx
    ///         .run_in_child_context(
    ///             Some("batch-processing"),
    ///             |child_ctx| async move {
    ///                 let step1: u32 = child_ctx
    ///                     .step(
    ///                         Some("step-1"),
    ///                         |_| async { process_step_1().await },
    ///                         None,
    ///                     )
    ///                     .await?;
    ///
    ///                 let step2: u32 = child_ctx
    ///                     .step(
    ///                         Some("step-2"),
    ///                         move |_| async move { process_step_2(step1).await },
    ///                         None,
    ///                     )
    ///                     .await?;
    ///
    ///                 Ok(step2)
    ///             },
    ///             None,
    ///         )
    ///         .await?;
    ///     Ok(processed)
    /// }
    /// ```
    ///
    /// # Use Cases
    ///
    /// - **Batch processing**: Group items being processed together
    /// - **Transaction scopes**: Group operations that should complete atomically
    /// - **Modular workflows**: Organize complex workflows into logical units
    pub async fn run_in_child_context<T, F, Fut>(
        &self,
        name: Option<&str>,
        context_fn: F,
        config: Option<ChildContextConfig<T>>,
    ) -> DurableResult<T>
    where
        T: Serialize + DeserializeOwned + Send + Sync + 'static,
        F: FnOnce(DurableContextHandle) -> Fut + Send + 'static,
        Fut: Future<Output = DurableResult<T>> + Send + 'static,
    {
        self.inner
            .run_in_child_context(name, context_fn, config)
            .await
    }

    /// Map over a list of items using durable child contexts.
    ///
    /// Each item is processed in its own child context, so durable operations
    /// inside the mapper are isolated and replay-safe.
    pub async fn map<TIn, TOut, F, Fut>(
        &self,
        name: Option<&str>,
        items: Vec<TIn>,
        map_fn: F,
        config: Option<MapConfig<TIn, TOut>>,
    ) -> DurableResult<BatchResult<TOut>>
    where
        TIn: Serialize + DeserializeOwned + Send + 'static,
        TOut: Serialize + DeserializeOwned + Send + Sync + 'static,
        F: Fn(TIn, DurableContextHandle, usize) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = DurableResult<TOut>> + Send + 'static,
    {
        let cfg = config.unwrap_or_default();
        let map_fn = Arc::new(map_fn);
        let items_len = items.len();
        validate_completion_config(&cfg.completion_config, items_len, name.unwrap_or("map"))?;
        let mode = self.inner.execution_ctx.get_mode().await;
        let item_serdes = cfg.item_serdes.clone();
        let item_namer = cfg.item_namer.clone();
        let batch_serdes = cfg.serdes.clone();

        let has_any_completion_criteria = cfg.completion_config.min_successful.is_some()
            || cfg.completion_config.tolerated_failure_count.is_some()
            || cfg.completion_config.tolerated_failure_percentage.is_some();

        let should_continue = |failure_count: usize| -> bool {
            if !has_any_completion_criteria {
                return failure_count == 0;
            }
            if let Some(tol) = cfg.completion_config.tolerated_failure_count {
                if failure_count > tol {
                    return false;
                }
            }
            if let Some(pct) = cfg.completion_config.tolerated_failure_percentage {
                if items_len > 0 {
                    let failure_pct = (failure_count as f64 / items_len as f64) * 100.0;
                    if failure_pct > pct {
                        return false;
                    }
                }
            }
            true
        };

        let compute_completion_reason =
            |failure_count: usize, success_count: usize, completed_count: usize| {
                if !should_continue(failure_count) {
                    BatchCompletionReason::FailureToleranceExceeded
                } else if completed_count == items_len {
                    BatchCompletionReason::AllCompleted
                } else if let Some(min) = cfg.completion_config.min_successful {
                    if success_count >= min {
                        BatchCompletionReason::MinSuccessfulReached
                    } else {
                        BatchCompletionReason::AllCompleted
                    }
                } else {
                    BatchCompletionReason::AllCompleted
                }
            };

        // Start top-level MAP context for observability.
        let map_step_id = self.inner.execution_ctx.next_operation_id(name);
        let map_hashed_id = DurableContextImpl::hash_id(&map_step_id);
        if self
            .inner
            .execution_ctx
            .get_step_data(&map_hashed_id)
            .await
            .is_none()
        {
            let parent_id = self.inner.execution_ctx.get_parent_id().await;
            let mut builder = OperationUpdate::builder()
                .id(&map_hashed_id)
                .operation_type(OperationType::Context)
                .sub_type("Map")
                .action(OperationAction::Start);
            if let Some(pid) = parent_id {
                builder = builder.parent_id(pid);
            }
            if let Some(n) = name {
                builder = builder.name(n);
            }
            self.inner
                .execution_ctx
                .checkpoint_manager
                .checkpoint(
                    map_step_id.clone(),
                    builder.build().map_err(|e| {
                        DurableError::Internal(format!("Failed to build map START update: {e}"))
                    })?,
                )
                .await?;
        }

        // Replay handling: if the top-level map completed, reconstruct children and skip incomplete ones.
        if mode == ExecutionMode::Replay {
            if let Some(op) = self.inner.execution_ctx.get_step_data(&map_hashed_id).await {
                match op.status {
                    OperationStatus::Failed => {
                        let msg = op
                            .context_details
                            .as_ref()
                            .and_then(|d| d.error.as_ref())
                            .map(|e| e.error_message.clone())
                            .unwrap_or_else(|| "Batch operation failed".to_string());
                        return Err(DurableError::BatchOperationFailed {
                            name: name.unwrap_or("map").to_string(),
                            message: msg,
                            successful_count: 0,
                            failed_count: 0,
                        });
                    }
                    OperationStatus::Succeeded => {
                        if let Some(payload) =
                            op.context_details.as_ref().and_then(|d| d.result.as_ref())
                        {
                            if let Some(batch_serdes) = batch_serdes.clone() {
                                let batch: BatchResult<TOut> =
                                    safe_deserialize_required_with_serdes(
                                        batch_serdes,
                                        payload,
                                        &map_hashed_id,
                                        name,
                                        &self.inner.execution_ctx,
                                    )
                                    .await;

                                let target_total_count = batch
                                    .all
                                    .iter()
                                    .map(|i| i.index)
                                    .max()
                                    .map(|m| m + 1)
                                    .unwrap_or(0);

                                if target_total_count > items.len() {
                                    return Err(DurableError::ReplayValidationFailed {
                                        expected: format!("map totalCount <= {}", items.len()),
                                        actual: target_total_count.to_string(),
                                    });
                                }

                                // Consume child context operation IDs to keep the parent context counter in sync.
                                for (index, item) in
                                    items.iter().enumerate().take(target_total_count)
                                {
                                    let item_name = if let Some(ref namer) = item_namer {
                                        namer(item, index)
                                    } else {
                                        format!("{}-item-{}", name.unwrap_or("map"), index)
                                    };
                                    let _ = self
                                        .inner
                                        .execution_ctx
                                        .next_operation_id(Some(&item_name));
                                }

                                return Ok(batch);
                            }
                        }

                        let target_total_count = op
                            .context_details
                            .as_ref()
                            .and_then(|d| d.result.as_ref())
                            .and_then(|payload| {
                                serde_json::from_str::<serde_json::Value>(payload)
                                    .ok()
                                    .and_then(|v| v.get("totalCount").and_then(|tc| tc.as_u64()))
                            })
                            .map(|tc| tc as usize);

                        if let Some(target_total_count) = target_total_count {
                            let mut successes = Vec::new();
                            let mut failures = Vec::new();
                            let mut started = Vec::new();
                            let mut seen_count = 0usize;

                            for (index, item) in items.iter().enumerate() {
                                if seen_count >= target_total_count {
                                    break;
                                }

                                let item_name = if let Some(ref namer) = item_namer {
                                    namer(item, index)
                                } else {
                                    format!("{}-item-{}", name.unwrap_or("map"), index)
                                };

                                let child_step_id =
                                    self.inner.execution_ctx.next_operation_id(Some(&item_name));
                                let child_hashed_id = DurableContextImpl::hash_id(&child_step_id);

                                if let Some(child_op) = self
                                    .inner
                                    .execution_ctx
                                    .get_step_data(&child_hashed_id)
                                    .await
                                {
                                    seen_count += 1;

                                    match child_op.status {
                                        OperationStatus::Succeeded => {
                                            if let Some(ref details) = child_op.context_details {
                                                if let Some(ref payload) = details.result {
                                                    let val: TOut = safe_deserialize(
                                                        item_serdes.clone(),
                                                        Some(payload.as_str()),
                                                        &child_hashed_id,
                                                        Some(&item_name),
                                                        &self.inner.execution_ctx,
                                                    )
                                                    .await
                                                    .ok_or_else(|| {
                                                        DurableError::Internal(
                                                            "Missing child context output in replay"
                                                                .to_string(),
                                                        )
                                                    })?;
                                                    successes.push((index, val));
                                                }
                                            }
                                        }
                                        OperationStatus::Failed => {
                                            let msg = child_op
                                                .context_details
                                                .as_ref()
                                                .and_then(|d| d.error.as_ref())
                                                .map(|e| e.error_message.clone())
                                                .unwrap_or_else(|| {
                                                    "Child context failed".to_string()
                                                });
                                            failures.push((
                                                index,
                                                DurableError::ChildContextFailed {
                                                    name: child_step_id,
                                                    message: msg,
                                                    source: None,
                                                },
                                            ));
                                        }
                                        _ => started.push(index),
                                    }
                                }
                            }

                            let completed_count = successes.len() + failures.len();
                            let completion_reason = compute_completion_reason(
                                failures.len(),
                                successes.len(),
                                completed_count,
                            );

                            let mut all = Vec::new();
                            for (i, v) in successes {
                                all.push(BatchItem {
                                    index: i,
                                    status: BatchItemStatus::Succeeded,
                                    result: Some(v),
                                    error: None,
                                });
                            }
                            for (i, e) in failures {
                                all.push(BatchItem {
                                    index: i,
                                    status: BatchItemStatus::Failed,
                                    result: None,
                                    error: Some(Arc::new(e)),
                                });
                            }
                            for i in started {
                                all.push(BatchItem {
                                    index: i,
                                    status: BatchItemStatus::Started,
                                    result: None,
                                    error: None,
                                });
                            }
                            all.sort_by_key(|i| i.index);

                            return Ok(BatchResult {
                                all,
                                completion_reason,
                            });
                        }
                    }
                    _ => {
                        // Incomplete top-level map during replay; continue execution.
                        self.inner
                            .execution_ctx
                            .set_mode(ExecutionMode::Execution)
                            .await;
                    }
                }
            }
        }

        let mut successes: Vec<(usize, TOut)> = Vec::new();
        let mut failures: Vec<(usize, DurableError)> = Vec::new();
        let mut started_indices: HashSet<usize> = HashSet::new();
        let mut success_count = 0usize;
        let mut failure_count = 0usize;
        let mut completed_count = 0usize;

        if items_len == 0 {
            let completion_reason = compute_completion_reason(0, 0, 0);
            let status_str = "SUCCEEDED";
            let reason_str = match completion_reason {
                BatchCompletionReason::AllCompleted => "ALL_COMPLETED",
                BatchCompletionReason::MinSuccessfulReached => "MIN_SUCCESSFUL_REACHED",
                BatchCompletionReason::FailureToleranceExceeded => "FAILURE_TOLERANCE_EXCEEDED",
            };

            let batch_result = BatchResult {
                all: Vec::new(),
                completion_reason,
            };

            let payload = if let Some(batch_serdes) = batch_serdes.clone() {
                safe_serialize_required_with_serdes(
                    batch_serdes,
                    &batch_result,
                    &map_hashed_id,
                    name,
                    &self.inner.execution_ctx,
                )
                .await
            } else {
                safe_serialize(
                    None,
                    Some(&serde_json::json!({
                    "type": "MapResult",
                    "totalCount": 0,
                    "successCount": 0,
                    "failureCount": 0,
                    "completionReason": reason_str,
                    "status": status_str,
                    })),
                    &map_hashed_id,
                    name,
                    &self.inner.execution_ctx,
                )
                .await
                .expect("summary payload must be present")
            };

            let succeed_update = OperationUpdate::builder()
                .id(&map_hashed_id)
                .operation_type(OperationType::Context)
                .sub_type("Map")
                .action(OperationAction::Succeed)
                .payload(payload)
                .build()
                .map_err(|e| {
                    DurableError::Internal(format!("Failed to build map completion update: {e}"))
                })?;
            self.inner
                .execution_ctx
                .checkpoint_manager
                .checkpoint(map_step_id, succeed_update)
                .await?;

            return Ok(batch_result);
        }

        let max_concurrency = cfg
            .max_concurrency
            .unwrap_or_else(|| items_len.max(1))
            .max(1);
        let min_successful = cfg.completion_config.min_successful;

        let mut join_set = tokio::task::JoinSet::new();
        let mut items_iter = items.into_iter().enumerate();

        loop {
            while join_set.len() < max_concurrency && should_continue(failure_count) {
                if let Some(min) = min_successful {
                    if success_count >= min {
                        break;
                    }
                }

                let Some((index, item)) = items_iter.next() else {
                    break;
                };

                let item_name = if let Some(ref namer) = item_namer {
                    namer(&item, index)
                } else {
                    format!("{}-item-{}", name.unwrap_or("map"), index)
                };

                let child_step_id = self.inner.execution_ctx.next_operation_id(Some(&item_name));
                let child_hashed_id = DurableContextImpl::hash_id(&child_step_id);
                started_indices.insert(index);

                let child_cfg = ChildContextConfig::<TOut> {
                    sub_type: Some("MapIteration".to_string()),
                    serdes: item_serdes.clone(),
                    ..Default::default()
                };

                let inner = Arc::clone(&self.inner);
                let map_fn = Arc::clone(&map_fn);
                join_set.spawn(async move {
                    let res = inner
                        .run_in_child_context_with_ids(
                            child_step_id.clone(),
                            child_hashed_id,
                            Some(&item_name),
                            move |child_ctx| map_fn(item, child_ctx, index),
                            Some(child_cfg),
                        )
                        .await;
                    (index, res)
                });
            }

            let Some(joined) = join_set.join_next().await else {
                break;
            };
            let (index, res) = joined
                .map_err(|e| DurableError::Internal(format!("Child task join error: {}", e)))?;

            started_indices.remove(&index);
            completed_count += 1;

            match res {
                Ok(v) => {
                    successes.push((index, v));
                    success_count += 1;
                }
                Err(e) => {
                    failures.push((index, e));
                    failure_count += 1;
                }
            }

            let is_complete = completed_count == items_len
                || min_successful
                    .map(|min| success_count >= min)
                    .unwrap_or(false);

            if is_complete || !should_continue(failure_count) {
                if !started_indices.is_empty() {
                    join_set.abort_all();
                }

                let completion_reason =
                    compute_completion_reason(failure_count, success_count, completed_count);
                let started_count = started_indices.len();
                let total_count = completed_count + started_count;
                let status_str = if failure_count > 0 {
                    "FAILED"
                } else {
                    "SUCCEEDED"
                };
                let reason_str = match completion_reason {
                    BatchCompletionReason::AllCompleted => "ALL_COMPLETED",
                    BatchCompletionReason::MinSuccessfulReached => "MIN_SUCCESSFUL_REACHED",
                    BatchCompletionReason::FailureToleranceExceeded => "FAILURE_TOLERANCE_EXCEEDED",
                };

                let mut all = Vec::new();
                for (i, v) in successes {
                    all.push(BatchItem {
                        index: i,
                        status: BatchItemStatus::Succeeded,
                        result: Some(v),
                        error: None,
                    });
                }
                for (i, e) in failures {
                    all.push(BatchItem {
                        index: i,
                        status: BatchItemStatus::Failed,
                        result: None,
                        error: Some(Arc::new(e)),
                    });
                }
                for i in started_indices {
                    all.push(BatchItem {
                        index: i,
                        status: BatchItemStatus::Started,
                        result: None,
                        error: None,
                    });
                }
                all.sort_by_key(|i| i.index);

                let batch_result = BatchResult {
                    all,
                    completion_reason,
                };

                let payload = if let Some(batch_serdes) = batch_serdes.clone() {
                    safe_serialize_required_with_serdes(
                        batch_serdes,
                        &batch_result,
                        &map_hashed_id,
                        name,
                        &self.inner.execution_ctx,
                    )
                    .await
                } else {
                    safe_serialize(
                        None,
                        Some(&serde_json::json!({
                        "type": "MapResult",
                        "totalCount": total_count,
                        "successCount": success_count,
                        "failureCount": failure_count,
                        "completionReason": reason_str,
                        "status": status_str,
                        })),
                        &map_hashed_id,
                        name,
                        &self.inner.execution_ctx,
                    )
                    .await
                    .expect("summary payload must be present")
                };

                let succeed_update = OperationUpdate::builder()
                    .id(&map_hashed_id)
                    .operation_type(OperationType::Context)
                    .sub_type("Map")
                    .action(OperationAction::Succeed)
                    .payload(payload)
                    .build()
                    .map_err(|e| {
                        DurableError::Internal(format!(
                            "Failed to build map completion update: {e}"
                        ))
                    })?;
                self.inner
                    .execution_ctx
                    .checkpoint_manager
                    .checkpoint(map_step_id.clone(), succeed_update)
                    .await?;

                return Ok(batch_result);
            }
        }

        let completion_reason =
            compute_completion_reason(failure_count, success_count, completed_count);
        let status_str = if failure_count > 0 {
            "FAILED"
        } else {
            "SUCCEEDED"
        };
        let reason_str = match completion_reason {
            BatchCompletionReason::AllCompleted => "ALL_COMPLETED",
            BatchCompletionReason::MinSuccessfulReached => "MIN_SUCCESSFUL_REACHED",
            BatchCompletionReason::FailureToleranceExceeded => "FAILURE_TOLERANCE_EXCEEDED",
        };

        let mut all = Vec::new();
        for (i, v) in successes {
            all.push(BatchItem {
                index: i,
                status: BatchItemStatus::Succeeded,
                result: Some(v),
                error: None,
            });
        }
        for (i, e) in failures {
            all.push(BatchItem {
                index: i,
                status: BatchItemStatus::Failed,
                result: None,
                error: Some(Arc::new(e)),
            });
        }
        all.sort_by_key(|i| i.index);

        let batch_result = BatchResult {
            all,
            completion_reason,
        };

        let payload = if let Some(batch_serdes) = batch_serdes {
            safe_serialize_required_with_serdes(
                batch_serdes,
                &batch_result,
                &map_hashed_id,
                name,
                &self.inner.execution_ctx,
            )
            .await
        } else {
            safe_serialize(
                None,
                Some(&serde_json::json!({
                "type": "MapResult",
                "totalCount": completed_count,
                "successCount": success_count,
                "failureCount": failure_count,
                "completionReason": reason_str,
                "status": status_str,
                })),
                &map_hashed_id,
                name,
                &self.inner.execution_ctx,
            )
            .await
            .expect("summary payload must be present")
        };

        let succeed_update = OperationUpdate::builder()
            .id(&map_hashed_id)
            .operation_type(OperationType::Context)
            .sub_type("Map")
            .action(OperationAction::Succeed)
            .payload(payload)
            .build()
            .map_err(|e| {
                DurableError::Internal(format!("Failed to build map completion update: {e}"))
            })?;
        self.inner
            .execution_ctx
            .checkpoint_manager
            .checkpoint(map_step_id, succeed_update)
            .await?;

        Ok(batch_result)
    }

    /// Execute multiple branches in parallel with deterministic concurrency.
    pub async fn parallel<T, F, Fut>(
        &self,
        name: Option<&str>,
        branches: Vec<F>,
        config: Option<ParallelConfig<T>>,
    ) -> DurableResult<BatchResult<T>>
    where
        T: Serialize + DeserializeOwned + Send + Sync + 'static,
        F: Fn(DurableContextHandle) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = DurableResult<T>> + Send + 'static,
    {
        let named: Vec<NamedParallelBranch<F>> = branches
            .into_iter()
            .map(|func| NamedParallelBranch { name: None, func })
            .collect();
        return self.parallel_named(name, named, config).await;
        /*

        let par_step_id = self.inner.execution_ctx.next_operation_id(name);
        let par_hashed_id = DurableContextImpl::hash_id(&par_step_id);
        if self
            .inner
            .execution_ctx
            .get_step_data(&par_hashed_id)
            .await
            .is_none()
        {
            let parent_id = self.inner.execution_ctx.get_parent_id().await;
            let mut builder = OperationUpdate::builder()
                .id(&par_hashed_id)
                .operation_type(OperationType::Context)
                .sub_type("Parallel")
                .action(OperationAction::Start);
            if let Some(pid) = parent_id {
                builder = builder.parent_id(pid);
            }
            if let Some(n) = name {
                builder = builder.name(n);
            }
            self.inner
                .execution_ctx
                .checkpoint_manager
                .checkpoint(
                    par_step_id.clone(),
                    builder.build().map_err(|e| {
                        DurableError::Internal(format!(
                            "Failed to build parallel START update: {e}"
                        ))
                    })?,
                )
                .await?;
        }

        // Replay handling: if the top-level parallel completed, reconstruct children and skip incomplete ones.
        if mode == ExecutionMode::Replay {
            if let Some(op) = self.inner.execution_ctx.get_step_data(&par_hashed_id).await {
                match op.status {
                    OperationStatus::Failed => {
                        let msg = op
                            .context_details
                            .as_ref()
                            .and_then(|d| d.error.as_ref())
                            .map(|e| e.error_message.clone())
                            .unwrap_or_else(|| "Batch operation failed".to_string());
                        return Err(DurableError::BatchOperationFailed {
                            name: name.unwrap_or("parallel").to_string(),
                            message: msg,
                            successful_count: 0,
                            failed_count: 0,
                        });
                    }
                    OperationStatus::Succeeded => {
                        let target_total_count = op
                            .context_details
                            .as_ref()
                            .and_then(|d| d.result.as_ref())
                            .and_then(|payload| {
                                serde_json::from_str::<serde_json::Value>(payload)
                                    .ok()
                                    .and_then(|v| v.get("totalCount").and_then(|tc| tc.as_u64()))
                            })
                            .map(|tc| tc as usize);

                        if let Some(target_total_count) = target_total_count {
                            let mut successes = Vec::new();
                            let mut failures = Vec::new();
                            let mut completed_count = 0usize;

                            for (index, _branch) in branches.into_iter().enumerate() {
                                if completed_count >= target_total_count {
                                    break;
                                }

                                let branch_name =
                                    format!("{}-branch-{}", name.unwrap_or("parallel"), index);
                                let child_step_id = self
                                    .inner
                                    .execution_ctx
                                    .next_operation_id(Some(&branch_name));
                                let child_hashed_id = DurableContextImpl::hash_id(&child_step_id);

                                if let Some(child_op) = self
                                    .inner
                                    .execution_ctx
                                    .get_step_data(&child_hashed_id)
                                    .await
                                {
                                    match child_op.status {
                                        OperationStatus::Succeeded => {
                                            if let Some(ref details) = child_op.context_details {
                                                if let Some(ref payload) = details.result {
                                                    let val: T = safe_deserialize(
                                                        item_serdes.clone(),
                                                        Some(payload.as_str()),
                                                        &child_hashed_id,
                                                        Some(&branch_name),
                                                        &self.inner.execution_ctx,
                                                    )
                                                    .await
                                                    .ok_or_else(|| {
                                                        DurableError::Internal(
                                                            "Missing child context output in replay"
                                                                .to_string(),
                                                        )
                                                    })?;
                                                    successes.push((index, val));
                                                    completed_count += 1;
                                                }
                                            }
                                        }
                                        OperationStatus::Failed => {
                                            let msg = child_op
                                                .context_details
                                                .as_ref()
                                                .and_then(|d| d.error.as_ref())
                                                .map(|e| e.error_message.clone())
                                                .unwrap_or_else(|| {
                                                    "Child context failed".to_string()
                                                });
                                            failures.push((
                                                index,
                                                DurableError::ChildContextFailed {
                                                    name: child_step_id,
                                                    message: msg,
                                                    source: None,
                                                },
                                            ));
                                            completed_count += 1;
                                        }
                                        _ => continue,
                                    }
                                } else {
                                    continue;
                                }
                            }

                            let completion_reason = if target_total_count < branches_len {
                                BatchCompletionReason::MinSuccessfulReached
                            } else {
                                BatchCompletionReason::AllCompleted
                            };

                            let mut all = Vec::new();
                            for (i, v) in successes {
                                all.push(BatchItem {
                                    index: i,
                                    status: BatchItemStatus::Succeeded,
                                    result: Some(v),
                                    error: None,
                                });
                            }
                            for (i, e) in failures {
                                all.push(BatchItem {
                                    index: i,
                                    status: BatchItemStatus::Failed,
                                    result: None,
                                    error: Some(Arc::new(e)),
                                });
                            }
                            all.sort_by_key(|i| i.index);

                            return Ok(BatchResult {
                                all,
                                completion_reason,
                            });
                        }
                    }
                    _ => {
                        self.inner
                            .execution_ctx
                            .set_mode(ExecutionMode::Execution)
                            .await;
                    }
                }
            }
        }

        let mut successes = Vec::new();
        let mut failures = Vec::new();
        let mut success_count = 0usize;
        let mut failure_count = 0usize;

        let max_concurrency = cfg
            .max_concurrency
            .unwrap_or_else(|| branches_len.max(1))
            .max(1);
        let min_successful = cfg.completion_config.min_successful;
        let mut stop_starting = false;
        let mut join_set = tokio::task::JoinSet::new();
        let mut branches_iter = branches.into_iter().enumerate();

        loop {
            while !stop_starting && join_set.len() < max_concurrency {
                if let Some(min) = min_successful {
                    if success_count >= min {
                        stop_starting = true;
                        break;
                    }
                }

                let Some((index, branch)) = branches_iter.next() else {
                    break;
                };

                let branch_name = format!("{}-branch-{}", name.unwrap_or("parallel"), index);
                let child_step_id = self
                    .inner
                    .execution_ctx
                    .next_operation_id(Some(&branch_name));
                let child_hashed_id = DurableContextImpl::hash_id(&child_step_id);

                let child_cfg = ChildContextConfig::<T> {
                    sub_type: Some("ParallelBranch".to_string()),
                    serdes: item_serdes.clone(),
                    ..Default::default()
                };

                let inner = Arc::clone(&self.inner);
                join_set.spawn(async move {
                    let res = inner
                        .run_in_child_context_with_ids(
                            child_step_id.clone(),
                            child_hashed_id,
                            Some(&branch_name),
                            move |child_ctx| branch(child_ctx),
                            Some(child_cfg),
                        )
                        .await;
                    (index, res)
                });
            }

            let Some(joined) = join_set.join_next().await else {
                break;
            };
            let (index, res) = joined
                .map_err(|e| DurableError::Internal(format!("Child task join error: {}", e)))?;

            match res {
                Ok(v) => {
                    successes.push((index, v));
                    success_count += 1;
                    if let Some(min) = min_successful {
                        if success_count >= min {
                            stop_starting = true;
                        }
                    }
                }
                Err(e) => {
                    failures.push((index, e));
                    failure_count += 1;

                    if !has_any_completion_criteria {
                        join_set.abort_all();
                        while join_set.join_next().await.is_some() {}
                        let error = DurableError::BatchOperationFailed {
                            name: name.unwrap_or("parallel").to_string(),
                            message: "Failure tolerance exceeded".to_string(),
                            successful_count: success_count,
                            failed_count: failure_count,
                        };
                        let err_obj = ErrorObject::from_durable_error(&error);
                        let fail_update = OperationUpdate::builder()
                            .id(&par_hashed_id)
                            .operation_type(OperationType::Context)
                            .sub_type("Parallel")
                            .action(OperationAction::Fail)
                            .error(err_obj)
                            .build()
                            .unwrap();
                        self.inner
                            .execution_ctx
                            .checkpoint_manager
                            .checkpoint(par_step_id.clone(), fail_update)
                            .await?;
                        return Err(error);
                    }

                    if let Some(tol) = cfg.completion_config.tolerated_failure_count {
                        if failure_count > tol {
                            join_set.abort_all();
                            while join_set.join_next().await.is_some() {}
                            let error = DurableError::BatchOperationFailed {
                                name: name.unwrap_or("parallel").to_string(),
                                message: "Failure tolerance exceeded".to_string(),
                                successful_count: success_count,
                                failed_count: failure_count,
                            };
                            let err_obj = ErrorObject::from_durable_error(&error);
                            let fail_update = OperationUpdate::builder()
                                .id(&par_hashed_id)
                                .operation_type(OperationType::Context)
                                .sub_type("Parallel")
                                .action(OperationAction::Fail)
                                .error(err_obj)
                                .build()
                                .unwrap();
                            self.inner
                                .execution_ctx
                                .checkpoint_manager
                                .checkpoint(par_step_id.clone(), fail_update)
                                .await?;
                            return Err(error);
                        }
                    }
                    if let Some(pct) = cfg.completion_config.tolerated_failure_percentage {
                        if branches_len > 0 {
                            let failure_pct = (failure_count as f64 / branches_len as f64) * 100.0;
                            if failure_pct > pct {
                                join_set.abort_all();
                                while join_set.join_next().await.is_some() {}
                                let error = DurableError::BatchOperationFailed {
                                    name: name.unwrap_or("parallel").to_string(),
                                    message: "Failure tolerance exceeded".to_string(),
                                    successful_count: success_count,
                                    failed_count: failure_count,
                                };
                                let err_obj = ErrorObject::from_durable_error(&error);
                                let fail_update = OperationUpdate::builder()
                                    .id(&par_hashed_id)
                                    .operation_type(OperationType::Context)
                                    .sub_type("Parallel")
                                    .action(OperationAction::Fail)
                                    .error(err_obj)
                                    .build()
                                    .unwrap();
                                self.inner
                                    .execution_ctx
                                    .checkpoint_manager
                                    .checkpoint(par_step_id.clone(), fail_update)
                                    .await?;
                                return Err(error);
                            }
                        }
                    }
                }
            }
        }

        let summary = serde_json::to_string(&serde_json::json!({
            "totalCount": success_count + failure_count,
            "successCount": success_count,
            "failureCount": failure_count,
        }))
        .unwrap();

        let succeed_update = OperationUpdate::builder()
            .id(&par_hashed_id)
            .operation_type(OperationType::Context)
            .sub_type("Parallel")
            .action(OperationAction::Succeed)
            .payload(summary)
            .build()
            .unwrap();
        self.inner
            .execution_ctx
            .checkpoint_manager
            .checkpoint(par_step_id, succeed_update)
            .await?;

        let completion_reason = if cfg.completion_config.min_successful.is_some()
            && (success_count + failure_count) < branches_len
        {
            BatchCompletionReason::MinSuccessfulReached
        } else {
            BatchCompletionReason::AllCompleted
        };

        let mut all = Vec::new();
        for (i, v) in successes {
            all.push(BatchItem {
                index: i,
                status: BatchItemStatus::Succeeded,
                result: Some(v),
                error: None,
            });
        }
        for (i, e) in failures {
            all.push(BatchItem {
                index: i,
                status: BatchItemStatus::Failed,
                result: None,
                error: Some(Arc::new(e)),
            });
        }
        all.sort_by_key(|i| i.index);

        Ok(BatchResult {
            all,
            completion_reason,
        })
        */
    }

    /// Execute multiple branches in parallel, allowing optional per-branch names.
    pub async fn parallel_named<T, F, Fut>(
        &self,
        name: Option<&str>,
        branches: Vec<NamedParallelBranch<F>>,
        config: Option<ParallelConfig<T>>,
    ) -> DurableResult<BatchResult<T>>
    where
        T: Serialize + DeserializeOwned + Send + Sync + 'static,
        F: Fn(DurableContextHandle) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = DurableResult<T>> + Send + 'static,
    {
        let cfg = config.unwrap_or_default();
        let branches_len = branches.len();
        validate_completion_config(
            &cfg.completion_config,
            branches_len,
            name.unwrap_or("parallel"),
        )?;
        let mode = self.inner.execution_ctx.get_mode().await;
        let item_serdes = cfg.item_serdes.clone();
        let batch_serdes = cfg.serdes.clone();

        let has_any_completion_criteria = cfg.completion_config.min_successful.is_some()
            || cfg.completion_config.tolerated_failure_count.is_some()
            || cfg.completion_config.tolerated_failure_percentage.is_some();

        let should_continue = |failure_count: usize| -> bool {
            if !has_any_completion_criteria {
                return failure_count == 0;
            }
            if let Some(tol) = cfg.completion_config.tolerated_failure_count {
                if failure_count > tol {
                    return false;
                }
            }
            if let Some(pct) = cfg.completion_config.tolerated_failure_percentage {
                if branches_len > 0 {
                    let failure_pct = (failure_count as f64 / branches_len as f64) * 100.0;
                    if failure_pct > pct {
                        return false;
                    }
                }
            }
            true
        };

        let compute_completion_reason =
            |failure_count: usize, success_count: usize, completed_count: usize| {
                if !should_continue(failure_count) {
                    BatchCompletionReason::FailureToleranceExceeded
                } else if completed_count == branches_len {
                    BatchCompletionReason::AllCompleted
                } else if let Some(min) = cfg.completion_config.min_successful {
                    if success_count >= min {
                        BatchCompletionReason::MinSuccessfulReached
                    } else {
                        BatchCompletionReason::AllCompleted
                    }
                } else {
                    BatchCompletionReason::AllCompleted
                }
            };

        // Start top-level PARALLEL context.
        let par_step_id = self.inner.execution_ctx.next_operation_id(name);
        let par_hashed_id = DurableContextImpl::hash_id(&par_step_id);
        if self
            .inner
            .execution_ctx
            .get_step_data(&par_hashed_id)
            .await
            .is_none()
        {
            let parent_id = self.inner.execution_ctx.get_parent_id().await;
            let mut builder = OperationUpdate::builder()
                .id(&par_hashed_id)
                .operation_type(OperationType::Context)
                .sub_type("Parallel")
                .action(OperationAction::Start);
            if let Some(pid) = parent_id {
                builder = builder.parent_id(pid);
            }
            if let Some(n) = name {
                builder = builder.name(n);
            }
            self.inner
                .execution_ctx
                .checkpoint_manager
                .checkpoint(
                    par_step_id.clone(),
                    builder.build().map_err(|e| {
                        DurableError::Internal(format!(
                            "Failed to build parallel START update: {e}"
                        ))
                    })?,
                )
                .await?;
        }

        // Replay handling: reconstruct completed branches only.
        if mode == ExecutionMode::Replay {
            if let Some(op) = self.inner.execution_ctx.get_step_data(&par_hashed_id).await {
                match op.status {
                    OperationStatus::Failed => {
                        let msg = op
                            .context_details
                            .as_ref()
                            .and_then(|d| d.error.as_ref())
                            .map(|e| e.error_message.clone())
                            .unwrap_or_else(|| "Batch operation failed".to_string());
                        return Err(DurableError::BatchOperationFailed {
                            name: name.unwrap_or("parallel").to_string(),
                            message: msg,
                            successful_count: 0,
                            failed_count: 0,
                        });
                    }
                    OperationStatus::Succeeded => {
                        if let Some(payload) =
                            op.context_details.as_ref().and_then(|d| d.result.as_ref())
                        {
                            if let Some(batch_serdes) = batch_serdes.clone() {
                                let batch: BatchResult<T> = safe_deserialize_required_with_serdes(
                                    batch_serdes,
                                    payload,
                                    &par_hashed_id,
                                    name,
                                    &self.inner.execution_ctx,
                                )
                                .await;

                                let target_total_count = batch
                                    .all
                                    .iter()
                                    .map(|i| i.index)
                                    .max()
                                    .map(|m| m + 1)
                                    .unwrap_or(0);

                                if target_total_count > branches_len {
                                    return Err(DurableError::ReplayValidationFailed {
                                        expected: format!(
                                            "parallel totalCount <= {}",
                                            branches_len
                                        ),
                                        actual: target_total_count.to_string(),
                                    });
                                }

                                // Consume child context operation IDs to keep the parent context counter in sync.
                                for (index, branch) in
                                    branches.iter().enumerate().take(target_total_count)
                                {
                                    let base = name.unwrap_or("parallel");
                                    let branch_name = branch
                                        .name
                                        .clone()
                                        .unwrap_or_else(|| format!("{}-branch-{}", base, index));
                                    let _ = self
                                        .inner
                                        .execution_ctx
                                        .next_operation_id(Some(&branch_name));
                                }

                                return Ok(batch);
                            }
                        }

                        let target_total_count = op
                            .context_details
                            .as_ref()
                            .and_then(|d| d.result.as_ref())
                            .and_then(|payload| {
                                serde_json::from_str::<serde_json::Value>(payload)
                                    .ok()
                                    .and_then(|v| v.get("totalCount").and_then(|tc| tc.as_u64()))
                            })
                            .map(|tc| tc as usize);

                        if let Some(target_total_count) = target_total_count {
                            let mut successes = Vec::new();
                            let mut failures = Vec::new();
                            let mut started = Vec::new();
                            let mut seen_count = 0usize;

                            for (index, branch) in branches.iter().enumerate() {
                                if seen_count >= target_total_count {
                                    break;
                                }

                                let base = name.unwrap_or("parallel");
                                let branch_name = branch
                                    .name
                                    .clone()
                                    .unwrap_or_else(|| format!("{}-branch-{}", base, index));

                                let child_step_id = self
                                    .inner
                                    .execution_ctx
                                    .next_operation_id(Some(&branch_name));
                                let child_hashed_id = DurableContextImpl::hash_id(&child_step_id);

                                if let Some(child_op) = self
                                    .inner
                                    .execution_ctx
                                    .get_step_data(&child_hashed_id)
                                    .await
                                {
                                    seen_count += 1;

                                    match child_op.status {
                                        OperationStatus::Succeeded => {
                                            if let Some(ref details) = child_op.context_details {
                                                if let Some(ref payload) = details.result {
                                                    let val: T = safe_deserialize(
                                                        item_serdes.clone(),
                                                        Some(payload.as_str()),
                                                        &child_hashed_id,
                                                        Some(&branch_name),
                                                        &self.inner.execution_ctx,
                                                    )
                                                    .await
                                                    .ok_or_else(|| {
                                                        DurableError::Internal(
                                                            "Missing child context output in replay"
                                                                .to_string(),
                                                        )
                                                    })?;
                                                    successes.push((index, val));
                                                }
                                            }
                                        }
                                        OperationStatus::Failed => {
                                            let msg = child_op
                                                .context_details
                                                .as_ref()
                                                .and_then(|d| d.error.as_ref())
                                                .map(|e| e.error_message.clone())
                                                .unwrap_or_else(|| {
                                                    "Child context failed".to_string()
                                                });
                                            failures.push((
                                                index,
                                                DurableError::ChildContextFailed {
                                                    name: child_step_id,
                                                    message: msg,
                                                    source: None,
                                                },
                                            ));
                                        }
                                        _ => started.push(index),
                                    }
                                }
                            }

                            let completed_count = successes.len() + failures.len();
                            let completion_reason = compute_completion_reason(
                                failures.len(),
                                successes.len(),
                                completed_count,
                            );

                            let mut all = Vec::new();
                            for (i, v) in successes {
                                all.push(BatchItem {
                                    index: i,
                                    status: BatchItemStatus::Succeeded,
                                    result: Some(v),
                                    error: None,
                                });
                            }
                            for (i, e) in failures {
                                all.push(BatchItem {
                                    index: i,
                                    status: BatchItemStatus::Failed,
                                    result: None,
                                    error: Some(Arc::new(e)),
                                });
                            }
                            for i in started {
                                all.push(BatchItem {
                                    index: i,
                                    status: BatchItemStatus::Started,
                                    result: None,
                                    error: None,
                                });
                            }
                            all.sort_by_key(|i| i.index);

                            return Ok(BatchResult {
                                all,
                                completion_reason,
                            });
                        }
                    }
                    _ => {
                        self.inner
                            .execution_ctx
                            .set_mode(ExecutionMode::Execution)
                            .await;
                    }
                }
            }
        }

        // Execution mode: run branches with deterministic concurrency and early completion.
        let mut successes: Vec<(usize, T)> = Vec::new();
        let mut failures: Vec<(usize, DurableError)> = Vec::new();
        let mut started_indices: HashSet<usize> = HashSet::new();
        let mut success_count = 0usize;
        let mut failure_count = 0usize;
        let mut completed_count = 0usize;

        let max_concurrency = cfg
            .max_concurrency
            .unwrap_or_else(|| branches_len.max(1))
            .max(1);
        let min_successful = cfg.completion_config.min_successful;

        let mut join_set = tokio::task::JoinSet::new();
        let mut branches_iter = branches.into_iter().enumerate();

        loop {
            while join_set.len() < max_concurrency && should_continue(failure_count) {
                if let Some(min) = min_successful {
                    if success_count >= min {
                        break;
                    }
                }

                let Some((index, branch)) = branches_iter.next() else {
                    break;
                };

                let base = name.unwrap_or("parallel");
                let branch_name = branch
                    .name
                    .unwrap_or_else(|| format!("{}-branch-{}", base, index));

                let child_step_id = self
                    .inner
                    .execution_ctx
                    .next_operation_id(Some(&branch_name));
                let child_hashed_id = DurableContextImpl::hash_id(&child_step_id);
                started_indices.insert(index);

                let child_cfg = ChildContextConfig::<T> {
                    sub_type: Some("ParallelBranch".to_string()),
                    serdes: item_serdes.clone(),
                    ..Default::default()
                };

                let inner = Arc::clone(&self.inner);
                join_set.spawn(async move {
                    let res = inner
                        .run_in_child_context_with_ids(
                            child_step_id.clone(),
                            child_hashed_id,
                            Some(&branch_name),
                            move |child_ctx| (branch.func)(child_ctx),
                            Some(child_cfg),
                        )
                        .await;
                    (index, res)
                });
            }

            let Some(joined) = join_set.join_next().await else {
                break;
            };
            let (index, res) = joined
                .map_err(|e| DurableError::Internal(format!("Child task join error: {}", e)))?;

            started_indices.remove(&index);
            completed_count += 1;

            match res {
                Ok(v) => {
                    successes.push((index, v));
                    success_count += 1;
                }
                Err(e) => {
                    failures.push((index, e));
                    failure_count += 1;
                }
            }

            let is_complete = completed_count == branches_len
                || min_successful
                    .map(|min| success_count >= min)
                    .unwrap_or(false);

            if is_complete || !should_continue(failure_count) {
                if !started_indices.is_empty() {
                    join_set.abort_all();
                }

                let completion_reason =
                    compute_completion_reason(failure_count, success_count, completed_count);
                let started_count = started_indices.len();
                let total_count = completed_count + started_count;
                let status_str = if failure_count > 0 {
                    "FAILED"
                } else {
                    "SUCCEEDED"
                };
                let reason_str = match completion_reason {
                    BatchCompletionReason::AllCompleted => "ALL_COMPLETED",
                    BatchCompletionReason::MinSuccessfulReached => "MIN_SUCCESSFUL_REACHED",
                    BatchCompletionReason::FailureToleranceExceeded => "FAILURE_TOLERANCE_EXCEEDED",
                };

                let mut all = Vec::new();
                for (i, v) in successes {
                    all.push(BatchItem {
                        index: i,
                        status: BatchItemStatus::Succeeded,
                        result: Some(v),
                        error: None,
                    });
                }
                for (i, e) in failures {
                    all.push(BatchItem {
                        index: i,
                        status: BatchItemStatus::Failed,
                        result: None,
                        error: Some(Arc::new(e)),
                    });
                }
                for i in started_indices {
                    all.push(BatchItem {
                        index: i,
                        status: BatchItemStatus::Started,
                        result: None,
                        error: None,
                    });
                }
                all.sort_by_key(|i| i.index);

                let batch_result = BatchResult {
                    all,
                    completion_reason,
                };

                let payload = if let Some(batch_serdes) = batch_serdes.clone() {
                    safe_serialize_required_with_serdes(
                        batch_serdes,
                        &batch_result,
                        &par_hashed_id,
                        name,
                        &self.inner.execution_ctx,
                    )
                    .await
                } else {
                    safe_serialize(
                        None,
                        Some(&serde_json::json!({
                        "type": "ParallelResult",
                        "totalCount": total_count,
                        "successCount": success_count,
                        "failureCount": failure_count,
                        "startedCount": started_count,
                        "completionReason": reason_str,
                        "status": status_str,
                        })),
                        &par_hashed_id,
                        name,
                        &self.inner.execution_ctx,
                    )
                    .await
                    .expect("summary payload must be present")
                };

                let succeed_update = OperationUpdate::builder()
                    .id(&par_hashed_id)
                    .operation_type(OperationType::Context)
                    .sub_type("Parallel")
                    .action(OperationAction::Succeed)
                    .payload(payload)
                    .build()
                    .map_err(|e| {
                        DurableError::Internal(format!(
                            "Failed to build parallel completion update: {e}"
                        ))
                    })?;
                self.inner
                    .execution_ctx
                    .checkpoint_manager
                    .checkpoint(par_step_id.clone(), succeed_update)
                    .await?;

                return Ok(batch_result);
            }
        }

        let completion_reason =
            compute_completion_reason(failure_count, success_count, completed_count);
        let status_str = if failure_count > 0 {
            "FAILED"
        } else {
            "SUCCEEDED"
        };
        let reason_str = match completion_reason {
            BatchCompletionReason::AllCompleted => "ALL_COMPLETED",
            BatchCompletionReason::MinSuccessfulReached => "MIN_SUCCESSFUL_REACHED",
            BatchCompletionReason::FailureToleranceExceeded => "FAILURE_TOLERANCE_EXCEEDED",
        };

        let mut all = Vec::new();
        for (i, v) in successes {
            all.push(BatchItem {
                index: i,
                status: BatchItemStatus::Succeeded,
                result: Some(v),
                error: None,
            });
        }
        for (i, e) in failures {
            all.push(BatchItem {
                index: i,
                status: BatchItemStatus::Failed,
                result: None,
                error: Some(Arc::new(e)),
            });
        }
        all.sort_by_key(|i| i.index);

        let batch_result = BatchResult {
            all,
            completion_reason,
        };

        let payload = if let Some(batch_serdes) = batch_serdes {
            safe_serialize_required_with_serdes(
                batch_serdes,
                &batch_result,
                &par_hashed_id,
                name,
                &self.inner.execution_ctx,
            )
            .await
        } else {
            safe_serialize(
                None,
                Some(&serde_json::json!({
                "type": "ParallelResult",
                "totalCount": completed_count,
                "successCount": success_count,
                "failureCount": failure_count,
                "startedCount": 0,
                "completionReason": reason_str,
                "status": status_str,
                })),
                &par_hashed_id,
                name,
                &self.inner.execution_ctx,
            )
            .await
            .expect("summary payload must be present")
        };

        let succeed_update = OperationUpdate::builder()
            .id(&par_hashed_id)
            .operation_type(OperationType::Context)
            .sub_type("Parallel")
            .action(OperationAction::Succeed)
            .payload(payload)
            .build()
            .map_err(|e| {
                DurableError::Internal(format!("Failed to build parallel completion update: {e}"))
            })?;
        self.inner
            .execution_ctx
            .checkpoint_manager
            .checkpoint(par_step_id, succeed_update)
            .await?;

        Ok(batch_result)
    }

    /// Wait for a condition by periodically re-running a check function.
    ///
    /// This is equivalent to the JS SDK `waitForCondition`. Each attempt runs as a durable
    /// step with subtype `WaitForCondition`, checkpointing the intermediate state and
    /// scheduling a retry according to `wait_strategy`.
    pub async fn wait_for_condition<T, F, Fut>(
        &self,
        name: Option<&str>,
        check_fn: F,
        config: WaitConditionConfig<T>,
    ) -> DurableResult<T>
    where
        T: Serialize + DeserializeOwned + Send + Sync + Clone + 'static,
        F: Fn(T, StepContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = DurableResult<T>> + Send + 'static,
    {
        let check_fn = Arc::new(check_fn);
        let serdes = config.serdes.clone();

        let step_id = self.inner.execution_ctx.next_operation_id(name);
        let hashed_id = DurableContextImpl::hash_id(&step_id);

        // Replay short-circuit.
        if let Some(operation) = self.inner.execution_ctx.get_step_data(&hashed_id).await {
            match operation.status {
                OperationStatus::Succeeded => {
                    if let Some(details) = operation.step_details {
                        if let Some(payload) = details.result {
                            if let Some(val) = safe_deserialize(
                                serdes.clone(),
                                Some(payload.as_str()),
                                &hashed_id,
                                name,
                                &self.inner.execution_ctx,
                            )
                            .await
                            {
                                return Ok(val);
                            }
                        }
                    }
                    return Err(DurableError::Internal(
                        "Missing wait-for-condition result in replay".to_string(),
                    ));
                }
                OperationStatus::Failed => {
                    let attempts = operation
                        .step_details
                        .as_ref()
                        .and_then(|d| d.attempt)
                        .unwrap_or(1);
                    let msg = operation
                        .step_details
                        .as_ref()
                        .and_then(|d| d.error.as_ref())
                        .map(|e| e.error_message.clone())
                        .unwrap_or_else(|| "Wait for condition failed".to_string());
                    return Err(DurableError::step_failed_msg(step_id, attempts, msg));
                }
                _ => {
                    self.inner
                        .execution_ctx
                        .set_mode(ExecutionMode::Execution)
                        .await;
                }
            }
        }

        // Get current state from replay if present.
        let mut state = if let Some(op) = self.inner.execution_ctx.get_step_data(&hashed_id).await {
            if let Some(details) = op.step_details {
                if let Some(payload) = details.result {
                    safe_deserialize(
                        serdes.clone(),
                        Some(payload.as_str()),
                        &hashed_id,
                        name,
                        &self.inner.execution_ctx,
                    )
                    .await
                    .unwrap_or_else(|| config.initial_state.clone())
                } else {
                    config.initial_state.clone()
                }
            } else {
                config.initial_state.clone()
            }
        } else {
            config.initial_state.clone()
        };

        let attempt = self
            .inner
            .execution_ctx
            .get_step_data(&hashed_id)
            .await
            .and_then(|op| op.step_details.and_then(|d| d.attempt))
            .unwrap_or(0)
            + 1;

        if let Some(max) = config.max_attempts {
            if attempt > max {
                let error = DurableError::WaitConditionExceeded {
                    name: name.unwrap_or("wait_for_condition").to_string(),
                    attempts: attempt,
                };
                let err_obj = ErrorObject::from_durable_error(&error);
                let fail_update = OperationUpdate::builder()
                    .id(&hashed_id)
                    .operation_type(OperationType::Step)
                    .sub_type("WaitForCondition")
                    .action(OperationAction::Fail)
                    .error(err_obj)
                    .build()
                    .map_err(|e| {
                        DurableError::Internal(format!(
                            "Failed to build wait_for_condition FAIL update: {e}"
                        ))
                    })?;
                self.inner
                    .execution_ctx
                    .checkpoint_manager
                    .checkpoint(step_id.clone(), fail_update)
                    .await?;
                return Err(error);
            }
        }

        // Start step if not already started.
        if self
            .inner
            .execution_ctx
            .get_step_data(&hashed_id)
            .await
            .is_none()
        {
            let parent_id = self.inner.execution_ctx.get_parent_id().await;
            let mut builder = OperationUpdate::builder()
                .id(&hashed_id)
                .operation_type(OperationType::Step)
                .sub_type("WaitForCondition")
                .action(OperationAction::Start);
            if let Some(pid) = parent_id {
                builder = builder.parent_id(pid);
            }
            if let Some(n) = name {
                builder = builder.name(n);
            }
            self.inner
                .execution_ctx
                .checkpoint_manager
                .checkpoint(
                    step_id.clone(),
                    builder.build().map_err(|e| {
                        DurableError::Internal(format!(
                            "Failed to build wait_for_condition START update: {e}"
                        ))
                    })?,
                )
                .await?;
        }

        let mode_now = self.inner.execution_ctx.get_mode().await;
        let step_ctx = StepContext::new_with_logger(
            name.map(String::from),
            hashed_id.clone(),
            self.inner.execution_ctx.durable_execution_arn.clone(),
            self.inner.execution_ctx.logger.clone(),
            mode_now,
            self.inner.execution_ctx.mode_aware_logging,
            None,
        );
        let new_state = check_fn(state, step_ctx).await?;
        state = new_state;

        let payload = safe_serialize(
            serdes.clone(),
            Some(&state),
            &hashed_id,
            name,
            &self.inner.execution_ctx,
        )
        .await;

        match (config.wait_strategy)(&state, attempt) {
            WaitConditionDecision::Stop => {
                let mut succeed_builder = OperationUpdate::builder()
                    .id(&hashed_id)
                    .operation_type(OperationType::Step)
                    .sub_type("WaitForCondition")
                    .action(OperationAction::Succeed);
                if let Some(p) = payload.clone() {
                    succeed_builder = succeed_builder.payload(p);
                }
                let succeed_update = succeed_builder.build().map_err(|e| {
                    DurableError::Internal(format!(
                        "Failed to build wait_for_condition SUCCEED update: {e}"
                    ))
                })?;
                self.inner
                    .execution_ctx
                    .checkpoint_manager
                    .checkpoint(step_id, succeed_update)
                    .await?;

                Ok(state)
            }
            WaitConditionDecision::Continue { delay } => {
                let mut retry_builder = OperationUpdate::builder()
                    .id(&hashed_id)
                    .operation_type(OperationType::Step)
                    .sub_type("WaitForCondition")
                    .action(OperationAction::Retry)
                    .step_options(crate::types::StepUpdateOptions {
                        next_attempt_delay_seconds: Some(delay.to_seconds_i32_saturating()),
                    });
                if let Some(p) = payload {
                    retry_builder = retry_builder.payload(p);
                }
                let retry_update = retry_builder.build().map_err(|e| {
                    DurableError::Internal(format!(
                        "Failed to build wait_for_condition RETRY update: {e}"
                    ))
                })?;
                self.inner
                    .execution_ctx
                    .checkpoint_manager
                    .checkpoint(step_id.clone(), retry_update)
                    .await?;

                self.inner
                    .execution_ctx
                    .termination_manager
                    .terminate_for_retry()
                    .await;

                std::future::pending::<()>().await;
                unreachable!()
            }
        }
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

#[derive(Debug)]
struct StepInterruptedError {
    step_id: String,
    name: Option<String>,
}

impl std::fmt::Display for StepInterruptedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.name.as_deref() {
            Some(name) => write!(f, "Step interrupted: {} ({})", name, self.step_id),
            None => write!(f, "Step interrupted: {}", self.step_id),
        }
    }
}

impl std::error::Error for StepInterruptedError {}

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

    /// Execute a step.
    pub async fn step<T, F, Fut>(
        &self,
        name: Option<&str>,
        step_fn: F,
        config: Option<StepConfig<T>>,
    ) -> DurableResult<T>
    where
        T: Serialize + DeserializeOwned + Send + Sync + 'static,
        F: FnOnce(StepContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, Box<dyn std::error::Error + Send + Sync>>> + Send + 'static,
    {
        let step_id = self.execution_ctx.next_operation_id(name);
        let hashed_id = Self::hash_id(&step_id);
        let config = config.unwrap_or_default();
        let semantics = config.semantics;
        let retry_strategy = config.retry_strategy.unwrap_or_else(|| presets::default());
        let serdes = config.serdes.clone();

        let parent_id = self.execution_ctx.get_parent_id().await;
        let operation = self.execution_ctx.get_step_data(&hashed_id).await;

        // Replay handling: short-circuit completed, surface failures, or suspend.
        if let Some(op) = operation.as_ref() {
            match op.status {
                OperationStatus::Succeeded => {
                    if let Some(ref details) = op.step_details {
                        if let Some(ref payload) = details.result {
                            if let Some(val) = safe_deserialize(
                                serdes,
                                Some(payload.as_str()),
                                &hashed_id,
                                name,
                                &self.execution_ctx,
                            )
                            .await
                            {
                                return Ok(val);
                            }
                        }
                    }
                    return Err(DurableError::Internal(
                        "Missing step output in replay".to_string(),
                    ));
                }
                OperationStatus::Failed => {
                    let attempt_idx = op
                        .step_details
                        .as_ref()
                        .and_then(|d| d.attempt)
                        .unwrap_or(0);
                    let attempts = attempt_idx + 1;
                    let message = op
                        .step_details
                        .as_ref()
                        .and_then(|d| d.error.as_ref())
                        .map(|e| e.error_message.clone())
                        .unwrap_or_else(|| "Replayed failure".to_string());
                    return Err(DurableError::step_failed_msg(step_id, attempts, message));
                }
                OperationStatus::Pending => {
                    // Retry is scheduled; suspend until the backend advances state.
                    self.execution_ctx.set_mode(ExecutionMode::Execution).await;
                    self.execution_ctx
                        .termination_manager
                        .terminate_for_retry()
                        .await;
                    std::future::pending::<()>().await;
                    unreachable!()
                }
                OperationStatus::Started if semantics == StepSemantics::AtMostOncePerRetry => {
                    // Interrupted step in at-most-once semantics: treat as a failure and schedule retry.
                    self.execution_ctx.set_mode(ExecutionMode::Execution).await;

                    let attempt_idx = op
                        .step_details
                        .as_ref()
                        .and_then(|d| d.attempt)
                        .unwrap_or(0);
                    let attempts_made = attempt_idx + 1;
                    let interrupted = StepInterruptedError {
                        step_id: step_id.clone(),
                        name: name.map(|s| s.to_string()),
                    };

                    let decision = retry_strategy.should_retry(&interrupted, attempts_made);
                    let err_obj = ErrorObject::from_error(&interrupted);

                    if !decision.should_retry {
                        let mut builder = OperationUpdate::builder()
                            .id(&hashed_id)
                            .operation_type(OperationType::Step)
                            .sub_type("Step")
                            .action(OperationAction::Fail)
                            .error(err_obj);
                        if let Some(ref pid) = parent_id {
                            builder = builder.parent_id(pid);
                        }
                        if let Some(n) = name {
                            builder = builder.name(n);
                        }
                        self.execution_ctx
                            .checkpoint_manager
                            .checkpoint(
                                step_id.clone(),
                                builder.build().map_err(|e| {
                                    DurableError::Internal(format!(
                                        "Failed to build step FAIL update: {e}"
                                    ))
                                })?,
                            )
                            .await?;

                        return Err(DurableError::step_failed_msg(
                            step_id,
                            attempts_made,
                            interrupted.to_string(),
                        ));
                    }

                    let delay = decision.delay.unwrap_or(Duration::seconds(1));
                    let mut builder = OperationUpdate::builder()
                        .id(&hashed_id)
                        .operation_type(OperationType::Step)
                        .sub_type("Step")
                        .action(OperationAction::Retry)
                        .error(err_obj)
                        .step_options(crate::types::StepUpdateOptions {
                            next_attempt_delay_seconds: Some(delay.to_seconds_i32_saturating()),
                        });
                    if let Some(ref pid) = parent_id {
                        builder = builder.parent_id(pid);
                    }
                    if let Some(n) = name {
                        builder = builder.name(n);
                    }

                    self.execution_ctx
                        .checkpoint_manager
                        .checkpoint(
                            step_id.clone(),
                            builder.build().map_err(|e| {
                                DurableError::Internal(format!(
                                    "Failed to build step RETRY update: {e}"
                                ))
                            })?,
                        )
                        .await?;

                    self.execution_ctx
                        .termination_manager
                        .terminate_for_retry()
                        .await;

                    std::future::pending::<()>().await;
                    unreachable!()
                }
                _ => {
                    // Started (at-least-once) - continue execution without re-starting.
                }
            }
        }

        // We're executing a new (or interrupted-at-least-once) step now.
        self.execution_ctx.set_mode(ExecutionMode::Execution).await;

        let already_started = matches!(
            operation.as_ref().map(|op| op.status),
            Some(OperationStatus::Started | OperationStatus::Ready)
        );

        if !already_started {
            // Phase 1: checkpoint START depending on semantics.
            let mut builder = OperationUpdate::builder()
                .id(&hashed_id)
                .operation_type(OperationType::Step)
                .sub_type("Step")
                .action(OperationAction::Start);
            if let Some(ref pid) = parent_id {
                builder = builder.parent_id(pid);
            }
            if let Some(n) = name {
                builder = builder.name(n);
            }

            let start_update = builder.build().map_err(|e| {
                DurableError::Internal(format!("Failed to build step START update: {e}"))
            })?;
            match semantics {
                StepSemantics::AtMostOncePerRetry => {
                    self.execution_ctx
                        .checkpoint_manager
                        .checkpoint(step_id.clone(), start_update)
                        .await?;
                }
                StepSemantics::AtLeastOncePerRetry => {
                    // Enqueue without waiting, mirroring JS/Python semantics while preserving ordering
                    // with subsequent checkpoints for the same operation.
                    self.execution_ctx
                        .checkpoint_manager
                        .checkpoint_queued(step_id.clone(), start_update)
                        .await?;
                }
            }
        }

        // Create step context
        let attempt_idx = operation
            .as_ref()
            .and_then(|op| op.step_details.as_ref().and_then(|d| d.attempt))
            .unwrap_or(0);
        let attempt = attempt_idx + 1;
        let mode_now = self.execution_ctx.get_mode().await;
        let step_ctx = StepContext::new_with_logger(
            name.map(String::from),
            hashed_id.clone(),
            self.execution_ctx.durable_execution_arn.clone(),
            self.execution_ctx.logger.clone(),
            mode_now,
            self.execution_ctx.mode_aware_logging,
            Some(attempt),
        );

        // Execute step function
        match step_fn(step_ctx).await {
            Ok(result) => {
                // Checkpoint SUCCESS
                let payload =
                    safe_serialize(serdes, Some(&result), &hashed_id, name, &self.execution_ctx)
                        .await;

                let mut builder = OperationUpdate::builder()
                    .id(&hashed_id)
                    .operation_type(OperationType::Step)
                    .sub_type("Step")
                    .action(OperationAction::Succeed);

                if let Some(ref pid) = parent_id {
                    builder = builder.parent_id(pid);
                }
                if let Some(n) = name {
                    builder = builder.name(n);
                }

                if let Some(p) = payload {
                    builder = builder.payload(p);
                }

                self.execution_ctx
                    .checkpoint_manager
                    .checkpoint(
                        step_id.clone(),
                        builder.build().map_err(|e| {
                            DurableError::Internal(format!(
                                "Failed to build step SUCCEED update: {e}"
                            ))
                        })?,
                    )
                    .await?;

                Ok(result)
            }
            Err(error) => {
                let attempts_made = attempt;
                let decision = retry_strategy.should_retry(error.as_ref(), attempts_made);

                if decision.should_retry {
                    let delay = decision.delay.unwrap_or(Duration::seconds(1));
                    let error_obj = ErrorObject::from_error(error.as_ref());

                    // Checkpoint retry with delay - triggers termination
                    let mut builder = OperationUpdate::builder()
                        .id(&hashed_id)
                        .operation_type(OperationType::Step)
                        .sub_type("Step")
                        .action(OperationAction::Retry)
                        .error(error_obj)
                        .step_options(crate::types::StepUpdateOptions {
                            next_attempt_delay_seconds: Some(delay.to_seconds_i32_saturating()),
                        });

                    if let Some(ref pid) = parent_id {
                        builder = builder.parent_id(pid);
                    }
                    if let Some(n) = name {
                        builder = builder.name(n);
                    }

                    self.execution_ctx
                        .checkpoint_manager
                        .checkpoint(
                            step_id.clone(),
                            builder.build().map_err(|e| {
                                DurableError::Internal(format!(
                                    "Failed to build step RETRY update: {e}"
                                ))
                            })?,
                        )
                        .await?;

                    // Trigger termination for retry
                    self.execution_ctx
                        .termination_manager
                        .terminate_for_retry()
                        .await;

                    // Never reached
                    std::future::pending::<()>().await;
                    unreachable!()
                }

                // No more retries - fail
                let error_obj = ErrorObject::from_error(error.as_ref());

                let mut builder = OperationUpdate::builder()
                    .id(&hashed_id)
                    .operation_type(OperationType::Step)
                    .sub_type("Step")
                    .action(OperationAction::Fail)
                    .error(error_obj);

                if let Some(ref pid) = parent_id {
                    builder = builder.parent_id(pid);
                }
                if let Some(n) = name {
                    builder = builder.name(n);
                }

                self.execution_ctx
                    .checkpoint_manager
                    .checkpoint(
                        step_id.clone(),
                        builder.build().map_err(|e| {
                            DurableError::Internal(format!("Failed to build step FAIL update: {e}"))
                        })?,
                    )
                    .await?;

                Err(DurableError::step_failed_boxed(
                    step_id,
                    attempts_made,
                    error,
                ))
            }
        }
    }

    /// Wait for a duration.
    pub async fn wait(&self, name: Option<&str>, duration: Duration) -> DurableResult<()> {
        let step_id = self.execution_ctx.next_operation_id(name);
        let hashed_id = Self::hash_id(&step_id);

        // If the requested duration is zero, avoid scheduling/suspending.
        // Returning PENDING with no pending ops is invalid and can happen if the wait completes immediately.
        if duration.is_zero() {
            return Ok(());
        }

        // Replay handling: if the wait already exists, never re-start it.
        if let Some(operation) = self.execution_ctx.get_step_data(&hashed_id).await {
            match operation.status {
                OperationStatus::Succeeded => return Ok(()),
                OperationStatus::Failed => {
                    return Err(DurableError::Internal("Wait failed in replay".to_string()))
                }
                _ => {
                    self.execution_ctx.set_mode(ExecutionMode::Execution).await;
                    self.execution_ctx
                        .termination_manager
                        .terminate_for_wait()
                        .await;
                    std::future::pending::<()>().await;
                    unreachable!()
                }
            }
        }

        // Checkpoint the wait
        let parent_id = self.execution_ctx.get_parent_id().await;
        let mut builder = OperationUpdate::builder()
            .id(&hashed_id)
            .operation_type(OperationType::Wait)
            .sub_type("Wait")
            .action(OperationAction::Start)
            .wait_options(crate::types::WaitUpdateOptions {
                wait_seconds: Some(duration.to_seconds_i32_saturating()),
            });

        if let Some(pid) = parent_id {
            builder = builder.parent_id(pid);
        }
        if let Some(n) = name {
            builder = builder.name(n);
        }

        self.execution_ctx
            .checkpoint_manager
            .checkpoint(
                step_id.clone(),
                builder.build().map_err(|e| {
                    DurableError::Internal(format!("Failed to build wait START update: {e}"))
                })?,
            )
            .await?;

        // The backend can resolve short waits quickly; re-check after the START checkpoint
        // to avoid suspending when there is nothing left pending.
        if let Some(operation) = self.execution_ctx.get_step_data(&hashed_id).await {
            if operation.status == OperationStatus::Succeeded {
                return Ok(());
            }
            if operation.status == OperationStatus::Failed {
                return Err(DurableError::Internal(
                    "Wait failed immediately after checkpoint".to_string(),
                ));
            }
        }

        // Mark this operation as awaited before suspending
        self.execution_ctx
            .checkpoint_manager
            .mark_awaited(&hashed_id)
            .await;

        // Trigger termination for wait
        self.execution_ctx
            .termination_manager
            .terminate_for_wait()
            .await;

        // This point is never reached during normal execution
        std::future::pending::<()>().await;
        unreachable!()
    }

    /// Invoke another Lambda function (legacy wrapper).
    pub async fn invoke<I, O>(
        &self,
        name: Option<&str>,
        function_id: &str,
        input: Option<I>,
    ) -> DurableResult<O>
    where
        I: Serialize + Send + Sync,
        O: DeserializeOwned + Send + Sync + 'static,
    {
        self.invoke_with_config(name, function_id, input, None)
            .await
    }

    /// Invoke another Lambda function with custom Serdes.
    pub async fn invoke_with_config<I, O>(
        &self,
        name: Option<&str>,
        function_id: &str,
        input: Option<I>,
        config: Option<InvokeConfig<I, O>>,
    ) -> DurableResult<O>
    where
        I: Serialize + Send + Sync,
        O: DeserializeOwned + Send + Sync + 'static,
    {
        let step_id = self.execution_ctx.next_operation_id(name);
        let hashed_id = Self::hash_id(&step_id);

        let payload_serdes = config.as_ref().and_then(|c| c.payload_serdes.clone());
        let result_serdes = config.as_ref().and_then(|c| c.result_serdes.clone());
        let tenant_id = config.as_ref().and_then(|c| c.tenant_id.clone());

        // Replay handling
        if let Some(operation) = self.execution_ctx.get_step_data(&hashed_id).await {
            return match operation.status {
                OperationStatus::Succeeded => {
                    if let Some(ref details) = operation.chained_invoke_details {
                        if let Some(ref payload) = details.result {
                            let val = safe_deserialize(
                                result_serdes,
                                Some(payload.as_str()),
                                &hashed_id,
                                name,
                                &self.execution_ctx,
                            )
                            .await
                            .ok_or_else(|| {
                                DurableError::Internal(
                                    "Missing invoke result in replay".to_string(),
                                )
                            })?;
                            return Ok(val);
                        }
                    }
                    Err(DurableError::Internal(
                        "Missing invoke result in replay".to_string(),
                    ))
                }
                OperationStatus::Failed => {
                    let msg = operation
                        .chained_invoke_details
                        .as_ref()
                        .and_then(|d| d.error.as_ref())
                        .map(|e| e.error_message.clone())
                        .unwrap_or_else(|| "Invoke failed".to_string());
                    Err(DurableError::InvocationFailed {
                        function: function_id.to_string(),
                        message: msg,
                        source: None,
                    })
                }
                _ => {
                    // Pending or started - suspend until completion
                    self.execution_ctx.set_mode(ExecutionMode::Execution).await;
                    self.execution_ctx
                        .termination_manager
                        .terminate_for_invoke()
                        .await;
                    std::future::pending::<()>().await;
                    unreachable!()
                }
            };
        }

        // Serialize input using custom Serdes if provided.
        let input_payload = safe_serialize(
            payload_serdes,
            input.as_ref(),
            &hashed_id,
            name,
            &self.execution_ctx,
        )
        .await;

        // Checkpoint START for chained invoke
        let parent_id = self.execution_ctx.get_parent_id().await;
        let mut builder = OperationUpdate::builder()
            .id(&hashed_id)
            .operation_type(OperationType::ChainedInvoke)
            .sub_type("ChainedInvoke")
            .action(OperationAction::Start)
            .chained_invoke_options(ChainedInvokeUpdateOptions {
                function_name: function_id.to_string(),
                tenant_id,
            });

        if let Some(pid) = parent_id {
            builder = builder.parent_id(pid);
        }
        if let Some(n) = name {
            builder = builder.name(n);
        }
        if let Some(payload) = input_payload {
            builder = builder.payload(payload);
        }

        self.execution_ctx
            .checkpoint_manager
            .checkpoint(
                step_id.clone(),
                builder.build().map_err(|e| {
                    DurableError::Internal(format!(
                        "Failed to build chained invoke START update: {e}"
                    ))
                })?,
            )
            .await?;

        // Suspend so the service can perform the invoke.
        self.execution_ctx
            .termination_manager
            .terminate_for_invoke()
            .await;

        std::future::pending::<()>().await;
        unreachable!()
    }

    /// Wait for an external callback.
    pub async fn wait_for_callback<T, F, Fut>(
        &self,
        name: Option<&str>,
        submitter: F,
        config: Option<CallbackConfig<T>>,
    ) -> DurableResult<T>
    where
        T: DeserializeOwned + Send + Sync + 'static,
        F: FnOnce(String, StepContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send + 'static,
    {
        let step_id = self.execution_ctx.next_operation_id(name);
        let hashed_id = Self::hash_id(&step_id);
        let config = config.unwrap_or_default();
        let serdes = config.serdes.clone();

        // Backwards compatibility: if an older Callback operation exists at this id,
        // return/await it directly.
        if let Some(operation) = self.execution_ctx.get_step_data(&hashed_id).await {
            if operation.operation_type == OperationType::Callback {
                return match operation.status {
                    OperationStatus::Succeeded => {
                        if let Some(ref details) = operation.callback_details {
                            if let Some(ref payload) = details.result {
                                if let Some(val) = safe_deserialize(
                                    serdes,
                                    Some(payload.as_str()),
                                    &hashed_id,
                                    name,
                                    &self.execution_ctx,
                                )
                                .await
                                {
                                    return Ok(val);
                                }
                            }
                        }
                        Err(DurableError::Internal(
                            "Missing callback result in replay".to_string(),
                        ))
                    }
                    OperationStatus::Failed => {
                        let error_msg = operation
                            .callback_details
                            .as_ref()
                            .and_then(|d| d.error.as_ref())
                            .map(|e| e.error_message.clone())
                            .unwrap_or_else(|| "Unknown error".to_string());
                        Err(DurableError::CallbackFailed {
                            name: step_id,
                            message: error_msg,
                        })
                    }
                    _ => {
                        self.execution_ctx
                            .termination_manager
                            .terminate_for_callback()
                            .await;
                        std::future::pending::<()>().await;
                        unreachable!()
                    }
                };
            }
        }

        let submitter_retry = config.retry_strategy.clone();
        let callback_cfg_for_child = config.clone();

        // New parity path: run in child context, create callback, execute submitter as a step with retries,
        // and return raw callback payload.
        let raw_payload: String = self
            .run_in_child_context_with_ids(
                step_id.clone(),
                hashed_id.clone(),
                name,
                move |child_ctx| async move {
                    let handle: CallbackHandle<T> = child_ctx
                        .create_callback(None, Some(callback_cfg_for_child))
                        .await?;
                    let callback_id = handle.callback_id().to_string();

                    let step_cfg = submitter_retry
                        .clone()
                        .map(|s| StepConfig::<()>::new().with_retry_strategy(s));

                    child_ctx
                        .step(
                            Some("submitter"),
                            move |step_ctx| async move {
                                submitter(callback_id, step_ctx).await?;
                                Ok(())
                            },
                            step_cfg,
                        )
                        .await?;

                    handle.wait_raw().await
                },
                Some(ChildContextConfig::<String> {
                    sub_type: Some("WaitForCallback".to_string()),
                    ..Default::default()
                }),
            )
            .await?;

        if let Some(val) = safe_deserialize(
            serdes,
            Some(raw_payload.as_str()),
            &hashed_id,
            name,
            &self.execution_ctx,
        )
        .await
        {
            Ok(val)
        } else {
            Err(DurableError::Internal(
                "Missing callback result after wait".to_string(),
            ))
        }
    }

    /// Run operations in a child context.
    pub async fn run_in_child_context<T, F, Fut>(
        &self,
        name: Option<&str>,
        context_fn: F,
        config: Option<ChildContextConfig<T>>,
    ) -> DurableResult<T>
    where
        T: Serialize + DeserializeOwned + Send + Sync + 'static,
        F: FnOnce(DurableContextHandle) -> Fut + Send + 'static,
        Fut: Future<Output = DurableResult<T>> + Send + 'static,
    {
        let step_id = self.execution_ctx.next_operation_id(name);
        let hashed_id = Self::hash_id(&step_id);

        self.run_in_child_context_with_ids(step_id, hashed_id, name, context_fn, config)
            .await
    }

    /// Run operations in a child context using pre-generated IDs.
    ///
    /// This is used internally for deterministic concurrent execution where the parent
    /// allocates child IDs sequentially before spawning tasks.
    pub async fn run_in_child_context_with_ids<T, F, Fut>(
        &self,
        step_id: String,
        hashed_id: String,
        name: Option<&str>,
        context_fn: F,
        config: Option<ChildContextConfig<T>>,
    ) -> DurableResult<T>
    where
        T: Serialize + DeserializeOwned + Send + Sync + 'static,
        F: FnOnce(DurableContextHandle) -> Fut + Send + 'static,
        Fut: Future<Output = DurableResult<T>> + Send + 'static,
    {
        let serdes = config.as_ref().and_then(|c| c.serdes.clone());
        let sub_type = config
            .as_ref()
            .and_then(|c| c.sub_type.clone())
            .unwrap_or_else(|| "RunInChildContext".to_string());

        // Check if already completed in replay
        if let Some(operation) = self.execution_ctx.get_step_data(&hashed_id).await {
            match operation.status {
                OperationStatus::Succeeded => {
                    if operation
                        .context_details
                        .as_ref()
                        .and_then(|d| d.replay_children)
                        == Some(true)
                    {
                        // ReplayChildren mode: reconstruct the result by re-running the child
                        // context while reading child operation outputs from replay state.
                        let child_execution_ctx =
                            self.execution_ctx.with_parent_id(hashed_id.clone());
                        let child_impl = Arc::new(DurableContextImpl::new(child_execution_ctx));
                        let child_ctx = DurableContextHandle::new(child_impl);
                        return context_fn(child_ctx).await;
                    }

                    if let Some(ref details) = operation.context_details {
                        if let Some(ref payload) = details.result {
                            if let Some(val) = safe_deserialize(
                                serdes.clone(),
                                Some(payload.as_str()),
                                &hashed_id,
                                name,
                                &self.execution_ctx,
                            )
                            .await
                            {
                                return Ok(val);
                            }
                        }
                    }
                    // Fallback for older payload locations.
                    if let Some(ref details) = operation.execution_details {
                        if let Some(ref payload) = details.output_payload {
                            if let Some(val) = safe_deserialize(
                                serdes.clone(),
                                Some(payload.as_str()),
                                &hashed_id,
                                name,
                                &self.execution_ctx,
                            )
                            .await
                            {
                                return Ok(val);
                            }
                        }
                    }
                }
                OperationStatus::Failed => {
                    let msg = operation
                        .context_details
                        .as_ref()
                        .and_then(|d| d.error.as_ref())
                        .map(|e| e.error_message.clone())
                        .unwrap_or_else(|| "Child context failed".to_string());
                    return Err(DurableError::ChildContextFailed {
                        name: step_id,
                        message: msg,
                        source: None,
                    });
                }
                _ => {
                    // Incomplete child context during replay, continue execution.
                    self.execution_ctx.set_mode(ExecutionMode::Execution).await;
                }
            }
        }

        // Checkpoint at start if not already started. This ensures any child operations that
        // reference `ParentId` (this context) are valid to the backend.
        if self.execution_ctx.get_step_data(&hashed_id).await.is_none() {
            let parent_id = self.execution_ctx.get_parent_id().await;
            let mut builder = OperationUpdate::builder()
                .id(&hashed_id)
                .operation_type(OperationType::Context)
                .sub_type(sub_type.clone())
                .action(OperationAction::Start);

            if let Some(pid) = parent_id {
                builder = builder.parent_id(pid);
            }
            if let Some(n) = name {
                builder = builder.name(n);
            }

            self.execution_ctx
                .checkpoint_manager
                .checkpoint(
                    step_id.clone(),
                    builder.build().map_err(|e| {
                        DurableError::Internal(format!(
                            "Failed to build child context START update: {e}"
                        ))
                    })?,
                )
                .await?;
        }

        // Create child context
        let child_execution_ctx = self.execution_ctx.with_parent_id(hashed_id.clone());
        let child_impl = Arc::new(DurableContextImpl::new(child_execution_ctx));
        let child_ctx = DurableContextHandle::new(child_impl);

        // Execute child context
        let result = match context_fn(child_ctx).await {
            Ok(val) => val,
            Err(error) => {
                let err_obj = ErrorObject::from_durable_error(&error);
                let parent_id = self.execution_ctx.get_parent_id().await;

                let mut builder = OperationUpdate::builder()
                    .id(&hashed_id)
                    .operation_type(OperationType::Context)
                    .sub_type(sub_type)
                    .action(OperationAction::Fail)
                    .error(err_obj.clone());

                if let Some(pid) = parent_id {
                    builder = builder.parent_id(pid);
                }
                if let Some(n) = name {
                    builder = builder.name(n);
                }

                self.execution_ctx
                    .checkpoint_manager
                    .checkpoint(
                        step_id.clone(),
                        builder.build().map_err(|e| {
                            DurableError::Internal(format!(
                                "Failed to build child context FAIL update: {e}"
                            ))
                        })?,
                    )
                    .await?;

                return Err(DurableError::ChildContextFailed {
                    name: step_id,
                    message: err_obj.error_message,
                    source: Some(Arc::new(Box::new(error))),
                });
            }
        };

        // Checkpoint child context completion
        let mut payload =
            safe_serialize(serdes, Some(&result), &hashed_id, name, &self.execution_ctx).await;
        let mut replay_children = false;
        if let Some(ref p) = payload {
            if p.len() > CHECKPOINT_SIZE_LIMIT_BYTES {
                replay_children = true;
                payload = Some(String::new());
            }
        }

        let parent_id = self.execution_ctx.get_parent_id().await;
        let mut builder = OperationUpdate::builder()
            .id(&hashed_id)
            .operation_type(OperationType::Context)
            .sub_type(sub_type)
            .action(OperationAction::Succeed);

        if replay_children {
            builder = builder.context_options(ContextUpdateOptions {
                replay_children: Some(true),
            });
        }

        if let Some(p) = payload {
            builder = builder.payload(p);
        }

        if let Some(pid) = parent_id {
            builder = builder.parent_id(pid);
        }
        if let Some(n) = name {
            builder = builder.name(n);
        }
        self.execution_ctx
            .checkpoint_manager
            .checkpoint(
                step_id,
                builder.build().map_err(|e| {
                    DurableError::Internal(format!(
                        "Failed to build child context completion update: {e}"
                    ))
                })?,
            )
            .await?;

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CompletionConfig;

    #[test]
    fn test_validate_completion_config_ok() {
        let config = CompletionConfig::new()
            .with_min_successful(1)
            .with_tolerated_failures(1)
            .with_tolerated_failure_percentage(50.0);

        validate_completion_config(&config, 2, "parallel").expect("valid config");
    }

    #[test]
    fn test_validate_completion_config_min_successful_exceeds() {
        let config = CompletionConfig::new().with_min_successful(3);
        let err = validate_completion_config(&config, 2, "parallel").expect_err("error");

        match err {
            DurableError::InvalidConfiguration { message } => {
                assert!(message.contains("min_successful"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn test_validate_completion_config_failure_count_exceeds() {
        let config = CompletionConfig::new().with_tolerated_failures(4);
        let err = validate_completion_config(&config, 2, "parallel").expect_err("error");

        match err {
            DurableError::InvalidConfiguration { message } => {
                assert!(message.contains("tolerated_failure_count"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn test_validate_completion_config_percentage_invalid() {
        let config = CompletionConfig::new().with_tolerated_failure_percentage(-1.0);
        assert!(validate_completion_config(&config, 2, "parallel").is_err());

        let config = CompletionConfig::new().with_tolerated_failure_percentage(101.0);
        assert!(validate_completion_config(&config, 2, "parallel").is_err());

        let config = CompletionConfig::new().with_tolerated_failure_percentage(f64::NAN);
        assert!(validate_completion_config(&config, 2, "parallel").is_err());
    }
}

impl std::fmt::Debug for DurableContextImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DurableContextImpl")
            .field("execution_ctx", &self.execution_ctx)
            .finish()
    }
}
