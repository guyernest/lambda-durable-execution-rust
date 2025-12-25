use super::super::*;

#[allow(clippy::too_many_arguments)]
#[allow(clippy::type_complexity)]
pub(super) async fn run_map_execution<TIn, TOut, F, Fut>(
    inner: Arc<DurableContextImpl>,
    name: Option<&str>,
    items: Vec<TIn>,
    map_fn: Arc<F>,
    cfg: MapConfig<TIn, TOut>,
    item_namer: Option<Arc<dyn Fn(&TIn, usize) -> String + Send + Sync>>,
    item_serdes: Option<Arc<dyn Serdes<TOut>>>,
    batch_serdes: Option<Arc<dyn Serdes<BatchResult<TOut>>>>,
    map_step_id: String,
    map_hashed_id: String,
) -> DurableResult<BatchResult<TOut>>
where
    TIn: Serialize + DeserializeOwned + Send + 'static,
    TOut: Serialize + DeserializeOwned + Send + Sync + 'static,
    F: Fn(TIn, DurableContextHandle, usize) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = DurableResult<TOut>> + Send + 'static,
{
    let items_len = items.len();

    let should_continue = |failure_count: usize| -> bool {
        should_continue_batch(failure_count, items_len, &cfg.completion_config)
    };

    let compute_completion_reason =
        |failure_count: usize, success_count: usize, completed_count: usize| {
            compute_batch_completion_reason(
                failure_count,
                success_count,
                completed_count,
                items_len,
                &cfg.completion_config,
            )
        };

    let mut successes: Vec<(usize, TOut)> = Vec::new();
    let mut failures: Vec<(usize, DurableError)> = Vec::new();
    let mut started_indices: HashSet<usize> = HashSet::new();
    let mut success_count = 0usize;
    let mut failure_count = 0usize;
    let mut completed_count = 0usize;

    if items_len == 0 {
        let completion_reason = compute_completion_reason(0, 0, 0);
        let batch_result = BatchResult {
            all: Vec::new(),
            completion_reason,
        };

        let payload = if let Some(batch_serdes) = batch_serdes.clone() {
            safe_serialize_required_with_serdes(
                batch_serdes,
                &batch_result,
                &map_hashed_id,
                name,
                &inner.execution_ctx,
            )
            .await
        } else {
            safe_serialize(
                None,
                Some(&map_summary_payload(0, 0, 0, completion_reason)),
                &map_hashed_id,
                name,
                &inner.execution_ctx,
            )
            .await
            .expect("summary payload must be present")
        };

        let succeed_update = OperationUpdate::builder()
            .id(&map_hashed_id)
            .operation_type(OperationType::Context)
            .sub_type("Map")
            .action(OperationAction::Succeed)
            .payload(payload)
            .build()
            .map_err(|e| {
                DurableError::Internal(format!("Failed to build map completion update: {e}"))
            })?;
        inner
            .execution_ctx
            .checkpoint_manager
            .checkpoint(map_step_id, succeed_update)
            .await?;

        return Ok(batch_result);
    }

    let max_concurrency = cfg
        .max_concurrency
        .unwrap_or_else(|| items_len.max(1))
        .max(1);
    let min_successful = cfg.completion_config.min_successful;

    let mut join_set = tokio::task::JoinSet::new();
    let mut items_iter = items.into_iter().enumerate();

    loop {
        while join_set.len() < max_concurrency && should_continue(failure_count) {
            if let Some(min) = min_successful {
                if success_count >= min {
                    break;
                }
            }

            let Some((index, item)) = items_iter.next() else {
                break;
            };

            let item_name = if let Some(ref namer) = item_namer {
                namer(&item, index)
            } else {
                format!("{}-item-{}", name.unwrap_or("map"), index)
            };

            let child_step_id = inner.execution_ctx.next_operation_id(Some(&item_name));
            let child_hashed_id = DurableContextImpl::hash_id(&child_step_id);
            started_indices.insert(index);

            let child_cfg = ChildContextConfig::<TOut> {
                sub_type: Some("MapIteration".to_string()),
                serdes: item_serdes.clone(),
                ..Default::default()
            };

            let inner = Arc::clone(&inner);
            let map_fn = Arc::clone(&map_fn);
            join_set.spawn(async move {
                let res = inner
                    .run_in_child_context_with_ids(
                        child_step_id.clone(),
                        child_hashed_id,
                        Some(&item_name),
                        move |child_ctx| map_fn(item, child_ctx, index),
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

        let is_complete = completed_count == items_len
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
                    &map_hashed_id,
                    name,
                    &inner.execution_ctx,
                )
                .await
            } else {
                safe_serialize(
                    None,
                    Some(&map_summary_payload(
                        total_count,
                        success_count,
                        failure_count,
                        completion_reason,
                    )),
                    &map_hashed_id,
                    name,
                    &inner.execution_ctx,
                )
                .await
                .expect("summary payload must be present")
            };

            let succeed_update = OperationUpdate::builder()
                .id(&map_hashed_id)
                .operation_type(OperationType::Context)
                .sub_type("Map")
                .action(OperationAction::Succeed)
                .payload(payload)
                .build()
                .map_err(|e| {
                    DurableError::Internal(format!("Failed to build map completion update: {e}"))
                })?;
            inner
                .execution_ctx
                .checkpoint_manager
                .checkpoint(map_step_id, succeed_update)
                .await?;

            return Ok(batch_result);
        }
    }

    unreachable!("map execution loop should return a batch result");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::BoxError;
    use crate::mock::{MockCheckpointConfig, MockLambdaService};
    use crate::types::{
        BatchCompletionReason, CompletionConfig, DurableExecutionInvocationInput, ExecutionDetails,
        InitialExecutionState, Operation, OperationStatus, OperationType, SerdesContext,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Arc;

    struct BatchSerdes;

    #[async_trait]
    impl Serdes<BatchResult<u32>> for BatchSerdes {
        async fn serialize(
            &self,
            _value: Option<&BatchResult<u32>>,
            _context: SerdesContext,
        ) -> Result<Option<String>, BoxError> {
            Ok(Some("payload".to_string()))
        }

        async fn deserialize(
            &self,
            _data: Option<&str>,
            _context: SerdesContext,
        ) -> Result<Option<BatchResult<u32>>, BoxError> {
            Ok(None)
        }
    }

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
    async fn test_run_map_execution_empty_items() {
        let (inner, lambda_service) = make_execution_context().await;
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());

        let map_step_id = "map_0".to_string();
        let map_hashed_id = CheckpointManager::hash_id(&map_step_id);
        let cfg = MapConfig::<u32, u32>::new();
        let map_fn = Arc::new(
            |_item: u32, _ctx: DurableContextHandle, _idx: usize| async move {
                Ok::<u32, DurableError>(0)
            },
        );

        let result = run_map_execution(
            inner,
            Some("map"),
            Vec::new(),
            map_fn,
            cfg,
            None,
            None,
            None,
            map_step_id,
            map_hashed_id,
        )
        .await
        .expect("map should succeed");

        assert!(result.all.is_empty());
        assert_eq!(
            result.completion_reason,
            BatchCompletionReason::AllCompleted
        );
    }

    #[tokio::test]
    async fn test_run_map_execution_empty_items_with_batch_serdes() {
        let (inner, lambda_service) = make_execution_context().await;
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());

        let map_step_id = "map_0".to_string();
        let map_hashed_id = CheckpointManager::hash_id(&map_step_id);
        let cfg = MapConfig::<u32, u32>::new().with_serdes(Arc::new(BatchSerdes));
        let map_fn = Arc::new(
            |_item: u32, _ctx: DurableContextHandle, _idx: usize| async move {
                Ok::<u32, DurableError>(0)
            },
        );

        let result = run_map_execution(
            inner,
            Some("map"),
            Vec::new(),
            map_fn,
            cfg,
            None,
            None,
            Some(Arc::new(BatchSerdes)),
            map_step_id,
            map_hashed_id,
        )
        .await
        .expect("map should succeed");

        assert!(result.all.is_empty());
        assert_eq!(
            result.completion_reason,
            BatchCompletionReason::AllCompleted
        );
    }

    #[tokio::test]
    async fn test_run_map_execution_mixed_results() {
        let (inner, lambda_service) = make_execution_context().await;
        for _ in 0..8 {
            lambda_service.expect_checkpoint(MockCheckpointConfig::default());
        }

        let map_step_id = "map_0".to_string();
        let map_hashed_id = CheckpointManager::hash_id(&map_step_id);
        let cfg = MapConfig::new()
            .with_max_concurrency(4)
            .with_completion_config(CompletionConfig::new().with_tolerated_failures(1));

        let map_fn = Arc::new(
            |item: u32, _ctx: DurableContextHandle, idx: usize| async move {
                if idx == 0 {
                    Ok::<u32, DurableError>(item + 1)
                } else {
                    Err(DurableError::Internal("boom".to_string()))
                }
            },
        );

        let result = run_map_execution(
            inner,
            Some("map"),
            vec![1u32, 2u32],
            map_fn,
            cfg,
            None,
            None,
            None,
            map_step_id,
            map_hashed_id,
        )
        .await
        .expect("map should succeed");

        assert_eq!(result.success_count(), 1);
        assert_eq!(result.failure_count(), 1);
        assert_eq!(
            result.completion_reason,
            BatchCompletionReason::AllCompleted
        );
    }

    #[tokio::test]
    async fn test_run_map_execution_uses_batch_serdes() {
        let (inner, lambda_service) = make_execution_context().await;
        for _ in 0..4 {
            lambda_service.expect_checkpoint(MockCheckpointConfig::default());
        }

        let map_step_id = "map_0".to_string();
        let map_hashed_id = CheckpointManager::hash_id(&map_step_id);
        let cfg = MapConfig::new().with_serdes(Arc::new(BatchSerdes));

        let map_fn = Arc::new(
            |item: u32, _ctx: DurableContextHandle, _idx: usize| async move {
                Ok::<u32, DurableError>(item + 1)
            },
        );

        let result = run_map_execution(
            inner,
            Some("map"),
            vec![1u32],
            map_fn,
            cfg,
            None,
            None,
            Some(Arc::new(BatchSerdes)),
            map_step_id,
            map_hashed_id,
        )
        .await
        .expect("map should succeed");

        assert_eq!(result.success_count(), 1);
    }
}
