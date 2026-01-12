use super::super::*;

#[derive(Debug)]
pub(super) enum ParallelReplayDecision<T> {
    Return(BatchResult<T>),
    Reconstruct { total_count: usize },
    Continue,
}

pub(super) async fn evaluate_parallel_replay<T, F>(
    name: Option<&str>,
    branches: &[NamedParallelBranch<F>],
    batch_serdes: &Option<Arc<dyn Serdes<BatchResult<T>>>>,
    execution_ctx: &ExecutionContext,
    par_hashed_id: &str,
    _completion_config: &crate::types::CompletionConfig,
) -> DurableResult<ParallelReplayDecision<T>>
where
    T: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    let Some(op) = execution_ctx.get_step_data(par_hashed_id).await else {
        return Ok(ParallelReplayDecision::Continue);
    };

    match op.status {
        OperationStatus::Failed => {
            let msg = op
                .context_details
                .as_ref()
                .and_then(|d| d.error.as_ref())
                .map(|e| e.error_message.clone())
                .unwrap_or_else(|| "Batch operation failed".to_string());
            Err(DurableError::BatchOperationFailed {
                name: name.unwrap_or("parallel").to_string(),
                message: msg,
                successful_count: 0,
                failed_count: 0,
            })
        }
        OperationStatus::Succeeded => {
            if let Some(payload) = op.context_details.as_ref().and_then(|d| d.result.as_ref()) {
                let parsed = serde_json::from_str::<serde_json::Value>(payload).ok();
                let is_summary = parsed
                    .as_ref()
                    .and_then(|v| v.as_object())
                    .map(|obj| {
                        let kind_is_batch =
                            obj.get("kind").and_then(|k| k.as_str()) == Some("BatchResult");
                        if kind_is_batch {
                            return false;
                        }

                        obj.get("type").and_then(|t| t.as_str()) == Some("ParallelResult")
                            || obj.get("totalCount").is_some()
                    })
                    .unwrap_or(false);

                if !is_summary {
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
                        for (index, branch) in branches.iter().enumerate().take(target_total_count)
                        {
                            let base = name.unwrap_or("parallel");
                            let branch_name = branch
                                .name
                                .clone()
                                .unwrap_or_else(|| format!("{base}-branch-{index}"));
                            let _ = execution_ctx.next_operation_id(Some(&branch_name));
                        }

                        return Ok(ParallelReplayDecision::Return(batch));
                    }
                } else {
                    let total_count = parsed
                        .as_ref()
                        .and_then(|v| v.get("totalCount").and_then(|tc| tc.as_u64()))
                        .map(|tc| tc as usize);

                    if let Some(total_count) = total_count {
                        if total_count > branches.len() {
                            return Err(DurableError::ReplayValidationFailed {
                                expected: format!("parallel totalCount <= {}", branches.len()),
                                actual: total_count.to_string(),
                            });
                        }

                        return Ok(ParallelReplayDecision::Reconstruct { total_count });
                    }
                }
            }

            Ok(ParallelReplayDecision::Continue)
        }
        _ => {
            execution_ctx.set_mode(ExecutionMode::Execution).await;
            Ok(ParallelReplayDecision::Continue)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn reconstruct_parallel_from_children<T, F, Fut>(
    inner: Arc<DurableContextImpl>,
    name: Option<&str>,
    branches: Vec<NamedParallelBranch<F>>,
    item_serdes: Option<Arc<dyn Serdes<T>>>,
    par_hashed_id: &str,
    total_count: usize,
    completion_config: &crate::types::CompletionConfig,
) -> DurableResult<BatchResult<T>>
where
    T: Serialize + DeserializeOwned + Send + Sync + 'static,
    F: Fn(DurableContextHandle) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = DurableResult<T>> + Send + 'static,
{
    if total_count > branches.len() {
        return Err(DurableError::ReplayValidationFailed {
            expected: format!("parallel totalCount <= {}", branches.len()),
            actual: total_count.to_string(),
        });
    }

    let branches_len = branches.len();
    let item_serdes = item_serdes;

    let par_parent_execution_ctx = inner
        .execution_ctx
        .with_parent_id(par_hashed_id.to_string());
    let par_parent_impl = Arc::new(DurableContextImpl::new(par_parent_execution_ctx));

    let mut successes = Vec::new();
    let mut failures = Vec::new();
    let mut started = Vec::new();

    for (index, branch) in branches.into_iter().enumerate().take(total_count) {
        let base = name.unwrap_or("parallel");
        let branch_name = branch
            .name
            .clone()
            .unwrap_or_else(|| format!("{base}-branch-{index}"));

        let child_step_id = inner.execution_ctx.next_operation_id(Some(&branch_name));
        let child_hashed_id = DurableContextImpl::hash_id(&child_step_id);
        let Some(child_op) = inner.execution_ctx.get_step_data(&child_hashed_id).await else {
            return Err(DurableError::ReplayValidationFailed {
                expected: format!("parallel child context present for index {index}"),
                actual: "missing".to_string(),
            });
        };

        match child_op.status {
            OperationStatus::Succeeded => {
                if child_op
                    .context_details
                    .as_ref()
                    .and_then(|d| d.replay_children)
                    == Some(true)
                {
                    let res = par_parent_impl
                        .run_in_child_context_with_ids(
                            child_step_id.clone(),
                            child_hashed_id,
                            Some(&branch_name),
                            move |child_ctx| (branch.func)(child_ctx),
                            Some(ChildContextConfig::<T> {
                                sub_type: Some("ParallelBranch".to_string()),
                                serdes: item_serdes.clone(),
                                ..Default::default()
                            }),
                        )
                        .await;

                    match res {
                        Ok(v) => successes.push((index, v)),
                        Err(e) => failures.push((index, e)),
                    }
                } else {
                    let payload = child_op
                        .context_details
                        .as_ref()
                        .and_then(|d| d.result.as_deref());
                    let val: T = safe_deserialize(
                        item_serdes.clone(),
                        payload,
                        &child_hashed_id,
                        Some(&branch_name),
                        &inner.execution_ctx,
                    )
                    .await
                    .ok_or_else(|| {
                        DurableError::Internal("Missing child context output in replay".to_string())
                    })?;
                    successes.push((index, val));
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

    let completed_count = successes.len() + failures.len();
    let completion_reason = compute_batch_completion_reason(
        failures.len(),
        successes.len(),
        completed_count,
        branches_len,
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

    Ok(BatchResult {
        all,
        completion_reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::DurableError;
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

        let result = evaluate_parallel_replay::<u32, _>(
            Some("parallel"),
            &branches,
            &None,
            &execution_ctx,
            &hashed_id,
            &CompletionConfig::new(),
        )
        .await
        .unwrap();

        assert!(matches!(result, ParallelReplayDecision::Continue));
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

        let result = evaluate_parallel_replay::<u32, _>(
            Some("parallel"),
            &branches,
            &None,
            &execution_ctx,
            &hashed_id,
            &CompletionConfig::new(),
        )
        .await
        .unwrap();

        assert!(matches!(result, ParallelReplayDecision::Continue));
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

        let result = evaluate_parallel_replay::<u32, _>(
            Some("parallel"),
            &branches,
            &Some(Arc::new(batch_serdes)),
            &execution_ctx,
            &hashed_id,
            &CompletionConfig::new(),
        )
        .await
        .unwrap();

        let ParallelReplayDecision::Return(result) = result else {
            panic!("expected batch result from batch serdes");
        };

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

        let err = evaluate_parallel_replay::<u32, _>(
            Some("parallel"),
            &branches,
            &None,
            &execution_ctx,
            &hashed_id,
            &CompletionConfig::new(),
        )
        .await
        .expect_err("expected replay validation failure");

        let DurableError::ReplayValidationFailed { .. } = err else {
            panic!("expected ReplayValidationFailed, got {err:?}");
        };
    }

    #[tokio::test]
    async fn test_maybe_replay_parallel_failed_returns_error() {
        let arn = "arn:test:durable";
        let step_id = "parallel_0".to_string();
        let hashed_id = CheckpointManager::hash_id(&step_id);
        let op = json!({
            "Id": hashed_id,
            "Type": "CONTEXT",
            "SubType": "Parallel",
            "Status": "FAILED",
            "ContextDetails": { "Error": { "ErrorMessage": "boom" } },
        });

        let execution_ctx = make_execution_context(arn, vec![op]).await;
        fn branch_fn(
            _ctx: DurableContextHandle,
        ) -> BoxFuture<'static, crate::error::DurableResult<u32>> {
            Box::pin(async move { Ok(1) })
        }
        let branches: Vec<NamedParallelBranch<_>> = vec![NamedParallelBranch::new(branch_fn)];

        let err = evaluate_parallel_replay::<u32, _>(
            Some("parallel"),
            &branches,
            &None,
            &execution_ctx,
            &hashed_id,
            &CompletionConfig::new(),
        )
        .await
        .expect_err("failed parallel should return error");

        match err {
            DurableError::BatchOperationFailed { name, message, .. } => {
                assert_eq!(name, "parallel");
                assert!(message.contains("boom"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_maybe_replay_parallel_reconstructs_children_from_total_count() {
        let arn = "arn:test:durable";
        let step_id = "parallel_0".to_string();
        let hashed_id = CheckpointManager::hash_id(&step_id);
        let payload = json!({ "totalCount": 3 }).to_string();
        let op = json!({
            "Id": hashed_id,
            "Type": "CONTEXT",
            "SubType": "Parallel",
            "Status": "SUCCEEDED",
            "ContextDetails": { "Result": payload },
        });

        let base = "parallel";
        // Consume the parent operation ID ("parallel_0") so branch IDs match the real
        // `parallel_named` path.
        let input_json = create_replay_input(arn, &json!({}), vec![op.clone()]);
        let input: DurableExecutionInvocationInput =
            serde_json::from_value(input_json).expect("valid invocation input");
        let execution_ctx =
            ExecutionContext::new(&input, Arc::new(MockLambdaService::new()), None, true)
                .await
                .expect("execution context should initialize");
        let _ = execution_ctx.next_operation_id(Some(base));

        let branch_names = [
            format!("{base}-branch-0"),
            format!("{base}-branch-1"),
            format!("{base}-branch-2"),
        ];

        let child_success_payload = serde_json::to_string(&1u32).unwrap();
        let child_ops = vec![
            json!({
                "Id": CheckpointManager::hash_id(&format!("{}_1", branch_names[0])),
                "Type": "CONTEXT",
                "SubType": "ParallelBranch",
                "Status": "SUCCEEDED",
                "ContextDetails": { "Result": child_success_payload },
            }),
            json!({
                "Id": CheckpointManager::hash_id(&format!("{}_2", branch_names[1])),
                "Type": "CONTEXT",
                "SubType": "ParallelBranch",
                "Status": "FAILED",
                "ContextDetails": { "Error": { "ErrorMessage": "child boom" } },
            }),
            json!({
                "Id": CheckpointManager::hash_id(&format!("{}_3", branch_names[2])),
                "Type": "CONTEXT",
                "SubType": "ParallelBranch",
                "Status": "STARTED",
            }),
        ];

        {
            let mut step_data = execution_ctx.step_data.lock().await;
            for child_op in child_ops {
                let op: crate::types::Operation =
                    serde_json::from_value(child_op).expect("valid operation");
                step_data.insert(op.id.clone(), op);
            }
        }

        fn branch_fn(
            _ctx: DurableContextHandle,
        ) -> BoxFuture<'static, crate::error::DurableResult<u32>> {
            Box::pin(async move { Ok(1) })
        }
        let mut branches: Vec<NamedParallelBranch<_>> = vec![
            NamedParallelBranch::new(branch_fn),
            NamedParallelBranch::new(branch_fn),
            NamedParallelBranch::new(branch_fn),
        ];

        let decision = evaluate_parallel_replay::<u32, _>(
            Some(base),
            &branches,
            &None,
            &execution_ctx,
            &hashed_id,
            &CompletionConfig::new(),
        )
        .await
        .unwrap();

        let ParallelReplayDecision::Reconstruct { total_count } = decision else {
            panic!("expected reconstruct decision");
        };

        let inner = Arc::new(DurableContextImpl::new(execution_ctx));
        let result = reconstruct_parallel_from_children(
            inner,
            Some(base),
            std::mem::take(&mut branches),
            None,
            &hashed_id,
            total_count,
            &CompletionConfig::new(),
        )
        .await
        .expect("batch result");

        assert_eq!(result.success_count(), 1);
        assert_eq!(result.failure_count(), 1);
        assert_eq!(result.started_count(), 1);
        let failed = result.failed();
        let err = failed
            .first()
            .and_then(|item| item.error.as_ref())
            .expect("failed error");
        assert!(err.to_string().contains("child boom"));
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

        let result = evaluate_parallel_replay::<u32, _>(
            Some("parallel"),
            &branches,
            &None,
            &execution_ctx,
            &hashed_id,
            &CompletionConfig::new(),
        )
        .await
        .unwrap();

        assert!(matches!(result, ParallelReplayDecision::Continue));
    }

    #[tokio::test]
    async fn test_maybe_replay_parallel_missing_child_operation_returns_error() {
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

        let decision = evaluate_parallel_replay::<u32, _>(
            Some("parallel"),
            &branches,
            &None,
            &execution_ctx,
            &hashed_id,
            &CompletionConfig::new(),
        )
        .await
        .unwrap();

        let ParallelReplayDecision::Reconstruct { total_count } = decision else {
            panic!("expected reconstruct decision");
        };

        let inner = Arc::new(DurableContextImpl::new(execution_ctx));
        let err = reconstruct_parallel_from_children(
            inner,
            Some("parallel"),
            branches,
            None,
            &hashed_id,
            total_count,
            &CompletionConfig::new(),
        )
        .await
        .expect_err("expected replay validation failure");

        let DurableError::ReplayValidationFailed { .. } = err else {
            panic!("expected ReplayValidationFailed, got {err:?}");
        };
    }
}
