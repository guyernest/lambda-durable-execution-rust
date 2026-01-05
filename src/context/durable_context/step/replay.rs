use super::super::*;
use super::StepInterruptedError;
use crate::retry::RetryStrategy;

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_replay<T>(
    operation: Option<&crate::types::Operation>,
    step_id: &str,
    hashed_id: &str,
    name: Option<&str>,
    semantics: StepSemantics,
    retry_strategy: &Arc<dyn RetryStrategy>,
    serdes: Option<Arc<dyn Serdes<T>>>,
    parent_id: Option<&str>,
    execution_ctx: &ExecutionContext,
) -> DurableResult<Option<T>>
where
    T: DeserializeOwned + Send + Sync + 'static,
{
    let Some(op) = operation else {
        return Ok(None);
    };

    match op.status {
        OperationStatus::Succeeded => {
            if let Some(ref details) = op.step_details {
                if let Some(ref payload) = details.result {
                    if let Some(val) = safe_deserialize(
                        serdes,
                        Some(payload.as_str()),
                        hashed_id,
                        name,
                        execution_ctx,
                    )
                    .await
                    {
                        return Ok(Some(val));
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
            return Err(DurableError::step_failed_msg(
                step_id.to_string(),
                attempts,
                message,
            ));
        }
        OperationStatus::Pending => {
            execution_ctx.set_mode(ExecutionMode::Execution).await;
            execution_ctx
                .termination_manager
                .terminate_for_retry()
                .await;
            std::future::pending::<()>().await;
            unreachable!()
        }
        OperationStatus::Started if semantics == StepSemantics::AtMostOncePerRetry => {
            execution_ctx.set_mode(ExecutionMode::Execution).await;

            let attempt_idx = op
                .step_details
                .as_ref()
                .and_then(|d| d.attempt)
                .unwrap_or(0);
            let attempts_made = attempt_idx + 1;
            let interrupted = StepInterruptedError {
                step_id: step_id.to_string(),
                name: name.map(|s| s.to_string()),
            };

            let decision = retry_strategy.should_retry(&interrupted, attempts_made);
            let err_obj = ErrorObject::from_error(&interrupted);

            if !decision.should_retry {
                let mut builder = OperationUpdate::builder()
                    .id(hashed_id)
                    .operation_type(OperationType::Step)
                    .sub_type("Step")
                    .action(OperationAction::Fail)
                    .error(err_obj);
                if let Some(pid) = parent_id {
                    builder = builder.parent_id(pid);
                }
                if let Some(n) = name {
                    builder = builder.name(n);
                }
                execution_ctx
                    .checkpoint_manager
                    .checkpoint(
                        step_id.to_string(),
                        builder.build().map_err(|e| {
                            DurableError::Internal(format!("Failed to build step FAIL update: {e}"))
                        })?,
                    )
                    .await?;

                return Err(DurableError::step_failed_msg(
                    step_id.to_string(),
                    attempts_made,
                    interrupted.to_string(),
                ));
            }

            let delay = decision.delay.unwrap_or(Duration::seconds(1));
            let mut builder = OperationUpdate::builder()
                .id(hashed_id)
                .operation_type(OperationType::Step)
                .sub_type("Step")
                .action(OperationAction::Retry)
                .error(err_obj)
                .step_options(crate::types::StepUpdateOptions {
                    next_attempt_delay_seconds: Some(delay.to_seconds_i32_saturating()),
                });
            if let Some(pid) = parent_id {
                builder = builder.parent_id(pid);
            }
            if let Some(n) = name {
                builder = builder.name(n);
            }

            execution_ctx
                .checkpoint_manager
                .checkpoint(
                    step_id.to_string(),
                    builder.build().map_err(|e| {
                        DurableError::Internal(format!("Failed to build step RETRY update: {e}"))
                    })?,
                )
                .await?;

            execution_ctx
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

    Ok(None)
}
