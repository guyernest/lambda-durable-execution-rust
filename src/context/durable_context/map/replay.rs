use super::super::*;

#[derive(Debug)]
pub(super) enum MapReplayDecision<TOut> {
    Return(BatchResult<TOut>),
    Reconstruct { total_count: usize },
    Continue,
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::type_complexity)]
pub(super) async fn evaluate_map_replay<TIn, TOut>(
    name: Option<&str>,
    items: &[TIn],
    item_namer: &Option<Arc<dyn Fn(&TIn, usize) -> String + Send + Sync>>,
    batch_serdes: &Option<Arc<dyn Serdes<BatchResult<TOut>>>>,
    execution_ctx: &ExecutionContext,
    map_hashed_id: &str,
    _completion_config: &crate::types::CompletionConfig,
) -> DurableResult<MapReplayDecision<TOut>>
where
    TIn: Serialize + DeserializeOwned + Send + 'static,
    TOut: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    let Some(op) = execution_ctx.get_step_data(map_hashed_id).await else {
        return Ok(MapReplayDecision::Continue);
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
                name: name.unwrap_or("map").to_string(),
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

                        obj.get("type").and_then(|t| t.as_str()) == Some("MapResult")
                            || obj.get("totalCount").is_some()
                    })
                    .unwrap_or(false);

                if !is_summary {
                    if let Some(batch_serdes) = batch_serdes.clone() {
                        let batch: BatchResult<TOut> = safe_deserialize_required_with_serdes(
                            batch_serdes,
                            payload,
                            map_hashed_id,
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

                        if target_total_count > items.len() {
                            return Err(DurableError::ReplayValidationFailed {
                                expected: format!("map totalCount <= {}", items.len()),
                                actual: target_total_count.to_string(),
                            });
                        }

                        // Consume child context operation IDs to keep the parent context counter in sync.
                        for (index, item) in items.iter().enumerate().take(target_total_count) {
                            let item_name = if let Some(ref namer) = item_namer {
                                namer(item, index)
                            } else {
                                format!("{}-item-{}", name.unwrap_or("map"), index)
                            };
                            let _ = execution_ctx.next_operation_id(Some(&item_name));
                        }

                        return Ok(MapReplayDecision::Return(batch));
                    }
                } else {
                    let total_count = parsed
                        .as_ref()
                        .and_then(|v| v.get("totalCount").and_then(|tc| tc.as_u64()))
                        .map(|tc| tc as usize);

                    if let Some(total_count) = total_count {
                        if total_count > items.len() {
                            return Err(DurableError::ReplayValidationFailed {
                                expected: format!("map totalCount <= {}", items.len()),
                                actual: total_count.to_string(),
                            });
                        }

                        return Ok(MapReplayDecision::Reconstruct { total_count });
                    }
                }
            }

            Ok(MapReplayDecision::Continue)
        }
        _ => {
            // Incomplete top-level map during replay; continue execution.
            execution_ctx.set_mode(ExecutionMode::Execution).await;
            Ok(MapReplayDecision::Continue)
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::type_complexity)]
pub(super) async fn reconstruct_map_from_children<TIn, TOut, F, Fut>(
    inner: Arc<DurableContextImpl>,
    name: Option<&str>,
    items: Vec<TIn>,
    map_fn: Arc<F>,
    item_namer: Option<Arc<dyn Fn(&TIn, usize) -> String + Send + Sync>>,
    item_serdes: Option<Arc<dyn Serdes<TOut>>>,
    map_hashed_id: &str,
    total_count: usize,
    completion_config: &crate::types::CompletionConfig,
) -> DurableResult<BatchResult<TOut>>
where
    TIn: Serialize + DeserializeOwned + Send + 'static,
    TOut: Serialize + DeserializeOwned + Send + Sync + 'static,
    F: Fn(TIn, DurableContextHandle, usize) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = DurableResult<TOut>> + Send + 'static,
{
    if total_count > items.len() {
        return Err(DurableError::ReplayValidationFailed {
            expected: format!("map totalCount <= {}", items.len()),
            actual: total_count.to_string(),
        });
    }

    let items_len = items.len();
    let item_serdes = item_serdes;

    let map_parent_execution_ctx = inner
        .execution_ctx
        .with_parent_id(map_hashed_id.to_string());
    let map_parent_impl = Arc::new(DurableContextImpl::new(map_parent_execution_ctx));

    let mut successes = Vec::new();
    let mut failures = Vec::new();
    let mut started = Vec::new();

    for (index, item) in items.into_iter().enumerate().take(total_count) {
        let item_name = if let Some(ref namer) = item_namer {
            namer(&item, index)
        } else {
            format!("{}-item-{}", name.unwrap_or("map"), index)
        };

        let child_step_id = inner.execution_ctx.next_operation_id(Some(&item_name));
        let child_hashed_id = DurableContextImpl::hash_id(&child_step_id);
        let Some(child_op) = inner.execution_ctx.get_step_data(&child_hashed_id).await else {
            return Err(DurableError::ReplayValidationFailed {
                expected: format!("map child context present for index {index}"),
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
                    let map_fn = Arc::clone(&map_fn);
                    let res = map_parent_impl
                        .run_in_child_context_with_ids(
                            child_step_id.clone(),
                            child_hashed_id,
                            Some(&item_name),
                            move |child_ctx| map_fn(item, child_ctx, index),
                            Some(ChildContextConfig::<TOut> {
                                sub_type: Some("MapIteration".to_string()),
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
                    let val: TOut = safe_deserialize(
                        item_serdes.clone(),
                        payload,
                        &child_hashed_id,
                        Some(&item_name),
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
        items_len,
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
    use crate::mock::MockLambdaService;
    use crate::types::{
        BatchCompletionReason, BatchItem, BatchItemStatus, CompletionConfig,
        DurableExecutionInvocationInput, SerdesContext,
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
    async fn test_maybe_replay_map_returns_none_without_operation() {
        let arn = "arn:test:durable";
        let execution_ctx = make_execution_context(arn, vec![]).await;

        let map_step_id = "map_0".to_string();
        let map_hashed_id = CheckpointManager::hash_id(&map_step_id);
        let result = evaluate_map_replay::<u32, u32>(
            Some("map"),
            &[1u32],
            &None,
            &None,
            &execution_ctx,
            &map_hashed_id,
            &CompletionConfig::new(),
        )
        .await
        .unwrap();

        assert!(matches!(result, MapReplayDecision::Continue));
    }

    #[tokio::test]
    async fn test_maybe_replay_map_started_sets_execution_mode() {
        let arn = "arn:test:durable";
        let map_step_id = "map_0".to_string();
        let map_hashed_id = CheckpointManager::hash_id(&map_step_id);
        let map_op = json!({
            "Id": map_hashed_id,
            "Type": "CONTEXT",
            "SubType": "Map",
            "Status": "STARTED",
        });

        let execution_ctx = make_execution_context(arn, vec![map_op]).await;
        assert_eq!(execution_ctx.get_mode().await, ExecutionMode::Replay);

        let result = evaluate_map_replay::<u32, u32>(
            Some("map"),
            &[1u32],
            &None,
            &None,
            &execution_ctx,
            &map_hashed_id,
            &CompletionConfig::new(),
        )
        .await
        .unwrap();

        assert!(matches!(result, MapReplayDecision::Continue));
        assert_eq!(execution_ctx.get_mode().await, ExecutionMode::Execution);
    }

    #[tokio::test]
    async fn test_maybe_replay_map_batch_serdes_returns_batch() {
        let arn = "arn:test:durable";
        let map_step_id = "map_0".to_string();
        let map_hashed_id = CheckpointManager::hash_id(&map_step_id);
        let map_op = json!({
            "Id": map_hashed_id,
            "Type": "CONTEXT",
            "SubType": "Map",
            "Status": "SUCCEEDED",
            "ContextDetails": { "Result": "payload" },
        });

        let execution_ctx = make_execution_context(arn, vec![map_op]).await;
        let batch_serdes = StaticBatchSerdes::<u32> {
            items: vec![
                (0, BatchItemStatus::Succeeded, Some(10)),
                (1, BatchItemStatus::Failed, None),
            ],
            completion_reason: BatchCompletionReason::FailureToleranceExceeded,
        };

        let result = evaluate_map_replay::<u32, u32>(
            Some("map"),
            &[1u32, 2u32],
            &None,
            &Some(Arc::new(batch_serdes)),
            &execution_ctx,
            &map_hashed_id,
            &CompletionConfig::new(),
        )
        .await
        .unwrap();

        let MapReplayDecision::Return(result) = result else {
            panic!("expected batch result from batch serdes");
        };

        assert_eq!(
            result.completion_reason,
            BatchCompletionReason::FailureToleranceExceeded
        );
        assert_eq!(result.all.len(), 2);
    }

    #[tokio::test]
    async fn test_maybe_replay_map_total_count_exceeds_items_returns_error() {
        let arn = "arn:test:durable";
        let map_step_id = "map_0".to_string();
        let map_hashed_id = CheckpointManager::hash_id(&map_step_id);
        let payload = json!({ "totalCount": 2 }).to_string();
        let map_op = json!({
            "Id": map_hashed_id,
            "Type": "CONTEXT",
            "SubType": "Map",
            "Status": "SUCCEEDED",
            "ContextDetails": { "Result": payload },
        });

        let execution_ctx = make_execution_context(arn, vec![map_op]).await;
        let err = evaluate_map_replay::<u32, u32>(
            Some("map"),
            &[1u32],
            &None,
            &None,
            &execution_ctx,
            &map_hashed_id,
            &CompletionConfig::new(),
        )
        .await
        .expect_err("expected replay validation failure");

        let DurableError::ReplayValidationFailed { .. } = err else {
            panic!("expected ReplayValidationFailed, got {err:?}");
        };
    }
}
