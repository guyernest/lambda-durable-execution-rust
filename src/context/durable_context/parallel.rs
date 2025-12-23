use super::*;

impl DurableContextHandle {
    /// Execute multiple branches in parallel with deterministic concurrency.
    pub async fn parallel<T, F, Fut>(
        &self,
        name: Option<&str>,
        branches: Vec<F>,
        config: Option<ParallelConfig<T>>,
    ) -> DurableResult<BatchResult<T>>
    where
        T: Serialize + DeserializeOwned + Send + Sync + 'static,
        F: Fn(DurableContextHandle) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = DurableResult<T>> + Send + 'static,
    {
        let named: Vec<NamedParallelBranch<F>> = branches
            .into_iter()
            .map(|func| NamedParallelBranch { name: None, func })
            .collect();
        return self.parallel_named(name, named, config).await;
        /*

        let par_step_id = self.inner.execution_ctx.next_operation_id(name);
        let par_hashed_id = DurableContextImpl::hash_id(&par_step_id);
        if self
            .inner
            .execution_ctx
            .get_step_data(&par_hashed_id)
            .await
            .is_none()
        {
            let parent_id = self.inner.execution_ctx.get_parent_id().await;
            let mut builder = OperationUpdate::builder()
                .id(&par_hashed_id)
                .operation_type(OperationType::Context)
                .sub_type("Parallel")
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
                    par_step_id.clone(),
                    builder.build().map_err(|e| {
                        DurableError::Internal(format!(
                            "Failed to build parallel START update: {e}"
                        ))
                    })?,
                )
                .await?;
        }

        // Replay handling: if the top-level parallel completed, reconstruct children and skip incomplete ones.
        if mode == ExecutionMode::Replay {
            if let Some(op) = self.inner.execution_ctx.get_step_data(&par_hashed_id).await {
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
                            let mut completed_count = 0usize;

                            for (index, _branch) in branches.into_iter().enumerate() {
                                if completed_count >= target_total_count {
                                    break;
                                }

                                let branch_name =
                                    format!("{}-branch-{}", name.unwrap_or("parallel"), index);
                                let child_step_id = self
                                    .inner
                                    .execution_ctx
                                    .next_operation_id(Some(&branch_name));
                                let child_hashed_id = DurableContextImpl::hash_id(&child_step_id);

                                if let Some(child_op) = self
                                    .inner
                                    .execution_ctx
                                    .get_step_data(&child_hashed_id)
                                    .await
                                {
                                    match child_op.status {
                                        OperationStatus::Succeeded => {
                                            if let Some(ref details) = child_op.context_details {
                                                if let Some(ref payload) = details.result {
                                                    let val: T = safe_deserialize(
                                                        item_serdes.clone(),
                                                        Some(payload.as_str()),
                                                        &child_hashed_id,
                                                        Some(&branch_name),
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
                                                    completed_count += 1;
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
                                            completed_count += 1;
                                        }
                                        _ => continue,
                                    }
                                } else {
                                    continue;
                                }
                            }

                            let completion_reason = if target_total_count < branches_len {
                                BatchCompletionReason::MinSuccessfulReached
                            } else {
                                BatchCompletionReason::AllCompleted
                            };

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

                            return Ok(BatchResult {
                                all,
                                completion_reason,
                            });
                        }
                    }
                    _ => {
                        self.inner
                            .execution_ctx
                            .set_mode(ExecutionMode::Execution)
                            .await;
                    }
                }
            }
        }

        let mut successes = Vec::new();
        let mut failures = Vec::new();
        let mut success_count = 0usize;
        let mut failure_count = 0usize;

        let max_concurrency = cfg
            .max_concurrency
            .unwrap_or_else(|| branches_len.max(1))
            .max(1);
        let min_successful = cfg.completion_config.min_successful;
        let mut stop_starting = false;
        let mut join_set = tokio::task::JoinSet::new();
        let mut branches_iter = branches.into_iter().enumerate();

        loop {
            while !stop_starting && join_set.len() < max_concurrency {
                if let Some(min) = min_successful {
                    if success_count >= min {
                        stop_starting = true;
                        break;
                    }
                }

                let Some((index, branch)) = branches_iter.next() else {
                    break;
                };

                let branch_name = format!("{}-branch-{}", name.unwrap_or("parallel"), index);
                let child_step_id = self
                    .inner
                    .execution_ctx
                    .next_operation_id(Some(&branch_name));
                let child_hashed_id = DurableContextImpl::hash_id(&child_step_id);

                let child_cfg = ChildContextConfig::<T> {
                    sub_type: Some("ParallelBranch".to_string()),
                    serdes: item_serdes.clone(),
                    ..Default::default()
                };

                let inner = Arc::clone(&self.inner);
                join_set.spawn(async move {
                    let res = inner
                        .run_in_child_context_with_ids(
                            child_step_id.clone(),
                            child_hashed_id,
                            Some(&branch_name),
                            move |child_ctx| branch(child_ctx),
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

            match res {
                Ok(v) => {
                    successes.push((index, v));
                    success_count += 1;
                    if let Some(min) = min_successful {
                        if success_count >= min {
                            stop_starting = true;
                        }
                    }
                }
                Err(e) => {
                    failures.push((index, e));
                    failure_count += 1;

                    if !has_any_completion_criteria {
                        join_set.abort_all();
                        while join_set.join_next().await.is_some() {}
                        let error = DurableError::BatchOperationFailed {
                            name: name.unwrap_or("parallel").to_string(),
                            message: "Failure tolerance exceeded".to_string(),
                            successful_count: success_count,
                            failed_count: failure_count,
                        };
                        let err_obj = ErrorObject::from_durable_error(&error);
                        let fail_update = OperationUpdate::builder()
                            .id(&par_hashed_id)
                            .operation_type(OperationType::Context)
                            .sub_type("Parallel")
                            .action(OperationAction::Fail)
                            .error(err_obj)
                            .build()
                            .unwrap();
                        self.inner
                            .execution_ctx
                            .checkpoint_manager
                            .checkpoint(par_step_id.clone(), fail_update)
                            .await?;
                        return Err(error);
                    }

                    if let Some(tol) = cfg.completion_config.tolerated_failure_count {
                        if failure_count > tol {
                            join_set.abort_all();
                            while join_set.join_next().await.is_some() {}
                            let error = DurableError::BatchOperationFailed {
                                name: name.unwrap_or("parallel").to_string(),
                                message: "Failure tolerance exceeded".to_string(),
                                successful_count: success_count,
                                failed_count: failure_count,
                            };
                            let err_obj = ErrorObject::from_durable_error(&error);
                            let fail_update = OperationUpdate::builder()
                                .id(&par_hashed_id)
                                .operation_type(OperationType::Context)
                                .sub_type("Parallel")
                                .action(OperationAction::Fail)
                                .error(err_obj)
                                .build()
                                .unwrap();
                            self.inner
                                .execution_ctx
                                .checkpoint_manager
                                .checkpoint(par_step_id.clone(), fail_update)
                                .await?;
                            return Err(error);
                        }
                    }
                    if let Some(pct) = cfg.completion_config.tolerated_failure_percentage {
                        if branches_len > 0 {
                            let failure_pct = (failure_count as f64 / branches_len as f64) * 100.0;
                            if failure_pct > pct {
                                join_set.abort_all();
                                while join_set.join_next().await.is_some() {}
                                let error = DurableError::BatchOperationFailed {
                                    name: name.unwrap_or("parallel").to_string(),
                                    message: "Failure tolerance exceeded".to_string(),
                                    successful_count: success_count,
                                    failed_count: failure_count,
                                };
                                let err_obj = ErrorObject::from_durable_error(&error);
                                let fail_update = OperationUpdate::builder()
                                    .id(&par_hashed_id)
                                    .operation_type(OperationType::Context)
                                    .sub_type("Parallel")
                                    .action(OperationAction::Fail)
                                    .error(err_obj)
                                    .build()
                                    .unwrap();
                                self.inner
                                    .execution_ctx
                                    .checkpoint_manager
                                    .checkpoint(par_step_id.clone(), fail_update)
                                    .await?;
                                return Err(error);
                            }
                        }
                    }
                }
            }
        }

        let summary = serde_json::to_string(&serde_json::json!({
            "totalCount": success_count + failure_count,
            "successCount": success_count,
            "failureCount": failure_count,
        }))
        .unwrap();

        let succeed_update = OperationUpdate::builder()
            .id(&par_hashed_id)
            .operation_type(OperationType::Context)
            .sub_type("Parallel")
            .action(OperationAction::Succeed)
            .payload(summary)
            .build()
            .unwrap();
        self.inner
            .execution_ctx
            .checkpoint_manager
            .checkpoint(par_step_id, succeed_update)
            .await?;

        let completion_reason = if cfg.completion_config.min_successful.is_some()
            && (success_count + failure_count) < branches_len
        {
            BatchCompletionReason::MinSuccessfulReached
        } else {
            BatchCompletionReason::AllCompleted
        };

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

        Ok(BatchResult {
            all,
            completion_reason,
        })
        */
    }

