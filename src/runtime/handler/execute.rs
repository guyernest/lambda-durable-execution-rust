use crate::context::{DurableContextHandle, DurableContextImpl, ExecutionContext};
use crate::error::DurableResult;
use crate::termination::TerminationReason;
use crate::types::{
    DurableExecutionInvocationInput, DurableExecutionInvocationOutput, OperationAction,
    OperationType, OperationUpdate, RealLambdaService,
};
use aws_sdk_lambda::Client as LambdaClient;
use lambda_runtime::Error as LambdaError;
use serde::{de::DeserializeOwned, Serialize};
use std::future::Future;
use std::sync::Arc;
use tracing::{debug, error, info};

use super::{DurableExecutionConfig, LAMBDA_RESPONSE_SIZE_LIMIT};

pub(super) async fn execute_durable_handler<E, R, F, Fut>(
    input: DurableExecutionInvocationInput,
    handler: F,
    config: DurableExecutionConfig,
) -> Result<DurableExecutionInvocationOutput, LambdaError>
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
    .map_err(|e| LambdaError::from(format!("Failed to initialize context: {}", e)))?;
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
            return Err(LambdaError::from(
                "Missing execution operation in initial execution state",
            ));
        }
        (true, Some(payload)) => payload,
        (true, None) => {
            return Err(LambdaError::from(
                "Missing input payload in execution operation",
            ));
        }
    };

    let event: E = serde_json::from_str(&input_payload)
        .map_err(|e| LambdaError::from(format!("Failed to deserialize input: {}", e)))?;

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

            let output_payload = serde_json::to_string(&response)
                .map_err(|e| LambdaError::from(format!("Failed to serialize output: {}", e)))?;

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
                        LambdaError::from(format!("Failed to build large-result update: {}", e))
                    })?;

                checkpoint_manager
                    .checkpoint(step_id, update)
                    .await
                    .map_err(|e| {
                        LambdaError::from(format!("Failed to checkpoint large result: {}", e))
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
                        return Err(LambdaError::from(msg));
                    }
                    TerminationReason::SerdesFailed => {
                        let msg = term
                            .message
                            .unwrap_or_else(|| "Serdes operation failed".to_string());
                        return Err(LambdaError::from(msg));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::DurableError;
    use crate::error::DurableResult;
    use crate::mock::{MockCheckpointConfig, MockLambdaService};
    use crate::types::{
        ExecutionDetails, InitialExecutionState, InvocationStatus, Operation, OperationStatus,
    };
    use serde_json::json;
    use std::sync::{Arc, Once};

    fn init_aws_env() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            std::env::set_var("AWS_EC2_METADATA_DISABLED", "true");
            std::env::set_var("AWS_REGION", "us-east-1");
            std::env::set_var("AWS_ACCESS_KEY_ID", "test");
            std::env::set_var("AWS_SECRET_ACCESS_KEY", "test");
        });
    }

    fn input_with_payload(payload: Option<String>) -> DurableExecutionInvocationInput {
        DurableExecutionInvocationInput {
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
                        input_payload: payload,
                        output_payload: None,
                    }),
                    context_details: None,
                    chained_invoke_details: None,
                }],
                next_marker: None,
            },
        }
    }

    #[tokio::test]
    async fn test_execute_missing_execution_op_returns_error() {
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
    async fn test_execute_missing_input_payload_returns_error() {
        let input = input_with_payload(None);
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

    #[derive(serde::Deserialize)]
    struct SampleEvent {
        value: u32,
    }

    #[tokio::test]
    async fn test_execute_input_deserialization_failure_returns_error() {
        let input = input_with_payload(Some("{\"value\":\"oops\"}".to_string()));
        let config =
            DurableExecutionConfig::new().with_lambda_service(Arc::new(MockLambdaService::new()));

        let err = execute_durable_handler(
            input,
            |event: SampleEvent, _ctx| async move { Ok(json!({ "ok": event.value })) },
            config,
        )
        .await
        .expect_err("deserialization should fail");

        assert!(err.to_string().contains("Failed to deserialize input"));
    }

    #[tokio::test]
    async fn test_execute_handler_error_returns_failed_output() {
        let input_payload = serde_json::to_string(&json!({"value": 1})).unwrap();
        let input = input_with_payload(Some(input_payload));
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

        assert_eq!(output.status, InvocationStatus::Failed);
        let err = output.error.expect("error object");
        assert!(err.error_message.contains("boom"));
    }

    #[tokio::test]
    async fn test_execute_large_payload_checkpoint_failure_returns_empty_result() {
        let input_payload = serde_json::to_string(&json!({"value": 1})).unwrap();
        let input = input_with_payload(Some(input_payload));

        let mut fail = MockCheckpointConfig::default();
        fail.error = Some(DurableError::Internal("checkpoint failed".to_string()));

        let mock = Arc::new(MockLambdaService::new());
        mock.expect_checkpoint(fail);

        let config = DurableExecutionConfig::new().with_lambda_service(mock);

        let big = "a".repeat(LAMBDA_RESPONSE_SIZE_LIMIT + 64);
        let output = execute_durable_handler(
            input,
            move |_event: serde_json::Value, _ctx| {
                let big = big.clone();
                async move { Ok(json!({ "data": big })) }
            },
            config,
        )
        .await
        .expect("handler should succeed even when checkpoint fails");

        assert_eq!(output.status, InvocationStatus::Succeeded);
        assert_eq!(output.result, Some(String::new()));
    }

    #[tokio::test]
    async fn test_execute_uses_default_lambda_service() {
        init_aws_env();
        let input_payload = serde_json::to_string(&json!({"value": 1})).unwrap();
        let input = input_with_payload(Some(input_payload));

        let output = execute_durable_handler(
            input,
            |_event: serde_json::Value, _ctx| async { Ok(json!({"ok": true})) },
            DurableExecutionConfig::new(),
        )
        .await
        .expect("handler should succeed");

        assert_eq!(output.status, InvocationStatus::Succeeded);
    }

    #[tokio::test]
    async fn test_execute_termination_wait_returns_pending() {
        let input_payload = serde_json::to_string(&json!({"value": 1})).unwrap();
        let input = input_with_payload(Some(input_payload));
        let config =
            DurableExecutionConfig::new().with_lambda_service(Arc::new(MockLambdaService::new()));

        let output = execute_durable_handler(
            input,
            |_event: serde_json::Value, ctx| async move {
                ctx.execution_context()
                    .termination_manager
                    .terminate_for_wait()
                    .await;
                std::future::pending::<DurableResult<serde_json::Value>>().await
            },
            config,
        )
        .await
        .expect("wait termination should return pending");

        assert_eq!(output.status, InvocationStatus::Pending);
    }
}
