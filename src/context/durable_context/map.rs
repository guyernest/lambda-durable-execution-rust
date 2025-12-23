use super::*;

impl DurableContextHandle {
    /// Map over a list of items using durable child contexts.
    ///
    /// Each item is processed in its own child context, so durable operations
    /// inside the mapper are isolated and replay-safe.
    pub async fn map<TIn, TOut, F, Fut>(
        &self,
        name: Option<&str>,
        items: Vec<TIn>,
        map_fn: F,
        config: Option<MapConfig<TIn, TOut>>,
    ) -> DurableResult<BatchResult<TOut>>
    where
        TIn: Serialize + DeserializeOwned + Send + 'static,
        TOut: Serialize + DeserializeOwned + Send + Sync + 'static,
        F: Fn(TIn, DurableContextHandle, usize) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = DurableResult<TOut>> + Send + 'static,
    {
        let cfg = config.unwrap_or_default();
        let map_fn = Arc::new(map_fn);
        let items_len = items.len();
        validate_completion_config(&cfg.completion_config, items_len, name.unwrap_or("map"))?;
        let mode = self.inner.execution_ctx.get_mode().await;
        let item_serdes = cfg.item_serdes.clone();
        let item_namer = cfg.item_namer.clone();
        let batch_serdes = cfg.serdes.clone();

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

        // Start top-level MAP context for observability.
        let map_step_id = self.inner.execution_ctx.next_operation_id(name);
        let map_hashed_id = DurableContextImpl::hash_id(&map_step_id);
        if self
            .inner
            .execution_ctx
            .get_step_data(&map_hashed_id)
            .await
            .is_none()
        {
            let parent_id = self.inner.execution_ctx.get_parent_id().await;
            let mut builder = OperationUpdate::builder()
                .id(&map_hashed_id)
                .operation_type(OperationType::Context)
                .sub_type("Map")
                .action(OperationAction::Start);
            if let Some(pid) = parent_id {
                builder = builder.parent_id(pid);
            }
            if let Some(n) = name {
                builder = builder.name(n);
            }
            self.inner
                .execution_ctx
                .checkpoint_manager
                .checkpoint(
                    map_step_id.clone(),
                    builder.build().map_err(|e| {
                        DurableError::Internal(format!("Failed to build map START update: {e}"))
                    })?,
                )
                .await?;
        }

        // Replay handling: if the top-level map completed, reconstruct children and skip incomplete ones.
        if mode == ExecutionMode::Replay {
            if let Some(op) = self.inner.execution_ctx.get_step_data(&map_hashed_id).await {
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
                        if let Some(payload) =
                            op.context_details.as_ref().and_then(|d| d.result.as_ref())
                        {
                            if let Some(batch_serdes) = batch_serdes.clone() {
                                let batch: BatchResult<TOut> =
                                    safe_deserialize_required_with_serdes(
                                        batch_serdes,
                                        payload,
                                        &map_hashed_id,
                                        name,
                                        &self.inner.execution_ctx,
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
                                for (index, item) in
                                    items.iter().enumerate().take(target_total_count)
                                {
                                    let item_name = if let Some(ref namer) = item_namer {
                                        namer(item, index)
                                    } else {
                                        format!("{}-item-{}", name.unwrap_or("map"), index)
                                    };
                                    let _ = self
                                        .inner
                                        .execution_ctx
                                        .next_operation_id(Some(&item_name));
                                }

                                return Ok(batch);
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

                                let child_step_id =
                                    self.inner.execution_ctx.next_operation_id(Some(&item_name));
                                let child_hashed_id = DurableContextImpl::hash_id(&child_step_id);

                                if let Some(child_op) = self
                                    .inner
                                    .execution_ctx
                                    .get_step_data(&child_hashed_id)
                                    .await
                                {
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
                                                        &self.inner.execution_ctx,
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
                                                .unwrap_or_else(|| {
                                                    "Child context failed".to_string()
                                                });
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
                            let completion_reason = compute_completion_reason(
                                failures.len(),
                                successes.len(),
                                completed_count,
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

                            return Ok(BatchResult {
                                all,
                                completion_reason,
                            });
                        }
                    }
                    _ => {
                        // Incomplete top-level map during replay; continue execution.
                        self.inner
                            .execution_ctx
                            .set_mode(ExecutionMode::Execution)
                            .await;
                    }
                }
            }
        }

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
                    &self.inner.execution_ctx,
                )
                .await
            } else {
                safe_serialize(
                    None,
                    Some(&map_summary_payload(0, 0, 0, completion_reason)),
                    &map_hashed_id,
                    name,
                    &self.inner.execution_ctx,
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
            self.inner
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

                let child_step_id = self.inner.execution_ctx.next_operation_id(Some(&item_name));
                let child_hashed_id = DurableContextImpl::hash_id(&child_step_id);
                started_indices.insert(index);

                let child_cfg = ChildContextConfig::<TOut> {
                    sub_type: Some("MapIteration".to_string()),
                    serdes: item_serdes.clone(),
                    ..Default::default()
                };

                let inner = Arc::clone(&self.inner);
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
            let (index, res) = joined
                .map_err(|e| DurableError::Internal(format!("Child task join error: {}", e)))?;

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
                        &self.inner.execution_ctx,
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
                        &self.inner.execution_ctx,
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
                        DurableError::Internal(format!(
                            "Failed to build map completion update: {e}"
                        ))
                    })?;
                self.inner
                    .execution_ctx
                    .checkpoint_manager
                    .checkpoint(map_step_id.clone(), succeed_update)
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
                &map_hashed_id,
                name,
                &self.inner.execution_ctx,
            )
            .await
        } else {
            safe_serialize(
                None,
                Some(&map_summary_payload(
                    completed_count,
                    success_count,
                    failure_count,
                    completion_reason,
                )),
                &map_hashed_id,
                name,
                &self.inner.execution_ctx,
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
        self.inner
            .execution_ctx
            .checkpoint_manager
            .checkpoint(map_step_id, succeed_update)
            .await?;

        Ok(batch_result)
    }
}
