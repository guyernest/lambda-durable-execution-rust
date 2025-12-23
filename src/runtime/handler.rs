//! Runtime handler for durable Lambda functions.
//!
//! This module provides the `with_durable_execution` wrapper that handles
//! the durable execution lifecycle including:
//! - Parsing input and setting up execution context
//! - Managing termination signals
//! - Coordinating checkpoint completion
//! - Returning proper output format

use crate::context::{DurableContextHandle, DurableContextImpl, ExecutionContext};
use crate::error::DurableResult;
use crate::termination::TerminationReason;
use crate::types::DurableLogger;
use crate::types::{
    DurableExecutionInvocationInput, DurableExecutionInvocationOutput, LambdaService,
    OperationAction, OperationType, OperationUpdate, RealLambdaService,
};
use aws_sdk_lambda::Client as LambdaClient;
use futures::future::BoxFuture;
use lambda_runtime::{service_fn, LambdaEvent, Service};
use serde::{de::DeserializeOwned, Serialize};
use std::future::Future;
use std::sync::Arc;
use tracing::{debug, error, info};

/// Lambda response size limit is 6MB.
///
/// Keep a small buffer for the envelope and headers (matches the JS SDK behavior).
const LAMBDA_RESPONSE_SIZE_LIMIT: usize = 6 * 1024 * 1024 - 50;

/// Configuration for durable execution.
#[derive(Clone)]
pub struct DurableExecutionConfig {
    /// Custom Lambda service (if not provided, one will be created).
    pub lambda_service: Option<Arc<dyn LambdaService>>,

    /// Optional custom logger for durable operations.
    pub logger: Option<Arc<dyn DurableLogger>>,

    /// Whether to suppress logs during replay.
    pub mode_aware_logging: bool,
}

impl std::fmt::Debug for DurableExecutionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DurableExecutionConfig")
            .field("lambda_service", &self.lambda_service.is_some())
            .field("logger", &self.logger.is_some())
            .field("mode_aware_logging", &self.mode_aware_logging)
            .finish()
    }
}

impl Default for DurableExecutionConfig {
    fn default() -> Self {
        Self {
            lambda_service: None,
            logger: None,
            mode_aware_logging: true,
        }
    }
}

impl DurableExecutionConfig {
    /// Create a new config with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a custom Lambda client.
    pub fn with_lambda_client(mut self, client: Arc<LambdaClient>) -> Self {
        self.lambda_service = Some(Arc::new(RealLambdaService::new(client)));
        self
    }

    /// Set a custom Lambda service.
    pub fn with_lambda_service(mut self, service: Arc<dyn LambdaService>) -> Self {
        self.lambda_service = Some(service);
        self
    }

    /// Set a custom durable logger.
    pub fn with_logger(mut self, logger: Arc<dyn DurableLogger>) -> Self {
        self.logger = Some(logger);
        self
    }

    /// Enable or disable mode-aware logging.
    pub fn with_mode_aware_logging(mut self, enabled: bool) -> Self {
        self.mode_aware_logging = enabled;
        self
    }
}

/// Wrap a handler function with durable execution support.
///
/// This is the main entry point for creating durable Lambda functions.
/// It handles:
/// - Parsing the durable execution input
/// - Setting up the execution context
/// - Running the user handler
/// - Managing termination and checkpointing
/// - Returning the proper output format
///
/// # Type Parameters
///
/// * `E` - The event type (deserialized from the `input_payload`)
/// * `R` - The response type (serialized to the `output_payload`)
/// * `F` - The handler function type
///
/// # Example
///
/// ```rust,no_run
/// use lambda_durable_execution_rust::prelude::*;
/// use lambda_durable_execution_rust::runtime::with_durable_execution_service;
/// use serde::{Deserialize, Serialize};
///
/// async fn my_handler(
///     _event: MyEvent,
///     ctx: DurableContextHandle,
/// ) -> DurableResult<MyResponse> {
///     let _ = ctx.step(Some("noop"), |_| async { Ok(()) }, None).await?;
///     Ok(MyResponse {})
/// }
///
/// #[derive(Deserialize)]
/// struct MyEvent;
///
/// #[derive(Serialize)]
/// struct MyResponse {}
///
/// #[tokio::main]
/// async fn main() -> Result<(), lambda_runtime::Error> {
///     let handler = with_durable_execution_service(my_handler, None);
///     lambda_runtime::run(handler).await
/// }
/// ```
pub fn with_durable_execution<E, R, F, Fut>(
    handler: F,
    config: Option<DurableExecutionConfig>,
) -> impl Fn(
    DurableExecutionInvocationInput,
) -> BoxFuture<'static, Result<DurableExecutionInvocationOutput, lambda_runtime::Error>>
       + Clone
       + Send
       + Sync
       + 'static
