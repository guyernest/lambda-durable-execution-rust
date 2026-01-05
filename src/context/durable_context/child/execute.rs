use super::super::*;

pub(super) async fn run_child_execution<T, F, Fut>(
    ctx: &DurableContextImpl,
    step_id: String,
    hashed_id: String,
    name: Option<&str>,
    context_fn: F,
    sub_type: String,
    serdes: Option<Arc<dyn Serdes<T>>>,
) -> DurableResult<T>
where
    T: Serialize + DeserializeOwned + Send + Sync + 'static,
    F: FnOnce(DurableContextHandle) -> Fut + Send + 'static,
    Fut: Future<Output = DurableResult<T>> + Send + 'static,
{
    // Checkpoint at start if not already started. This ensures any child operations that
    // reference `ParentId` (this context) are valid to the backend.
    if ctx.execution_ctx.get_step_data(&hashed_id).await.is_none() {
        let parent_id = ctx.execution_ctx.get_parent_id();
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

        ctx.execution_ctx
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
    let child_execution_ctx = ctx.execution_ctx.with_parent_id(hashed_id.clone());
    let child_impl = Arc::new(DurableContextImpl::new(child_execution_ctx));
    let child_ctx = DurableContextHandle::new(child_impl);

    // Execute child context
    let result = match context_fn(child_ctx).await {
        Ok(val) => val,
        Err(error) => {
            let err_obj = ErrorObject::from_durable_error(&error);
            let parent_id = ctx.execution_ctx.get_parent_id();

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

            ctx.execution_ctx
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
        safe_serialize(serdes, Some(&result), &hashed_id, name, &ctx.execution_ctx).await;
    let mut replay_children = false;
    if let Some(ref p) = payload {
        if p.len() > CHECKPOINT_SIZE_LIMIT_BYTES {
            replay_children = true;
            payload = Some(String::new());
        }
    }

    let parent_id = ctx.execution_ctx.get_parent_id();
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
    ctx.execution_ctx
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{MockCheckpointConfig, MockLambdaService};
    use crate::types::{
        DurableExecutionInvocationInput, ExecutionDetails, InitialExecutionState, Operation,
        OperationStatus, OperationType,
    };
    use serde_json::json;
    use std::sync::Arc;

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
                        input_payload: Some(json!({}).to_string()),
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
    async fn test_run_child_execution_success_direct() {
        let step_id = "child_0".to_string();
        let hashed_id = CheckpointManager::hash_id(&step_id);

        let (exec_ctx, lambda_service) = make_execution_context().await;
        for _ in 0..2 {
            lambda_service.expect_checkpoint(MockCheckpointConfig::default());
        }

        let ctx = DurableContextImpl::new(exec_ctx);
        let value: u32 = run_child_execution(
            &ctx,
            step_id,
            hashed_id,
            Some("child"),
            |_child_ctx| async move { Ok(7u32) },
            "RunInChildContext".to_string(),
            None,
        )
        .await
        .expect("child should succeed");

        assert_eq!(value, 7);
    }

    #[tokio::test]
    async fn test_run_child_execution_failure_direct() {
        let step_id = "child_0".to_string();
        let hashed_id = CheckpointManager::hash_id(&step_id);

        let (exec_ctx, lambda_service) = make_execution_context().await;
        for _ in 0..2 {
            lambda_service.expect_checkpoint(MockCheckpointConfig::default());
        }

        let ctx = DurableContextImpl::new(exec_ctx);
        let err = run_child_execution(
            &ctx,
            step_id,
            hashed_id,
            Some("child"),
            |_child_ctx| async move { Err(DurableError::Internal("boom".to_string())) },
            "RunInChildContext".to_string(),
            None::<Arc<dyn Serdes<u32>>>,
        )
        .await
        .expect_err("child should fail");

        match err {
            DurableError::ChildContextFailed { message, .. } => {
                assert!(message.contains("boom"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
