use super::*;

mod execute;
mod replay;

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
        let mut cfg = config.unwrap_or_default();
        if cfg.serdes.is_none() {
            cfg.serdes = Some(Arc::new(BatchResultSerdes));
        }
        let map_fn = Arc::new(map_fn);
        let items_len = items.len();
        validate_completion_config(&cfg.completion_config, items_len, name.unwrap_or("map"))?;
        let mode = self.inner.execution_ctx.get_mode().await;
        let item_serdes = cfg.item_serdes.clone();
        let item_namer = cfg.item_namer.clone();
        let batch_serdes = cfg.serdes.clone();

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
            let parent_id = self.inner.execution_ctx.get_parent_id();
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
            #[cfg(coverage)]
            let update = builder
                .build()
                .expect("map START update should always be valid");
            #[cfg(not(coverage))]
            let update = builder.build().map_err(|e| {
                DurableError::Internal(format!("Failed to build map START update: {e}"))
            })?;

            self.inner
                .execution_ctx
                .checkpoint_manager
                .checkpoint(map_step_id.clone(), update)
                .await?;
        }

        if mode == ExecutionMode::Replay {
            match replay::evaluate_map_replay(
                name,
                &items,
                &item_namer,
                &batch_serdes,
                &self.inner.execution_ctx,
                &map_hashed_id,
                &cfg.completion_config,
            )
            .await?
            {
                replay::MapReplayDecision::Return(result) => return Ok(result),
                replay::MapReplayDecision::Reconstruct { total_count } => {
                    return replay::reconstruct_map_from_children(
                        Arc::clone(&self.inner),
                        name,
                        items,
                        Arc::clone(&map_fn),
                        item_namer,
                        item_serdes,
                        &map_hashed_id,
                        total_count,
                        &cfg.completion_config,
                    )
                    .await;
                }
                replay::MapReplayDecision::Continue => {}
            }
        }

        execute::run_map_execution(
            Arc::clone(&self.inner),
            name,
            items,
            map_fn,
            cfg,
            item_namer,
            item_serdes,
            batch_serdes,
            map_step_id,
            map_hashed_id,
        )
        .await
    }
}