    /// Execute multiple branches in parallel, allowing optional per-branch names.
    pub async fn parallel_named<T, F, Fut>(
        &self,
        name: Option<&str>,
        branches: Vec<NamedParallelBranch<F>>,
        config: Option<ParallelConfig<T>>,
    ) -> DurableResult<BatchResult<T>>
    where
        T: Serialize + DeserializeOwned + Send + Sync + 'static,
        F: Fn(DurableContextHandle) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = DurableResult<T>> + Send + 'static,
    {
        let cfg = config.unwrap_or_default();
        let branches_len = branches.len();
        validate_completion_config(
            &cfg.completion_config,
            branches_len,
            name.unwrap_or("parallel"),
        )?;
        let mode = self.inner.execution_ctx.get_mode().await;
        let item_serdes = cfg.item_serdes.clone();
        let batch_serdes = cfg.serdes.clone();

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

        // Start top-level PARALLEL context.
        let par_step_id = self.inner.execution_ctx.next_operation_id(name);
        let par_hashed_id = DurableContextImpl::hash_id(&par_step_id);
        if self
            .inner
            .execution_ctx
            .get_step_data(&par_hashed_id)
            .await
            .is_none()
        {
            let parent_id = self.inner.execution_ctx.get_parent_id().await;
            let mut builder = OperationUpdate::builder()
                .id(&par_hashed_id)
                .operation_type(OperationType::Context)
                .sub_type("Parallel")
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
                    par_step_id.clone(),
                    builder.build().map_err(|e| {
                        DurableError::Internal(format!(
                            "Failed to build parallel START update: {e}"
                        ))
                    })?,
                )
                .await?;
        }

