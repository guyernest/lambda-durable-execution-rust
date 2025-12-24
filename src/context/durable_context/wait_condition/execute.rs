use super::super::*;

pub(super) async fn run_wait_condition<T, F, Fut>(
    inner: Arc<DurableContextImpl>,
    name: Option<&str>,
    check_fn: Arc<F>,
    config: WaitConditionConfig<T>,
    step_id: String,
    hashed_id: String,
) -> DurableResult<T>
where
    T: Serialize + DeserializeOwned + Send + Sync + Clone + 'static,
    F: Fn(T, StepContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = DurableResult<T>> + Send + 'static,
{
    let serdes = config.serdes.clone();

    // Get current state from replay if present.
    let mut state = if let Some(op) = inner.execution_ctx.get_step_data(&hashed_id).await {
        if let Some(details) = op.step_details {
            if let Some(payload) = details.result {
                safe_deserialize(
                    serdes.clone(),
                    Some(payload.as_str()),
                    &hashed_id,
                    name,
                    &inner.execution_ctx,
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

    let attempt = inner
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
            inner
                .execution_ctx
                .checkpoint_manager
                .checkpoint(step_id.clone(), fail_update)
                .await?;
            return Err(error);
        }
    }

    // Start step if not already started.
    if inner
        .execution_ctx
        .get_step_data(&hashed_id)
        .await
        .is_none()
    {
        let parent_id = inner.execution_ctx.get_parent_id().await;
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
        inner
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

    let mode_now = inner.execution_ctx.get_mode().await;
    let step_ctx = StepContext::new_with_logger(
        name.map(String::from),
        hashed_id.clone(),
        inner.execution_ctx.durable_execution_arn.clone(),
        inner.execution_ctx.logger.clone(),
        mode_now,
        inner.execution_ctx.mode_aware_logging,
        None,
    );
    let new_state = check_fn(state, step_ctx).await?;
    state = new_state;

    let payload = safe_serialize(
        serdes.clone(),
        Some(&state),
        &hashed_id,
        name,
        &inner.execution_ctx,
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
            inner
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
            inner
                .execution_ctx
                .checkpoint_manager
                .checkpoint(step_id.clone(), retry_update)
                .await?;

            inner
                .execution_ctx
                .termination_manager
                .terminate_for_retry()
                .await;

            std::future::pending::<()>().await;
            unreachable!()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{MockCheckpointConfig, MockLambdaService};
    use crate::termination::TerminationReason;
    use crate::types::{
        DurableExecutionInvocationInput, Duration, ExecutionDetails, InitialExecutionState,
        Operation, OperationStatus, OperationType, StepDetails,
    };
    use std::sync::Arc;
    use std::time::Duration as StdDuration;

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

    async fn make_execution_context() -> (ExecutionContext, Arc<MockLambdaService>) {
        let input = DurableExecutionInvocationInput {
            durable_execution_arn: "arn:test:durable".to_string(),
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
                        input_payload: Some("{}".to_string()),
                        output_payload: None,
                    }),
                    context_details: None,
                    chained_invoke_details: None,
                }],
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
    async fn test_run_wait_condition_skips_start_when_operation_exists() {
        let step_id = "wait_0".to_string();
        let hashed_id = CheckpointManager::hash_id(&step_id);
        let op = Operation {
            id: hashed_id.clone(),
            parent_id: None,
            name: None,
            operation_type: OperationType::Step,
            sub_type: Some("WaitForCondition".to_string()),
            status: OperationStatus::Started,
            step_details: Some(StepDetails {
                attempt: Some(0),
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

        let (exec_ctx, lambda_service) = make_execution_context_with_op(op).await;
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());

        let ctx = Arc::new(DurableContextImpl::new(exec_ctx));
        let config = WaitConditionConfig::new(
            5u32,
            Arc::new(|_state: &u32, _attempt: u32| WaitConditionDecision::Stop),
        );

        let value = run_wait_condition(
            ctx,
            Some("wait"),
            Arc::new(|state, _step_ctx| async move { Ok(state + 1) }),
            config,
            step_id,
            hashed_id,
        )
        .await
        .expect("wait_for_condition should succeed");

        assert_eq!(value, 6);

        let updates: Vec<_> = lambda_service
            .checkpoint_calls()
            .into_iter()
            .flat_map(|call| call.updates)
            .collect();
        assert!(updates
            .iter()
            .all(|update| update.action != OperationAction::Start));
    }

    #[tokio::test]
    async fn test_run_wait_condition_starts_and_succeeds() {
        let step_id = "wait_0".to_string();
        let hashed_id = CheckpointManager::hash_id(&step_id);

        let (exec_ctx, lambda_service) = make_execution_context().await;
        for _ in 0..2 {
            lambda_service.expect_checkpoint(MockCheckpointConfig::default());
        }

        let ctx = Arc::new(DurableContextImpl::new(exec_ctx));
        let config = WaitConditionConfig::new(
            0u32,
            Arc::new(|_state: &u32, _attempt: u32| WaitConditionDecision::Stop),
        );

        let value = run_wait_condition(
            ctx,
            Some("wait"),
            Arc::new(|state, _step_ctx| async move { Ok(state + 2) }),
            config,
            step_id,
            hashed_id,
        )
        .await
        .expect("wait_for_condition should succeed");

        assert_eq!(value, 2);

        let updates: Vec<_> = lambda_service
            .checkpoint_calls()
            .into_iter()
            .flat_map(|call| call.updates)
            .collect();
        assert!(updates.iter().any(|update| update.action == OperationAction::Start));
        assert!(updates
            .iter()
            .any(|update| update.action == OperationAction::Succeed));
    }

    #[tokio::test]
    async fn test_run_wait_condition_retry_terminates() {
        let step_id = "wait_0".to_string();
        let hashed_id = CheckpointManager::hash_id(&step_id);

        let (exec_ctx, lambda_service) = make_execution_context().await;
        for _ in 0..2 {
            lambda_service.expect_checkpoint(MockCheckpointConfig::default());
        }

        let ctx = Arc::new(DurableContextImpl::new(exec_ctx));
        let config = WaitConditionConfig::new(
            1u32,
            Arc::new(|_state: &u32, _attempt: u32| {
                WaitConditionDecision::Continue {
                    delay: Duration::seconds(3),
                }
            }),
        );

        let result = tokio::time::timeout(
            StdDuration::from_millis(50),
            run_wait_condition(
                Arc::clone(&ctx),
                Some("wait"),
                Arc::new(|state, _step_ctx| async move { Ok(state + 1) }),
                config,
                step_id,
                hashed_id,
            ),
        )
        .await;

        assert!(result.is_err(), "wait_for_condition should suspend");

        let termination = ctx
            .execution_ctx
            .termination_manager
            .get_termination_result()
            .expect("termination should be recorded");
        assert_eq!(termination.reason, TerminationReason::RetryScheduled);

        let updates: Vec<_> = lambda_service
            .checkpoint_calls()
            .into_iter()
            .flat_map(|call| call.updates)
            .collect();
        assert!(updates.iter().any(|update| update.action == OperationAction::Retry));
    }

    #[tokio::test]
    async fn test_run_wait_condition_max_attempts_exceeded() {
        let step_id = "wait_0".to_string();
        let hashed_id = CheckpointManager::hash_id(&step_id);
        let op = Operation {
            id: hashed_id.clone(),
            parent_id: None,
            name: None,
            operation_type: OperationType::Step,
            sub_type: Some("WaitForCondition".to_string()),
            status: OperationStatus::Started,
            step_details: Some(StepDetails {
                attempt: Some(3),
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

        let (exec_ctx, lambda_service) = make_execution_context_with_op(op).await;
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());

        let ctx = Arc::new(DurableContextImpl::new(exec_ctx));
        let config = WaitConditionConfig::new(
            0u32,
            Arc::new(|_state: &u32, _attempt: u32| WaitConditionDecision::Stop),
        )
        .with_max_attempts(3);

        let err = run_wait_condition(
            ctx,
            Some("wait"),
            Arc::new(|state, _step_ctx| async move { Ok(state + 1) }),
            config,
            step_id,
            hashed_id,
        )
        .await
        .expect_err("wait_for_condition should fail when max attempts exceeded");

        match err {
            DurableError::WaitConditionExceeded { attempts, .. } => {
                assert_eq!(attempts, 4);
            }
            other => panic!("unexpected error: {other:?}"),
        }

        let updates: Vec<_> = lambda_service
            .checkpoint_calls()
            .into_iter()
            .flat_map(|call| call.updates)
            .collect();
        assert!(updates.iter().any(|update| update.action == OperationAction::Fail));
    }
}