where
    E: DeserializeOwned + Send + 'static,
    R: Serialize + Send + 'static,
    F: Fn(E, DurableContextHandle) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = DurableResult<R>> + Send + 'static,
{
    let config = config.unwrap_or_default();

    move |input: DurableExecutionInvocationInput| {
        let handler = handler.clone();
        let config = config.clone();

        Box::pin(async move { execute_durable_handler(input, handler, config).await })
    }
}

/// Create a Lambda Runtime service for a durable handler.
///
/// `lambda_runtime::run` expects a [`Service`] that takes a [`LambdaEvent`]. This helper
/// wraps [`with_durable_execution`] into a compatible service.
pub fn with_durable_execution_service<E, R, F, Fut>(
    handler: F,
    config: Option<DurableExecutionConfig>,
) -> impl Service<
    LambdaEvent<DurableExecutionInvocationInput>,
    Response = DurableExecutionInvocationOutput,
    Error = lambda_runtime::Error,
> + Clone
       + Send
       + Sync
       + 'static
where
    E: DeserializeOwned + Send + 'static,
    R: Serialize + Send + 'static,
    F: Fn(E, DurableContextHandle) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = DurableResult<R>> + Send + 'static,
{
    let handler = with_durable_execution::<E, R, F, Fut>(handler, config);
    service_fn(move |event: LambdaEvent<DurableExecutionInvocationInput>| handler(event.payload))
}

