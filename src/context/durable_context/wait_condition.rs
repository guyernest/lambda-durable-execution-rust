use super::*;

mod execute;
mod replay;

impl DurableContextHandle {
    /// Wait for a condition by periodically re-running a check function.
    ///
    /// This is equivalent to the JS SDK `waitForCondition`. Each attempt runs as a durable
    /// step with subtype `WaitForCondition`, checkpointing the intermediate state and
    /// scheduling a retry according to `wait_strategy`.
    pub async fn wait_for_condition<T, F, Fut>(
        &self,
        name: Option<&str>,
        check_fn: F,
        config: WaitConditionConfig<T>,
    ) -> DurableResult<T>
    where
        T: Serialize + DeserializeOwned + Send + Sync + Clone + 'static,
        F: Fn(T, StepContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = DurableResult<T>> + Send + 'static,
    {
        let check_fn = Arc::new(check_fn);
        let serdes = config.serdes.clone();

        let step_id = self.inner.execution_ctx.next_operation_id(name);
        let hashed_id = DurableContextImpl::hash_id(&step_id);

        // Replay short-circuit.
        if let Some(result) = replay::handle_replay(
            self.inner.execution_ctx.get_step_data(&hashed_id).await,
            serdes.clone(),
            &hashed_id,
            &step_id,
            name,
            &self.inner.execution_ctx,
        )
        .await?
        {
            return Ok(result);
        }

        execute::run_wait_condition(
            Arc::clone(&self.inner),
            name,
            check_fn,
            config,
            step_id,
            hashed_id,
        )
        .await
    }
}
