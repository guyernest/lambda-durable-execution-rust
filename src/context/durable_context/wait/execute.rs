use super::super::*;

pub(super) async fn run_wait(
    ctx: &DurableContextImpl,
    name: Option<&str>,
    duration: Duration,
    step_id: String,
    hashed_id: String,
) -> DurableResult<()> {
    // If the requested duration is zero, avoid scheduling/suspending.
    // Returning PENDING with no pending ops is invalid and can happen if the wait completes immediately.
    if duration.is_zero() {
        return Ok(());
    }

    // Replay handling: if the wait already exists, never re-start it.
    if let Some(operation) = ctx.execution_ctx.get_step_data(&hashed_id).await {
        match operation.status {
            OperationStatus::Succeeded => return Ok(()),
            OperationStatus::Failed => {
                return Err(DurableError::Internal("Wait failed in replay".to_string()))
            }
            _ => {
                ctx.execution_ctx.set_mode(ExecutionMode::Execution).await;
                ctx.execution_ctx
                    .termination_manager
                    .terminate_for_wait()
                    .await;
                std::future::pending::<()>().await;
                unreachable!()
            }
        }
    }

    // Checkpoint the wait
    let parent_id = ctx.execution_ctx.get_parent_id().await;
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

    ctx.execution_ctx
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
    if let Some(operation) = ctx.execution_ctx.get_step_data(&hashed_id).await {
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
    ctx.execution_ctx
        .checkpoint_manager
        .mark_awaited(&hashed_id)
        .await;

    // Trigger termination for wait
    ctx.execution_ctx
        .termination_manager
        .terminate_for_wait()
        .await;

    // This point is never reached during normal execution
    std::future::pending::<()>().await;
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockLambdaService;
    use crate::termination::TerminationReason;
    use crate::types::{
        DurableExecutionInvocationInput, ExecutionDetails, InitialExecutionState, Operation,
        OperationStatus, OperationType,
    };
    use serde_json::json;
    use std::sync::Arc;

    async fn make_execution_context_with_ops(
        operations: Vec<Operation>,
    ) -> (ExecutionContext, Arc<MockLambdaService>) {
        let mut ops = vec![Operation {
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
                input_payload: Some(json!({}).to_string()),
                output_payload: None,
            }),
            context_details: None,
            chained_invoke_details: None,
        }];
        ops.extend(operations);

        let input = DurableExecutionInvocationInput {
            durable_execution_arn: "arn:test:durable".to_string(),
            checkpoint_token: "token-0".to_string(),
            initial_execution_state: InitialExecutionState {
                operations: ops,
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
    async fn test_run_wait_replay_failed_returns_error() {
        let step_id = "wait_0".to_string();
        let hashed_id = CheckpointManager::hash_id(&step_id);
        let op = Operation {
            id: hashed_id.clone(),
            parent_id: None,
            name: None,
            operation_type: OperationType::Wait,
            sub_type: Some("Wait".to_string()),
            status: OperationStatus::Failed,
            step_details: None,
            callback_details: None,
            wait_details: None,
            execution_details: None,
            context_details: None,
            chained_invoke_details: None,
        };

        let (exec_ctx, _lambda_service) = make_execution_context_with_ops(vec![op]).await;
        let ctx = DurableContextImpl::new(exec_ctx);

        let err = run_wait(&ctx, Some("wait"), Duration::seconds(5), step_id, hashed_id)
            .await
            .expect_err("wait should fail in replay");

        assert!(err.to_string().contains("Wait failed in replay"));
    }

    #[tokio::test]
    async fn test_run_wait_replay_succeeded_returns_ok() {
        let step_id = "wait_0".to_string();
        let hashed_id = CheckpointManager::hash_id(&step_id);
        let op = Operation {
            id: hashed_id.clone(),
            parent_id: None,
            name: None,
            operation_type: OperationType::Wait,
            sub_type: Some("Wait".to_string()),
            status: OperationStatus::Succeeded,
            step_details: None,
            callback_details: None,
            wait_details: None,
            execution_details: None,
            context_details: None,
            chained_invoke_details: None,
        };

        let (exec_ctx, _lambda_service) = make_execution_context_with_ops(vec![op]).await;
        let ctx = DurableContextImpl::new(exec_ctx);

        run_wait(&ctx, Some("wait"), Duration::seconds(5), step_id, hashed_id)
            .await
            .expect("wait should return ok when already succeeded");
    }

    #[tokio::test]
    async fn test_run_wait_replay_pending_suspends() {
        let step_id = "wait_0".to_string();
        let hashed_id = CheckpointManager::hash_id(&step_id);
        let op = Operation {
            id: hashed_id.clone(),
            parent_id: None,
            name: None,
            operation_type: OperationType::Wait,
            sub_type: Some("Wait".to_string()),
            status: OperationStatus::Started,
            step_details: None,
            callback_details: None,
            wait_details: None,
            execution_details: None,
            context_details: None,
            chained_invoke_details: None,
        };

        let (exec_ctx, _lambda_service) = make_execution_context_with_ops(vec![op]).await;
        let ctx = DurableContextImpl::new(exec_ctx);

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            run_wait(&ctx, Some("wait"), Duration::seconds(5), step_id, hashed_id),
        )
        .await;

        assert!(result.is_err(), "wait should suspend on replay pending");

        let termination = ctx
            .execution_ctx
            .termination_manager
            .get_termination_result()
            .expect("termination should be recorded");
        assert_eq!(termination.reason, TerminationReason::WaitScheduled);
    }
}