/// Execute a durable handler.
async fn execute_durable_handler<E, R, F, Fut>(
    input: DurableExecutionInvocationInput,
    handler: F,
    config: DurableExecutionConfig,
) -> Result<DurableExecutionInvocationOutput, lambda_runtime::Error>
where
    E: DeserializeOwned + Send + 'static,
    R: Serialize + Send + 'static,
    F: Fn(E, DurableContextHandle) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = DurableResult<R>> + Send + 'static,
{
    info!(
        "Starting durable execution: {}",
        input.durable_execution_arn
    );

    // Create or use provided Lambda service
    let lambda_service = match config.lambda_service {
        Some(service) => service,
        None => {
            let sdk_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
            let client = Arc::new(LambdaClient::new(&sdk_config));
            Arc::new(RealLambdaService::new(client))
        }
    };

    // Create execution context
    let execution_ctx = ExecutionContext::new(
        &input,
        lambda_service,
        config.logger.clone(),
        config.mode_aware_logging,
    )
    .await
    .map_err(|e| lambda_runtime::Error::from(format!("Failed to initialize context: {}", e)))?;
    let termination_manager = Arc::clone(&execution_ctx.termination_manager);
    let checkpoint_manager = Arc::clone(&execution_ctx.checkpoint_manager);

    // Deserialize the user event from the execution operation's input payload.
    let (has_execution_op, input_payload) = {
        let step_data = execution_ctx.step_data.lock().await;
        let execution_op = step_data
            .values()
            .find(|op| op.operation_type == OperationType::Execution);
        (
            execution_op.is_some(),
            execution_op
                .and_then(|op| op.execution_details.as_ref())
                .and_then(|details| details.input_payload.clone()),
        )
    };

    let input_payload = match (has_execution_op, input_payload) {
        (false, _) => {
            return Err(lambda_runtime::Error::from(
                "Missing execution operation in initial execution state",
            ));
        }
        (true, Some(payload)) => payload,
        (true, None) => {
            return Err(lambda_runtime::Error::from(
                "Missing input payload in execution operation",
            ));
        }
    };

    let event: E = serde_json::from_str(&input_payload)
        .map_err(|e| lambda_runtime::Error::from(format!("Failed to deserialize input: {}", e)))?;

    // Create durable context handle
    let durable_ctx = DurableContextHandle::new(Arc::new(DurableContextImpl::new(execution_ctx)));

    // Run the handler with termination monitoring
    let handler_future = handler(event, durable_ctx);

    // Race the handler against termination
    let mut termination_result = None;
    let result = tokio::select! {
        handler_result = handler_future => {
            // Handler completed
            Some(handler_result)
        }
        termination = termination_manager.wait_for_termination() => {
            // Termination was triggered
            debug!("Termination triggered: {:?}", termination);
            termination_result = Some(termination);
            None
        }
    };

    // Wait for any pending checkpoints to complete
    checkpoint_manager.wait_for_queue_completion().await;

    // Build the output based on result
    match result {
        Some(Ok(response)) => {
            // Successful completion
            info!("Handler completed successfully");

            let output_payload = serde_json::to_string(&response).map_err(|e| {
                lambda_runtime::Error::from(format!("Failed to serialize output: {}", e))
            })?;

            // If response is too large to return, checkpoint it and return an empty Result.
            if output_payload.len() > LAMBDA_RESPONSE_SIZE_LIMIT {
                info!(
                    "Response size ({}) exceeds Lambda limit ({}). Checkpointing result.",
                    output_payload.len(),
                    LAMBDA_RESPONSE_SIZE_LIMIT
                );

                let step_id = format!("execution-result-{}", uuid::Uuid::new_v4());
                let hashed_id = crate::checkpoint::CheckpointManager::hash_id(&step_id);
                let update = OperationUpdate::builder()
                    .id(&hashed_id)
                    .operation_type(OperationType::Execution)
                    .action(OperationAction::Succeed)
                    .payload(&output_payload)
                    .build()
                    .map_err(|e| {
                        lambda_runtime::Error::from(format!(
                            "Failed to build large-result update: {}",
                            e
                        ))
                    })?;

                checkpoint_manager
                    .checkpoint(step_id, update)
                    .await
                    .map_err(|e| {
                        lambda_runtime::Error::from(format!(
                            "Failed to checkpoint large result: {}",
                            e
                        ))
                    })?;

                // Ensure the checkpoint queue drains before returning.
                checkpoint_manager.wait_for_queue_completion().await;

                return Ok(DurableExecutionInvocationOutput::succeeded(Some(
                    String::new(),
                )));
            }

            Ok(DurableExecutionInvocationOutput::succeeded(Some(
                output_payload,
            )))
        }
        Some(Err(error)) => {
            // Handler returned an error
            error!("Handler failed: {}", error);

            let error_obj = crate::error::ErrorObject::from_durable_error(&error);
            Ok(DurableExecutionInvocationOutput::failed(error_obj))
        }
        None => {
            // Termination was triggered (wait, callback, retry, etc.)
            if let Some(term) = termination_result {
                match term.reason {
                    TerminationReason::CheckpointFailed => {
                        // Propagate checkpoint failure as a Lambda error (matches JS parity).
                        let msg = term
                            .message
                            .unwrap_or_else(|| "Checkpoint failed".to_string());
                        return Err(lambda_runtime::Error::from(msg));
                    }
                    TerminationReason::SerdesFailed => {
                        let msg = term
                            .message
                            .unwrap_or_else(|| "Serdes operation failed".to_string());
                        return Err(lambda_runtime::Error::from(msg));
                    }
                    TerminationReason::ContextValidationError => {
                        let message = match term.error.as_ref().map(|e| e.as_ref()) {
                            Some(crate::error::DurableError::ContextValidationError {
                                message,
                            }) => message.clone(),
                            Some(err) => err.to_string(),
                            None => term
                                .message
                                .clone()
                                .unwrap_or_else(|| "Context validation error".to_string()),
                        };

                        let err = crate::error::DurableError::ContextValidationError { message };
                        let err_obj = crate::error::ErrorObject::from_durable_error(&err);
                        return Ok(DurableExecutionInvocationOutput::failed(err_obj));
                    }
                    _ => {}
                }
            }

            info!("Handler suspended due to termination");
            Ok(DurableExecutionInvocationOutput::pending())
        }
    }
}

/// A durable handler function type.
///
/// This type alias represents the signature of a durable handler function.
pub type DurableHandlerFn<E, R> = Box<
    dyn Fn(E, DurableContextHandle) -> BoxFuture<'static, DurableResult<R>> + Send + Sync + 'static,
>;

/// Builder for creating durable Lambda handlers.
///
/// This provides a more ergonomic way to configure durable handlers.
#[derive(Clone)]
pub struct DurableHandlerBuilder<E, R, F, Fut>
where
    E: DeserializeOwned + Send + 'static,
    R: Serialize + Send + 'static,
    F: Fn(E, DurableContextHandle) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = DurableResult<R>> + Send + 'static,
{
    handler: F,
    config: DurableExecutionConfig,
    _phantom: std::marker::PhantomData<(E, R, Fut)>,
}

