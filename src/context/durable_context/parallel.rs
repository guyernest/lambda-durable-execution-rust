use super::*;

mod execute;
mod replay;

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
        self.parallel_named(name, named, config).await
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
            if let Some(result) = replay::maybe_replay_parallel(
                name,
                &branches,
                &batch_serdes,
                &item_serdes,
                &self.inner.execution_ctx,
                &par_hashed_id,
                &cfg.completion_config,
            )
            .await?
            {
                return Ok(result);
            }
        }

        execute::run_parallel_execution(
            Arc::clone(&self.inner),
            name,
            branches,
            cfg,
            item_serdes,
            batch_serdes,
            par_step_id,
            par_hashed_id,
        )
        .await
    }
}
