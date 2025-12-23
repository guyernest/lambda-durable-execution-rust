use super::super::*;

pub(super) async fn maybe_replay_map<TIn, TOut>(
    name: Option<&str>,
    items: &[TIn],
    item_namer: &Option<Arc<dyn Fn(&TIn, usize) -> String + Send + Sync>>,
    batch_serdes: &Option<Arc<dyn Serdes<BatchResult<TOut>>>>,
    item_serdes: &Option<Arc<dyn Serdes<TOut>>>,
    execution_ctx: &ExecutionContext,
    map_hashed_id: &str,
    completion_config: &crate::types::CompletionConfig,
) -> DurableResult<Option<BatchResult<TOut>>>
where
    TIn: Serialize + DeserializeOwned + Send + 'static,
    TOut: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    let Some(op) = execution_ctx.get_step_data(map_hashed_id).await else {
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
                name: name.unwrap_or("map").to_string(),
                message: msg,
                successful_count: 0,
                failed_count: 0,
            });
        }
        OperationStatus::Succeeded => {
            if let Some(payload) = op.context_details.as_ref().and_then(|d| d.result.as_ref()) {
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

                for (index, item) in items.iter().enumerate() {
                    if seen_count >= target_total_count {
                        break;
                    }

                    let item_name = if let Some(ref namer) = item_namer {
                        namer(item, index)
                    } else {
                        format!("{}-item-{}", name.unwrap_or("map"), index)
                    };

                    let child_step_id = execution_ctx.next_operation_id(Some(&item_name));
                    let child_hashed_id = DurableContextImpl::hash_id(&child_step_id);

                    if let Some(child_op) = execution_ctx.get_step_data(&child_hashed_id).await {
                        seen_count += 1;

                        match child_op.status {
                            OperationStatus::Succeeded => {
                                if let Some(ref details) = child_op.context_details {
                                    if let Some(ref payload) = details.result {
                                        let val: TOut = safe_deserialize(
                                            item_serdes.clone(),
                                            Some(payload.as_str()),
                                            &child_hashed_id,
                                            Some(&item_name),
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
                    items.len(),
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
            // Incomplete top-level map during replay; continue execution.
            execution_ctx.set_mode(ExecutionMode::Execution).await;
        }
    }

    Ok(None)
}
