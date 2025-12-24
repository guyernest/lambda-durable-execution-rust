use super::super::*;
use crate::retry::RetryStrategy;

pub(super) async fn run_step_execution<T, F, Fut>(
    ctx: &DurableContextImpl,
    name: Option<&str>,
    step_fn: F,
    step_id: String,
    hashed_id: String,
    operation: Option<crate::types::Operation>,
    semantics: StepSemantics,
    retry_strategy: Arc<dyn RetryStrategy>,
    serdes: Option<Arc<dyn Serdes<T>>>,
    parent_id: Option<String>,
) -> DurableResult<T>
where
    T: Serialize + DeserializeOwned + Send + Sync + 'static,
    F: FnOnce(StepContext) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, Box<dyn std::error::Error + Send + Sync>>> + Send + 'static,
{
    // We're executing a new (or interrupted-at-least-once) step now.
    ctx.execution_ctx.set_mode(ExecutionMode::Execution).await;

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
                ctx.execution_ctx
                    .checkpoint_manager
                    .checkpoint(step_id.clone(), start_update)
                    .await?;
            }
            StepSemantics::AtLeastOncePerRetry => {
                // Enqueue without waiting, mirroring JS/Python semantics while preserving ordering
                // with subsequent checkpoints for the same operation.
                ctx.execution_ctx
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
    let mode_now = ctx.execution_ctx.get_mode().await;
    let step_ctx = StepContext::new_with_logger(
        name.map(String::from),
        hashed_id.clone(),
        ctx.execution_ctx.durable_execution_arn.clone(),
        ctx.execution_ctx.logger.clone(),
        mode_now,
        ctx.execution_ctx.mode_aware_logging,
        Some(attempt),
    );

    // Execute step function
    match step_fn(step_ctx).await {
        Ok(result) => {
            // Checkpoint SUCCESS
            let payload =
                safe_serialize(serdes, Some(&result), &hashed_id, name, &ctx.execution_ctx).await;

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

            ctx.execution_ctx
                .checkpoint_manager
                .checkpoint(
                    step_id.clone(),
                    builder.build().map_err(|e| {
                        DurableError::Internal(format!("Failed to build step SUCCEED update: {e}"))
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

                ctx.execution_ctx
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
                ctx.execution_ctx
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

            ctx.execution_ctx
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::BoxError;
    use crate::mock::{MockCheckpointConfig, MockLambdaService};
    use crate::retry::NoRetry;
    use crate::types::{
        DurableExecutionInvocationInput, ExecutionDetails, InitialExecutionState, Operation,
        OperationStatus, OperationType, StepDetails,
    };
    use std::sync::Arc;

    async fn make_execution_context_with_op(
        op: Operation,
    ) -> (ExecutionContext, Arc<MockLambdaService>) {
        let input = DurableExecutionInvocationInput {
            durable_execution_arn: "arn:test:durable".to_string(),
            checkpoint_token: "token-0".to_string(),
            initial_execution_state: InitialExecutionState {
                operations: vec![
                    Operation {
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
                            input_payload: Some("{}".to_string()),
                            output_payload: None,
                        }),
                        context_details: None,
                        chained_invoke_details: None,
                    },
                    op,
                ],
                next_marker: None,
            },
        };

        let lambda_service = Arc::new(MockLambdaService::new());
        let exec_ctx = ExecutionContext::new(&input, lambda_service.clone(), None, true)
            .await
            .expect("execution context should initialize");
        (exec_ctx, lambda_service)
    }

    #[tokio::test]
    async fn test_run_step_execution_skips_start_when_already_started() {
        let step_id = "step_0".to_string();
        let hashed_id = CheckpointManager::hash_id(&step_id);
        let op = Operation {
            id: hashed_id.clone(),
            parent_id: None,
            name: None,
            operation_type: OperationType::Step,
            sub_type: Some("Step".to_string()),
            status: OperationStatus::Started,
            step_details: Some(StepDetails {
                attempt: Some(2),
                next_attempt_timestamp: None,
                result: None,
                error: None,
            }),
            callback_details: None,
            wait_details: None,
            execution_details: None,
            context_details: None,
            chained_invoke_details: None,
        };

        let (exec_ctx, lambda_service) = make_execution_context_with_op(op.clone()).await;
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());

        let ctx = DurableContextImpl::new(exec_ctx);
        let result: u32 = run_step_execution(
            &ctx,
            Some("step"),
            |step_ctx| async move { Ok::<u32, BoxError>(step_ctx.attempt().unwrap_or(0)) },
            step_id.clone(),
            hashed_id.clone(),
            Some(op),
            StepSemantics::AtLeastOncePerRetry,
            Arc::new(NoRetry),
            None,
            None,
        )
        .await
        .expect("step should succeed");

        assert_eq!(result, 3);

        let updates: Vec<_> = lambda_service
            .checkpoint_calls()
            .into_iter()
            .flat_map(|call| call.updates)
            .collect();
        assert!(updates
            .iter()
            .all(|update| update.action != OperationAction::Start));
    }
}
