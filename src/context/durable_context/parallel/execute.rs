use super::super::*;

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
                .unwrap_or_else(|| format!("{}-branch-{}", base, index));

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
            joined.map_err(|e| DurableError::Internal(format!("Child task join error: {}", e)))?;

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
