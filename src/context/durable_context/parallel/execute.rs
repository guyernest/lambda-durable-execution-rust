use super::super::*;

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_parallel_execution<T, F, Fut>(
    inner: Arc<DurableContextImpl>,
    name: Option<&str>,
    branches: Vec<NamedParallelBranch<F>>,
    cfg: ParallelConfig<T>,
    item_serdes: Option<Arc<dyn Serdes<T>>>,
    batch_serdes: Option<Arc<dyn Serdes<BatchResult<T>>>>,
    par_step_id: String,
    par_hashed_id: String,
) -> DurableResult<BatchResult<T>>
where
    T: Serialize + DeserializeOwned + Send + Sync + 'static,
    F: Fn(DurableContextHandle) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = DurableResult<T>> + Send + 'static,
{
    let branches_len = branches.len();

    let should_continue = |failure_count: usize| -> bool {
        should_continue_batch(failure_count, branches_len, &cfg.completion_config)
    };

    let compute_completion_reason =
        |failure_count: usize, success_count: usize, completed_count: usize| {
            compute_batch_completion_reason(
                failure_count,
                success_count,
                completed_count,
                branches_len,
                &cfg.completion_config,
            )
        };

    // Execution mode: run branches with deterministic concurrency and early completion.
    let mut successes: Vec<(usize, T)> = Vec::new();
    let mut failures: Vec<(usize, DurableError)> = Vec::new();
    let mut started_indices: HashSet<usize> = HashSet::new();
    let mut success_count = 0usize;
    let mut failure_count = 0usize;
    let mut completed_count = 0usize;

    let max_concurrency = cfg
        .max_concurrency
        .unwrap_or_else(|| branches_len.max(1))
        .max(1);
    let min_successful = cfg.completion_config.min_successful;

    let mut join_set = tokio::task::JoinSet::new();
    let mut branches_iter = branches.into_iter().enumerate();

    loop {
        while join_set.len() < max_concurrency && should_continue(failure_count) {
            if let Some(min) = min_successful {
                if success_count >= min {
                    break;
                }
            }

            let Some((index, branch)) = branches_iter.next() else {
                break;
            };

            let base = name.unwrap_or("parallel");
            let branch_name = branch
                .name
                .unwrap_or_else(|| format!("{base}-branch-{index}"));

            let child_step_id = inner.execution_ctx.next_operation_id(Some(&branch_name));
            let child_hashed_id = DurableContextImpl::hash_id(&child_step_id);
            started_indices.insert(index);

            let child_cfg = ChildContextConfig::<T> {
                sub_type: Some("ParallelBranch".to_string()),
                serdes: item_serdes.clone(),
                ..Default::default()
            };

            let inner = Arc::clone(&inner);
            join_set.spawn(async move {
                let res = inner
                    .run_in_child_context_with_ids(
                        child_step_id.clone(),
                        child_hashed_id,
                        Some(&branch_name),
                        move |child_ctx| (branch.func)(child_ctx),
                        Some(child_cfg),
                    )
                    .await;
                (index, res)
            });
        }

        let Some(joined) = join_set.join_next().await else {
            break;
        };
        let (index, res) =
            joined.map_err(|e| DurableError::Internal(format!("Child task join error: {e}")))?;

        started_indices.remove(&index);
        completed_count += 1;

        match res {
            Ok(v) => {
                successes.push((index, v));
                success_count += 1;
            }
            Err(e) => {
                failures.push((index, e));
                failure_count += 1;
            }
        }

        let is_complete = completed_count == branches_len
            || min_successful
                .map(|min| success_count >= min)
                .unwrap_or(false);

        if is_complete || !should_continue(failure_count) {
            if !started_indices.is_empty() {
                join_set.abort_all();
            }

            let completion_reason =
                compute_completion_reason(failure_count, success_count, completed_count);
            let started_count = started_indices.len();
            let total_count = completed_count + started_count;
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
            for i in started_indices {
                all.push(BatchItem {
                    index: i,
                    status: BatchItemStatus::Started,
                    result: None,
                    error: None,
                });
            }
            all.sort_by_key(|i| i.index);

            let batch_result = BatchResult {
                all,
                completion_reason,
            };

            let payload = if let Some(batch_serdes) = batch_serdes.clone() {
                safe_serialize_required_with_serdes(
                    batch_serdes,
                    &batch_result,
                    &par_hashed_id,
                    name,
                    &inner.execution_ctx,
                )
                .await
            } else {
                safe_serialize(
                    None,
                    Some(&parallel_summary_payload(
                        total_count,
                        success_count,
                        failure_count,
                        started_count,
                        completion_reason,
                    )),
                    &par_hashed_id,
                    name,
                    &inner.execution_ctx,
                )
                .await
                .expect("summary payload must be present")
            };

            let succeed_update = OperationUpdate::builder()
                .id(&par_hashed_id)
                .operation_type(OperationType::Context)
                .sub_type("Parallel")
                .action(OperationAction::Succeed)
                .payload(payload)
                .build()
                .map_err(|e| {
                    DurableError::Internal(format!(
                        "Failed to build parallel completion update: {e}"
                    ))
                })?;
            inner
                .execution_ctx
                .checkpoint_manager
                .checkpoint(par_step_id.clone(), succeed_update)
                .await?;

            return Ok(batch_result);
        }
    }

    let completion_reason =
        compute_completion_reason(failure_count, success_count, completed_count);
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
    all.sort_by_key(|i| i.index);

    let batch_result = BatchResult {
        all,
        completion_reason,
    };

    let payload = if let Some(batch_serdes) = batch_serdes {
        safe_serialize_required_with_serdes(
            batch_serdes,
            &batch_result,
            &par_hashed_id,
            name,
            &inner.execution_ctx,
        )
        .await
    } else {
        safe_serialize(
            None,
            Some(&parallel_summary_payload(
                completed_count,
                success_count,
                failure_count,
                0,
                completion_reason,
            )),
            &par_hashed_id,
            name,
            &inner.execution_ctx,
        )
        .await
        .expect("summary payload must be present")
    };

    let succeed_update = OperationUpdate::builder()
        .id(&par_hashed_id)
        .operation_type(OperationType::Context)
        .sub_type("Parallel")
        .action(OperationAction::Succeed)
        .payload(payload)
        .build()
        .map_err(|e| {
            DurableError::Internal(format!("Failed to build parallel completion update: {e}"))
        })?;
    inner
        .execution_ctx
        .checkpoint_manager
        .checkpoint(par_step_id, succeed_update)
        .await?;

    Ok(batch_result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::BoxFuture;
    use crate::error::{DurableError, DurableResult};
    use crate::mock::{MockCheckpointConfig, MockLambdaService};
    use crate::types::{
        BatchCompletionReason, CompletionConfig, DurableExecutionInvocationInput, ExecutionDetails,
        InitialExecutionState, NamedParallelBranch, Operation, OperationStatus, OperationType,
        ParallelConfig,
    };
    use serde_json::json;
    use std::sync::Arc;

    async fn make_execution_context() -> (Arc<DurableContextImpl>, Arc<MockLambdaService>) {
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
        (Arc::new(DurableContextImpl::new(exec_ctx)), lambda_service)
    }

    #[tokio::test]
    async fn test_run_parallel_execution_empty_branches() {
        let (inner, lambda_service) = make_execution_context().await;
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());

        let par_step_id = "parallel_0".to_string();
        let par_hashed_id = CheckpointManager::hash_id(&par_step_id);
        let cfg = ParallelConfig::<u32>::new();
        type BranchFn = fn(DurableContextHandle) -> BoxFuture<'static, DurableResult<u32>>;
        let branches: Vec<NamedParallelBranch<BranchFn>> = Vec::new();

        let result = run_parallel_execution(
            inner,
            Some("parallel"),
            branches,
            cfg,
            None,
            None,
            par_step_id,
            par_hashed_id,
        )
        .await
        .expect("parallel should succeed");

        assert!(result.all.is_empty());
        assert_eq!(
            result.completion_reason,
            BatchCompletionReason::AllCompleted
        );
    }

    #[tokio::test]
    async fn test_run_parallel_execution_mixed_results() {
        let (inner, lambda_service) = make_execution_context().await;
        for _ in 0..8 {
            lambda_service.expect_checkpoint(MockCheckpointConfig::default());
        }

        let par_step_id = "parallel_0".to_string();
        let par_hashed_id = CheckpointManager::hash_id(&par_step_id);
        let cfg = ParallelConfig::new()
            .with_max_concurrency(4)
            .with_completion_config(CompletionConfig::new().with_tolerated_failures(1));

        fn ok_branch(_ctx: DurableContextHandle) -> BoxFuture<'static, DurableResult<u32>> {
            Box::pin(async { Ok(1) })
        }

        fn fail_branch(_ctx: DurableContextHandle) -> BoxFuture<'static, DurableResult<u32>> {
            Box::pin(async { Err(DurableError::Internal("boom".to_string())) })
        }

        type BranchFn = fn(DurableContextHandle) -> BoxFuture<'static, DurableResult<u32>>;
        let ok = NamedParallelBranch::new(ok_branch as BranchFn);
        let fail = NamedParallelBranch::new(fail_branch as BranchFn);

        let result = run_parallel_execution(
            inner,
            Some("parallel"),
            vec![ok, fail],
            cfg,
            None,
            None,
            par_step_id,
            par_hashed_id,
        )
        .await
        .expect("parallel should succeed");

        assert_eq!(result.success_count(), 1);
        assert_eq!(result.failure_count(), 1);
        assert_eq!(
            result.completion_reason,
            BatchCompletionReason::AllCompleted
        );
    }
}
