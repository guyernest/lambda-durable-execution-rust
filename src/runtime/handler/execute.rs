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
