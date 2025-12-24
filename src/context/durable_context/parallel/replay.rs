use super::super::*;

pub(super) async fn maybe_replay_parallel<T, F>(
    name: Option<&str>,
    branches: &[NamedParallelBranch<F>],
    batch_serdes: &Option<Arc<dyn Serdes<BatchResult<T>>>>,
    item_serdes: &Option<Arc<dyn Serdes<T>>>,
    execution_ctx: &ExecutionContext,
    par_hashed_id: &str,
    completion_config: &crate::types::CompletionConfig,
) -> DurableResult<Option<BatchResult<T>>>
where
    T: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    let Some(op) = execution_ctx.get_step_data(par_hashed_id).await else {
        return Ok(None);
    };

    match op.status {
        OperationStatus::Failed => {
            let msg = op
                .context_details
                .as_ref()
                .and_then(|d| d.error.as_ref())
                .map(|e| e.error_message.clone())
                .unwrap_or_else(|| "Batch operation failed".to_string());
            return Err(DurableError::BatchOperationFailed {
                name: name.unwrap_or("parallel").to_string(),
                message: msg,
                successful_count: 0,
                failed_count: 0,
            });
        }
        OperationStatus::Succeeded => {
            if let Some(payload) = op.context_details.as_ref().and_then(|d| d.result.as_ref()) {
                if let Some(batch_serdes) = batch_serdes.clone() {
                    let batch: BatchResult<T> = safe_deserialize_required_with_serdes(
                        batch_serdes,
                        payload,
                        par_hashed_id,
                        name,
                        execution_ctx,
                    )
                    .await;

                    let target_total_count = batch
                        .all
                        .iter()
                        .map(|i| i.index)
                        .max()
                        .map(|m| m + 1)
                        .unwrap_or(0);

                    if target_total_count > branches.len() {
                        return Err(DurableError::ReplayValidationFailed {
                            expected: format!("parallel totalCount <= {}", branches.len()),
                            actual: target_total_count.to_string(),
                        });
                    }

                    // Consume child context operation IDs to keep the parent context counter in sync.
                    for (index, branch) in branches.iter().enumerate().take(target_total_count) {
                        let base = name.unwrap_or("parallel");
                        let branch_name = branch
                            .name
                            .clone()
                            .unwrap_or_else(|| format!("{}-branch-{}", base, index));
                        let _ = execution_ctx.next_operation_id(Some(&branch_name));
                    }

                    return Ok(Some(batch));
                }
            }

            let target_total_count = op
                .context_details
                .as_ref()
                .and_then(|d| d.result.as_ref())
                .and_then(|payload| {
                    serde_json::from_str::<serde_json::Value>(payload)
                        .ok()
                        .and_then(|v| v.get("totalCount").and_then(|tc| tc.as_u64()))
                })
                .map(|tc| tc as usize);

            if let Some(target_total_count) = target_total_count {
                let mut successes = Vec::new();
                let mut failures = Vec::new();
                let mut started = Vec::new();
                let mut seen_count = 0usize;

                for (index, branch) in branches.iter().enumerate() {
                    if seen_count >= target_total_count {
                        break;
                    }

                    let base = name.unwrap_or("parallel");
                    let branch_name = branch
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("{}-branch-{}", base, index));

                    let child_step_id = execution_ctx.next_operation_id(Some(&branch_name));
                    let child_hashed_id = DurableContextImpl::hash_id(&child_step_id);

                    if let Some(child_op) = execution_ctx.get_step_data(&child_hashed_id).await {
                        seen_count += 1;

                        match child_op.status {
                            OperationStatus::Succeeded => {
                                if let Some(ref details) = child_op.context_details {
                                    if let Some(ref payload) = details.result {
                                        let val: T = safe_deserialize(
                                            item_serdes.clone(),
                                            Some(payload.as_str()),
                                            &child_hashed_id,
                                            Some(&branch_name),
                                            execution_ctx,
                                        )
                                        .await
                                        .ok_or_else(|| {
                                            DurableError::Internal(
                                                "Missing child context output in replay"
                                                    .to_string(),
                                            )
                                        })?;
                                        successes.push((index, val));
                                    }
                                }
                            }
                            OperationStatus::Failed => {
                                let msg = child_op
                                    .context_details
                                    .as_ref()
                                    .and_then(|d| d.error.as_ref())
                                    .map(|e| e.error_message.clone())
                                    .unwrap_or_else(|| "Child context failed".to_string());
                                failures.push((
                                    index,
                                    DurableError::ChildContextFailed {
                                        name: child_step_id,
                                        message: msg,
                                        source: None,
                                    },
                                ));
                            }
                            _ => started.push(index),
                        }
                    }
                }

                let completed_count = successes.len() + failures.len();
                let completion_reason = compute_batch_completion_reason(
                    failures.len(),
                    successes.len(),
                    completed_count,
                    branches.len(),
                    completion_config,
                );

                let mut all = Vec::new();
                for (i, v) in successes {
                    all.push(BatchItem {
                        index: i,
                        status: BatchItemStatus::Succeeded,
                        result: Some(v),
                        error: None,
                    });
                }
                for (i, e) in failures {
                    all.push(BatchItem {
                        index: i,
                        status: BatchItemStatus::Failed,
                        result: None,
                        error: Some(Arc::new(e)),
                    });
                }
                for i in started {
                    all.push(BatchItem {
                        index: i,
                        status: BatchItemStatus::Started,
                        result: None,
                        error: None,
                    });
                }
                all.sort_by_key(|i| i.index);

                return Ok(Some(BatchResult {
                    all,
                    completion_reason,
                }));
            }
        }
        _ => {
            execution_ctx.set_mode(ExecutionMode::Execution).await;
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockLambdaService;
    use crate::types::{
        BatchCompletionReason, BatchItem, BatchItemStatus, CompletionConfig,
        DurableExecutionInvocationInput, NamedParallelBranch, SerdesContext,
    };
    use async_trait::async_trait;
    use serde::Serialize;
    use serde_json::json;
    use std::sync::Arc;

    struct StaticBatchSerdes<T> {
        items: Vec<(usize, BatchItemStatus, Option<T>)>,
        completion_reason: BatchCompletionReason,
    }

    #[async_trait]
    impl<T: Clone + Send + Sync> Serdes<BatchResult<T>> for StaticBatchSerdes<T> {
        async fn serialize(
            &self,
            _value: Option<&BatchResult<T>>,
            _context: SerdesContext,
        ) -> Result<Option<String>, crate::error::BoxError> {
            Ok(Some("payload".to_string()))
        }

        async fn deserialize(
            &self,
            _data: Option<&str>,
            _context: SerdesContext,
        ) -> Result<Option<BatchResult<T>>, crate::error::BoxError> {
            let mut all = Vec::new();
            for (index, status, result) in &self.items {
                all.push(BatchItem {
                    index: *index,
                    status: *status,
                    result: result.clone(),
                    error: None,
                });
            }
            Ok(Some(BatchResult {
                all,
                completion_reason: self.completion_reason,
            }))
        }
    }

    fn create_replay_input<T: Serialize>(
        durable_execution_arn: &str,
        input: &T,
        operations: Vec<serde_json::Value>,
    ) -> serde_json::Value {
        let input_payload = serde_json::to_string(input).expect("serialize test input");

        let mut ops = vec![json!({
            "Id": "execution",
            "Type": "EXECUTION",
            "Status": "STARTED",
            "ExecutionDetails": {
                "InputPayload": input_payload
            }
        })];
        ops.extend(operations);

        json!({
            "DurableExecutionArn": durable_execution_arn,
            "CheckpointToken": "test-token-123",
            "InitialExecutionState": {
                "Operations": ops,
                "NextMarker": null
            }
        })
    }

    async fn make_execution_context(
        durable_execution_arn: &str,
        operations: Vec<serde_json::Value>,
    ) -> ExecutionContext {
        let input_json = create_replay_input(durable_execution_arn, &json!({}), operations);
        let input: DurableExecutionInvocationInput =
            serde_json::from_value(input_json).expect("valid invocation input");

        ExecutionContext::new(&input, Arc::new(MockLambdaService::new()), None, true)
            .await
            .expect("execution context should initialize")
    }
    #[tokio::test]
    async fn test_maybe_replay_parallel_returns_none_without_operation() {
        let arn = "arn:test:durable";
        let execution_ctx = make_execution_context(arn, vec![]).await;

        let step_id = "parallel_0".to_string();
        let hashed_id = CheckpointManager::hash_id(&step_id);
        fn branch_fn(
            _ctx: DurableContextHandle,
        ) -> BoxFuture<'static, crate::error::DurableResult<u32>> {
            Box::pin(async move { Ok(1) })
        }
        let branches: Vec<NamedParallelBranch<_>> = vec![NamedParallelBranch::new(branch_fn)];

        let result = maybe_replay_parallel::<u32, _>(
            Some("parallel"),
            &branches,
            &None,
            &None,
            &execution_ctx,
            &hashed_id,
            &CompletionConfig::new(),
        )
        .await
        .unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_maybe_replay_parallel_started_sets_execution_mode() {
        let arn = "arn:test:durable";
        let step_id = "parallel_0".to_string();
        let hashed_id = CheckpointManager::hash_id(&step_id);
        let op = json!({
            "Id": hashed_id,
            "Type": "CONTEXT",
            "SubType": "Parallel",
            "Status": "STARTED",
        });

        let execution_ctx = make_execution_context(arn, vec![op]).await;
        assert_eq!(execution_ctx.get_mode().await, ExecutionMode::Replay);

        fn branch_fn(
            _ctx: DurableContextHandle,
        ) -> BoxFuture<'static, crate::error::DurableResult<u32>> {
            Box::pin(async move { Ok(1) })
        }
        let branches: Vec<NamedParallelBranch<_>> = vec![NamedParallelBranch::new(branch_fn)];

        let result = maybe_replay_parallel::<u32, _>(
            Some("parallel"),
            &branches,
            &None,
            &None,
            &execution_ctx,
            &hashed_id,
            &CompletionConfig::new(),
        )
        .await
        .unwrap();

        assert!(result.is_none());
        assert_eq!(execution_ctx.get_mode().await, ExecutionMode::Execution);
    }

    #[tokio::test]
    async fn test_maybe_replay_parallel_batch_serdes_returns_batch() {
        let arn = "arn:test:durable";
        let step_id = "parallel_0".to_string();
        let hashed_id = CheckpointManager::hash_id(&step_id);
        let op = json!({
            "Id": hashed_id,
            "Type": "CONTEXT",
            "SubType": "Parallel",
            "Status": "SUCCEEDED",
            "ContextDetails": { "Result": "payload" },
        });

        let execution_ctx = make_execution_context(arn, vec![op]).await;
        let batch_serdes = StaticBatchSerdes::<u32> {
            items: vec![
                (0, BatchItemStatus::Succeeded, Some(10)),
                (1, BatchItemStatus::Failed, None),
            ],
            completion_reason: BatchCompletionReason::FailureToleranceExceeded,
        };

        fn branch_fn(
            _ctx: DurableContextHandle,
        ) -> BoxFuture<'static, crate::error::DurableResult<u32>> {
            Box::pin(async move { Ok(1) })
        }
        let branches: Vec<NamedParallelBranch<_>> = vec![
            NamedParallelBranch::new(branch_fn),
            NamedParallelBranch::new(branch_fn),
        ];

        let result = maybe_replay_parallel::<u32, _>(
            Some("parallel"),
            &branches,
            &Some(Arc::new(batch_serdes)),
            &None,
            &execution_ctx,
            &hashed_id,
            &CompletionConfig::new(),
        )
        .await
        .unwrap()
        .expect("batch result");

        assert_eq!(
            result.completion_reason,
            BatchCompletionReason::FailureToleranceExceeded
        );
        assert_eq!(result.all.len(), 2);
    }

    #[tokio::test]
    async fn test_maybe_replay_parallel_total_count_exceeds_branches_returns_error() {
        let arn = "arn:test:durable";
        let step_id = "parallel_0".to_string();
        let hashed_id = CheckpointManager::hash_id(&step_id);
        let payload = json!({ "totalCount": 2 }).to_string();
        let op = json!({
            "Id": hashed_id,
            "Type": "CONTEXT",
            "SubType": "Parallel",
            "Status": "SUCCEEDED",
            "ContextDetails": { "Result": payload },
        });

        let execution_ctx = make_execution_context(arn, vec![op]).await;
        fn branch_fn(
            _ctx: DurableContextHandle,
        ) -> BoxFuture<'static, crate::error::DurableResult<u32>> {
            Box::pin(async move { Ok(1) })
        }
        let branches: Vec<NamedParallelBranch<_>> = vec![NamedParallelBranch::new(branch_fn)];

        let result = maybe_replay_parallel::<u32, _>(
            Some("parallel"),
            &branches,
            &None,
            &None,
            &execution_ctx,
            &hashed_id,
            &CompletionConfig::new(),
        )
        .await
        .unwrap()
        .expect("batch result");

        assert!(result.all.is_empty());
        assert_eq!(
            result.completion_reason,
            BatchCompletionReason::AllCompleted
        );
    }

    #[tokio::test]
    async fn test_maybe_replay_parallel_succeeded_missing_result_returns_none() {
        let arn = "arn:test:durable";
        let step_id = "parallel_0".to_string();
        let hashed_id = CheckpointManager::hash_id(&step_id);
        let op = json!({
            "Id": hashed_id,
            "Type": "CONTEXT",
            "SubType": "Parallel",
            "Status": "SUCCEEDED",
        });

        let execution_ctx = make_execution_context(arn, vec![op]).await;
        fn branch_fn(
            _ctx: DurableContextHandle,
        ) -> BoxFuture<'static, crate::error::DurableResult<u32>> {
            Box::pin(async move { Ok(1) })
        }
        let branches: Vec<NamedParallelBranch<_>> = vec![NamedParallelBranch::new(branch_fn)];

        let result = maybe_replay_parallel::<u32, _>(
            Some("parallel"),
            &branches,
            &None,
            &None,
            &execution_ctx,
            &hashed_id,
            &CompletionConfig::new(),
        )
        .await
        .unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_maybe_replay_parallel_missing_child_operation_returns_batch() {
        let arn = "arn:test:durable";
        let step_id = "parallel_0".to_string();
        let hashed_id = CheckpointManager::hash_id(&step_id);
        let payload = json!({ "totalCount": 1 }).to_string();
        let op = json!({
            "Id": hashed_id,
            "Type": "CONTEXT",
            "SubType": "Parallel",
            "Status": "SUCCEEDED",
            "ContextDetails": { "Result": payload },
        });

        let execution_ctx = make_execution_context(arn, vec![op]).await;
        fn branch_fn(
            _ctx: DurableContextHandle,
        ) -> BoxFuture<'static, crate::error::DurableResult<u32>> {
            Box::pin(async move { Ok(1) })
        }
        let branches: Vec<NamedParallelBranch<_>> = vec![NamedParallelBranch::new(branch_fn)];

        let result = maybe_replay_parallel::<u32, _>(
            Some("parallel"),
            &branches,
            &None,
            &None,
            &execution_ctx,
            &hashed_id,
            &CompletionConfig::new(),
        )
        .await
        .unwrap()
        .expect("batch result");

        assert!(result.all.is_empty());
        assert_eq!(result.completion_reason, BatchCompletionReason::AllCompleted);
    }
}