        // Replay handling: reconstruct completed branches only.
        if mode == ExecutionMode::Replay {
            if let Some(op) = self.inner.execution_ctx.get_step_data(&par_hashed_id).await {
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
                        if let Some(payload) =
                            op.context_details.as_ref().and_then(|d| d.result.as_ref())
                        {
                            if let Some(batch_serdes) = batch_serdes.clone() {
                                let batch: BatchResult<T> = safe_deserialize_required_with_serdes(
                                    batch_serdes,
                                    payload,
                                    &par_hashed_id,
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

                                if target_total_count > branches_len {
                                    return Err(DurableError::ReplayValidationFailed {
                                        expected: format!(
                                            "parallel totalCount <= {}",
                                            branches_len
                                        ),
                                        actual: target_total_count.to_string(),
                                    });
                                }

                                // Consume child context operation IDs to keep the parent context counter in sync.
                                for (index, branch) in
                                    branches.iter().enumerate().take(target_total_count)
                                {
                                    let base = name.unwrap_or("parallel");
                                    let branch_name = branch
                                        .name
                                        .clone()
                                        .unwrap_or_else(|| format!("{}-branch-{}", base, index));
                                    let _ = self
                                        .inner
                                        .execution_ctx
                                        .next_operation_id(Some(&branch_name));
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

                            for (index, branch) in branches.iter().enumerate() {
                                if seen_count >= target_total_count {
                                    break;
                                }

                                let base = name.unwrap_or("parallel");
                                let branch_name = branch
                                    .name
                                    .clone()
                                    .unwrap_or_else(|| format!("{}-branch-{}", base, index));

                                let child_step_id = self
                                    .inner
                                    .execution_ctx
                                    .next_operation_id(Some(&branch_name));
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
                                                    let val: T = safe_deserialize(
                                                        item_serdes.clone(),
                                                        Some(payload.as_str()),
                                                        &child_hashed_id,
                                                        Some(&branch_name),
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
                        self.inner
                            .execution_ctx
                            .set_mode(ExecutionMode::Execution)
                            .await;
                    }
                }
            }
        }

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
                    .unwrap_or_else(|| format!("{}-branch-{}", base, index));

                let child_step_id = self
                    .inner
                    .execution_ctx
                    .next_operation_id(Some(&branch_name));
                let child_hashed_id = DurableContextImpl::hash_id(&child_step_id);
                started_indices.insert(index);

                let child_cfg = ChildContextConfig::<T> {
                    sub_type: Some("ParallelBranch".to_string()),
                    serdes: item_serdes.clone(),
                    ..Default::default()
                };

                let inner = Arc::clone(&self.inner);
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
                        &self.inner.execution_ctx,
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
                        &self.inner.execution_ctx,
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
                self.inner
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
                &self.inner.execution_ctx,
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
                &self.inner.execution_ctx,
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
        self.inner
            .execution_ctx
            .checkpoint_manager
            .checkpoint(par_step_id, succeed_update)
            .await?;

        Ok(batch_result)
    }
}
