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
        let cfg = config.unwrap_or_default();
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
        // Replay handling extracted to map::replay.
        if mode == ExecutionMode::Replay {
            if let Some(result) = replay::maybe_replay_map(
                name,
                &items,
                &item_namer,
                &batch_serdes,
                &item_serdes,
                &self.inner.execution_ctx,
                &map_hashed_id,
                &cfg.completion_config,
            )
            .await?
            {
                return Ok(result);
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