impl<E, R, F, Fut> DurableHandlerBuilder<E, R, F, Fut>
where
    E: DeserializeOwned + Send + 'static,
    R: Serialize + Send + 'static,
    F: Fn(E, DurableContextHandle) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = DurableResult<R>> + Send + 'static,
{
    /// Create a new builder with the given handler.
    pub fn new(handler: F) -> Self {
        Self {
            handler,
            config: DurableExecutionConfig::default(),
            _phantom: std::marker::PhantomData,
        }
    }

    /// Set a custom Lambda client.
    pub fn with_lambda_client(mut self, client: Arc<LambdaClient>) -> Self {
        self.config.lambda_service = Some(Arc::new(RealLambdaService::new(client)));
        self
    }

    /// Set a custom Lambda service.
    pub fn with_lambda_service(mut self, service: Arc<dyn LambdaService>) -> Self {
        self.config.lambda_service = Some(service);
        self
    }

    /// Build the handler function.
    pub fn build(
        self,
    ) -> impl Fn(
        DurableExecutionInvocationInput,
    )
        -> BoxFuture<'static, Result<DurableExecutionInvocationOutput, lambda_runtime::Error>>
           + Clone
           + Send
           + Sync
           + 'static {
        with_durable_execution(self.handler, Some(self.config))
    }
}

