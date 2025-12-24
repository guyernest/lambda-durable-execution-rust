use super::super::*;

pub(super) enum ChildReplayDecision<T> {
    Return(T),
    ReplayChildren,
    Continue,
}

pub(super) async fn evaluate_replay<T>(
    operation: Option<crate::types::Operation>,
    serdes: Option<Arc<dyn Serdes<T>>>,
    hashed_id: &str,
    step_id: &str,
    name: Option<&str>,
    execution_ctx: &ExecutionContext,
) -> DurableResult<ChildReplayDecision<T>>
where
    T: DeserializeOwned + Send + Sync + 'static,
{
    let Some(operation) = operation else {
        return Ok(ChildReplayDecision::Continue);
    };

    match operation.status {
        OperationStatus::Succeeded => {
            if operation
                .context_details
                .as_ref()
                .and_then(|d| d.replay_children)
                == Some(true)
            {
                return Ok(ChildReplayDecision::ReplayChildren);
            }

            if let Some(ref details) = operation.context_details {
                if let Some(ref payload) = details.result {
                    if let Some(val) = safe_deserialize(
                        serdes.clone(),
                        Some(payload.as_str()),
                        hashed_id,
                        name,
                        execution_ctx,
                    )
                    .await
                    {
                        return Ok(ChildReplayDecision::Return(val));
                    }
                }
            }

            // Fallback for older payload locations.
            if let Some(ref details) = operation.execution_details {
                if let Some(ref payload) = details.output_payload {
                    if let Some(val) = safe_deserialize(
                        serdes,
                        Some(payload.as_str()),
                        hashed_id,
                        name,
                        execution_ctx,
                    )
                    .await
                    {
                        return Ok(ChildReplayDecision::Return(val));
                    }
                }
            }

            Ok(ChildReplayDecision::Continue)
        }
        OperationStatus::Failed => {
            let msg = operation
                .context_details
                .as_ref()
                .and_then(|d| d.error.as_ref())
                .map(|e| e.error_message.clone())
                .unwrap_or_else(|| "Child context failed".to_string());
            Err(DurableError::ChildContextFailed {
                name: step_id.to_string(),
                message: msg,
                source: None,
            })
        }
        _ => {
            execution_ctx.set_mode(ExecutionMode::Execution).await;
            Ok(ChildReplayDecision::Continue)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ExecutionMode;
    use crate::mock::MockLambdaService;
    use crate::types::{
        ContextDetails, DurableExecutionInvocationInput, ExecutionDetails, InitialExecutionState,
        Operation, OperationStatus, OperationType,
    };
    use std::sync::Arc;

    async fn make_execution_context() -> ExecutionContext {
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
        ExecutionContext::new(&input, lambda_service, None, true)
            .await
            .expect("execution context should initialize")
    }

    #[tokio::test]
    async fn test_evaluate_replay_none_returns_continue() {
        let ctx = make_execution_context().await;

        let decision = evaluate_replay::<u32>(None, None, "hashed", "child_0", None, &ctx)
            .await
            .unwrap();

        match decision {
            ChildReplayDecision::Continue => {}
            _ => panic!("unexpected decision"),
        }
    }

    #[tokio::test]
    async fn test_evaluate_replay_started_sets_mode() {
        let ctx = make_execution_context().await;
        ctx.set_mode(ExecutionMode::Replay).await;

        let op = Operation {
            id: "child-op".to_string(),
            parent_id: None,
            name: None,
            operation_type: OperationType::Context,
            sub_type: None,
            status: OperationStatus::Started,
            step_details: None,
            callback_details: None,
            wait_details: None,
            execution_details: None,
            context_details: Some(ContextDetails {
                result: None,
                error: None,
                replay_children: None,
            }),
            chained_invoke_details: None,
        };

        let decision = evaluate_replay::<u32>(Some(op), None, "hashed", "child_0", None, &ctx)
            .await
            .unwrap();

        match decision {
            ChildReplayDecision::Continue => {}
            _ => panic!("unexpected decision"),
        }

        let mode = ctx.get_mode().await;
        assert_eq!(mode, ExecutionMode::Execution);
    }
}
