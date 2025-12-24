//! Runtime handler for durable Lambda functions.
//!
//! This module provides the `with_durable_execution` wrapper that handles
//! the durable execution lifecycle including:
//! - Parsing input and setting up execution context
//! - Managing termination signals
//! - Coordinating checkpoint completion
//! - Returning proper output format

use crate::context::DurableContextHandle;
use crate::error::DurableResult;
use crate::types::DurableLogger;
use crate::types::{
    DurableExecutionInvocationInput, DurableExecutionInvocationOutput, LambdaService,
    OperationAction, OperationType, RealLambdaService,
};
use aws_sdk_lambda::Client as LambdaClient;
use futures::future::BoxFuture;
use lambda_runtime::{service_fn, LambdaEvent, Service};
use serde::{de::DeserializeOwned, Serialize};
use std::future::Future;
use std::sync::Arc;

mod execute;

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
    execute::execute_durable_handler(input, handler, config).await
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
    use crate::error::DurableError;
    use crate::mock::MockLambdaService;
    use crate::types::{
        ExecutionDetails, InitialExecutionState, InvocationStatus, Operation, OperationStatus,
    };
    use serde::{Deserialize, Serialize};
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

    #[tokio::test]
    async fn test_handler_error_returns_failed_output() {
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
            |_event: serde_json::Value, _ctx| async {
                Err::<serde_json::Value, _>(DurableError::Internal("boom".to_string()))
            },
            config,
        )
        .await
        .expect("handler error should map to invocation output");

        assert_eq!(output.status, crate::types::InvocationStatus::Failed);
        let err = output.error.expect("error object");
        assert_eq!(err.error_type, "Internal");
        assert!(err.error_message.contains("boom"));
    }

    #[derive(Debug)]
    struct BadSerialize;

    impl Serialize for BadSerialize {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            Err(serde::ser::Error::custom("nope"))
        }
    }

    #[tokio::test]
    async fn test_output_serialization_failure_returns_lambda_error() {
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

        let err = execute_durable_handler(
            input,
            |_event: serde_json::Value, _ctx| async { Ok(BadSerialize) },
            config,
        )
        .await
        .expect_err("serialization error should surface");

        assert!(err.to_string().contains("Failed to serialize output"));
    }

    #[derive(Debug, Deserialize)]
    struct SampleEvent {
        value: u32,
    }

    #[tokio::test]
    async fn test_input_deserialization_failure_returns_lambda_error() {
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
                        input_payload: Some("{\"value\":\"oops\"}".to_string()),
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
            |event: SampleEvent, _ctx| async move { Ok(json!({ "ok": event.value })) },
            config,
        )
        .await
        .expect_err("deserialization error should surface");

        assert!(err.to_string().contains("Failed to deserialize input"));
    }

    #[test]
    fn test_config_debug_includes_flags() {
        let config = DurableExecutionConfig::new()
            .with_logger(Arc::new(crate::types::TracingLogger))
            .with_mode_aware_logging(false);

        let debug = format!("{:?}", config);
        assert!(debug.contains("logger: true"));
        assert!(debug.contains("mode_aware_logging: false"));
    }

    #[tokio::test]
    async fn test_with_durable_execution_wrapper_invokes_handler() {
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

        let handler = with_durable_execution(
            |_event: serde_json::Value, _ctx| async { Ok(json!({"ok": true})) },
            Some(
                DurableExecutionConfig::new()
                    .with_lambda_service(Arc::new(MockLambdaService::new())),
            ),
        );

        let output = handler(input).await.expect("handler should succeed");
        assert_eq!(output.status, InvocationStatus::Succeeded);
    }

    #[tokio::test]
    async fn test_durable_handler_builder_with_lambda_service() {
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

        let handler_fn = durable_handler(|_event: serde_json::Value, _ctx| async {
            Ok(json!({"ok": true}))
        })
        .with_lambda_service(Arc::new(MockLambdaService::new()))
        .build();

        let output = handler_fn(input).await.expect("handler should succeed");
        assert_eq!(output.status, InvocationStatus::Succeeded);
    }

    #[test]
    fn test_durable_handler_builder_with_lambda_client() {
        let sdk_config = aws_sdk_lambda::Config::builder()
            .region(aws_sdk_lambda::config::Region::new("us-east-1"))
            .behavior_version(aws_sdk_lambda::config::BehaviorVersion::latest())
            .build();
        let client = aws_sdk_lambda::Client::from_conf(sdk_config);

        let _handler = durable_handler(|_event: serde_json::Value, _ctx| async {
            Ok(json!({"ok": true}))
        })
        .with_lambda_client(Arc::new(client));
    }

    #[tokio::test]
    async fn test_handler_termination_checkpoint_failed_returns_error() {
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

        let err = execute_durable_handler(
            input,
            |_event: serde_json::Value, ctx| async move {
                ctx.execution_context()
                    .termination_manager
                    .terminate_for_checkpoint_failure(DurableError::CheckpointFailed {
                        message: "checkpoint failed".to_string(),
                        recoverable: false,
                        source: None,
                    })
                    .await;
                std::future::pending::<DurableResult<serde_json::Value>>().await
            },
            config,
        )
        .await
        .expect_err("checkpoint failure should surface");

        assert!(err.to_string().contains("checkpoint failed"));
    }

    #[tokio::test]
    async fn test_handler_termination_serdes_failed_returns_error() {
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

        let err = execute_durable_handler(
            input,
            |_event: serde_json::Value, ctx| async move {
                ctx.execution_context()
                    .termination_manager
                    .terminate_for_serdes_failure("serdes failed")
                    .await;
                std::future::pending::<DurableResult<serde_json::Value>>().await
            },
            config,
        )
        .await
        .expect_err("serdes failure should surface");

        assert!(err.to_string().contains("serdes failed"));
    }

    #[tokio::test]
    async fn test_handler_termination_context_validation_returns_failed_output() {
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
            |_event: serde_json::Value, ctx| async move {
                ctx.execution_context()
                    .termination_manager
                    .terminate_for_context_validation(DurableError::ContextValidationError {
                        message: "bad".to_string(),
                    })
                    .await;
                std::future::pending::<DurableResult<serde_json::Value>>().await
            },
            config,
        )
        .await
        .expect("context validation should map to failed output");

        assert_eq!(output.status, InvocationStatus::Failed);
        let err = output.error.expect("error object");
        assert_eq!(err.error_type, "ContextValidationError");
        assert!(err.error_message.contains("bad"));
    }
}