/// Create a new durable handler builder.
///
/// # Example
///
/// ```rust,no_run
/// # use lambda_durable_execution_rust::prelude::*;
/// use lambda_durable_execution_rust::runtime::durable_handler;
/// use lambda_durable_execution_rust::types::DurableExecutionInvocationInput;
/// use lambda_runtime::{service_fn, LambdaEvent};
/// use std::sync::Arc;
///
/// # async fn my_handler(_event: serde_json::Value, _ctx: DurableContextHandle) -> DurableResult<()> { Ok(()) }
/// # fn make_client() -> aws_sdk_lambda::Client {
/// #     let conf = aws_sdk_lambda::Config::builder()
/// #         .region(aws_sdk_lambda::config::Region::new("us-east-1"))
/// #         .behavior_version(aws_sdk_lambda::config::BehaviorVersion::latest())
/// #         .build();
/// #     aws_sdk_lambda::Client::from_conf(conf)
/// # }
/// #[tokio::main]
/// async fn main() -> Result<(), lambda_runtime::Error> {
///     let custom_client = make_client();
///     let handler_fn = durable_handler(my_handler)
///         .with_lambda_client(Arc::new(custom_client))
///         .build();
///
///     let service = service_fn(move |event: LambdaEvent<DurableExecutionInvocationInput>| {
///         let handler_fn = handler_fn.clone();
///         async move { handler_fn(event.payload).await }
///     });
///
///     lambda_runtime::run(service).await
/// }
/// ```
pub fn durable_handler<E, R, F, Fut>(handler: F) -> DurableHandlerBuilder<E, R, F, Fut>
where
    E: DeserializeOwned + Send + 'static,
    R: Serialize + Send + 'static,
    F: Fn(E, DurableContextHandle) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = DurableResult<R>> + Send + 'static,
{
    DurableHandlerBuilder::new(handler)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockLambdaService;
    use crate::types::{ExecutionDetails, InitialExecutionState, Operation, OperationStatus};
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn test_config_builder() {
        let config = DurableExecutionConfig::new();
        assert!(config.lambda_service.is_none());
    }

    #[tokio::test]
    async fn test_missing_execution_operation_returns_error() {
        let input = DurableExecutionInvocationInput {
            durable_execution_arn: "arn:aws:lambda:us-east-1:123:function:durable".to_string(),
            checkpoint_token: "token-0".to_string(),
            initial_execution_state: InitialExecutionState {
                operations: vec![],
                next_marker: None,
            },
        };

        let config =
            DurableExecutionConfig::new().with_lambda_service(Arc::new(MockLambdaService::new()));

        let err = execute_durable_handler(
            input,
            |_event: serde_json::Value, _ctx| async { Ok(json!({"ok": true})) },
            config,
        )
        .await
        .expect_err("missing execution op should error");

        assert!(err
            .to_string()
            .contains("Missing execution operation in initial execution state"));
    }

    #[tokio::test]
    async fn test_missing_input_payload_returns_error() {
        let input = DurableExecutionInvocationInput {
            durable_execution_arn: "arn:aws:lambda:us-east-1:123:function:durable".to_string(),
            checkpoint_token: "token-0".to_string(),
            initial_execution_state: InitialExecutionState {
                operations: vec![Operation {
                    id: "execution".to_string(),
                    parent_id: None,
                    name: None,
                    operation_type: OperationType::Execution,
                    sub_type: None,
                    status: OperationStatus::Started,
                    step_details: None,
                    callback_details: None,
                    wait_details: None,
                    execution_details: Some(ExecutionDetails {
                        input_payload: None,
                        output_payload: None,
                    }),
                    context_details: None,
                    chained_invoke_details: None,
                }],
                next_marker: None,
            },
        };

        let config =
            DurableExecutionConfig::new().with_lambda_service(Arc::new(MockLambdaService::new()));

        let err = execute_durable_handler(
            input,
            |_event: serde_json::Value, _ctx| async { Ok(json!({"ok": true})) },
            config,
        )
        .await
        .expect_err("missing input payload should error");

        assert!(err
            .to_string()
            .contains("Missing input payload in execution operation"));
    }

    #[tokio::test]
    async fn test_handler_success_returns_output_payload() {
        let input_payload = serde_json::to_string(&json!({"value": 1})).unwrap();
        let input = DurableExecutionInvocationInput {
            durable_execution_arn: "arn:aws:lambda:us-east-1:123:function:durable".to_string(),
            checkpoint_token: "token-0".to_string(),
            initial_execution_state: InitialExecutionState {
                operations: vec![Operation {
                    id: "execution".to_string(),
                    parent_id: None,
                    name: None,
                    operation_type: OperationType::Execution,
                    sub_type: None,
                    status: OperationStatus::Started,
                    step_details: None,
                    callback_details: None,
                    wait_details: None,
                    execution_details: Some(ExecutionDetails {
                        input_payload: Some(input_payload),
                        output_payload: None,
                    }),
                    context_details: None,
                    chained_invoke_details: None,
                }],
                next_marker: None,
            },
        };

        let config =
            DurableExecutionConfig::new().with_lambda_service(Arc::new(MockLambdaService::new()));

        let output = execute_durable_handler(
            input,
            |_event: serde_json::Value, _ctx| async { Ok(json!({"ok": true})) },
            config,
        )
        .await
        .expect("handler should succeed");

        assert_eq!(output.status, crate::types::InvocationStatus::Succeeded);
        assert_eq!(output.result, Some("{\"ok\":true}".to_string()));
    }

    #[tokio::test]
    async fn test_large_output_payload_is_checkpointed() {
        let input_payload = serde_json::to_string(&json!({"value": 1})).unwrap();
        let input = DurableExecutionInvocationInput {
            durable_execution_arn: "arn:aws:lambda:us-east-1:123:function:durable".to_string(),
            checkpoint_token: "token-0".to_string(),
            initial_execution_state: InitialExecutionState {
                operations: vec![Operation {
                    id: "execution".to_string(),
                    parent_id: None,
                    name: None,
                    operation_type: OperationType::Execution,
                    sub_type: None,
                    status: OperationStatus::Started,
                    step_details: None,
                    callback_details: None,
                    wait_details: None,
                    execution_details: Some(ExecutionDetails {
                        input_payload: Some(input_payload),
                        output_payload: None,
                    }),
                    context_details: None,
                    chained_invoke_details: None,
                }],
                next_marker: None,
            },
        };

        let mock = Arc::new(MockLambdaService::new());
        mock.expect_checkpoint(crate::mock::MockCheckpointConfig::default());

        let config = DurableExecutionConfig::new().with_lambda_service(mock.clone());

        let big = "a".repeat(LAMBDA_RESPONSE_SIZE_LIMIT + 128);

        let output = execute_durable_handler(
            input,
            move |_event: serde_json::Value, _ctx| {
                let big = big.clone();
                async move { Ok(json!({ "data": big })) }
            },
            config,
        )
        .await
        .expect("handler should succeed");

        assert_eq!(output.status, crate::types::InvocationStatus::Succeeded);
        assert_eq!(output.result, Some(String::new()));

        let calls = mock.checkpoint_calls();
        assert_eq!(calls.len(), 1);
        let update = &calls[0].updates[0];
        assert_eq!(update.operation_type, OperationType::Execution);
        assert_eq!(update.action, OperationAction::Succeed);
        assert!(update.payload.as_ref().unwrap().len() > LAMBDA_RESPONSE_SIZE_LIMIT);
    }
}
