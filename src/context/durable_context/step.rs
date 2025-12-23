use super::*;

mod execute;
mod replay;

#[derive(Debug)]
struct StepInterruptedError {
    step_id: String,
    name: Option<String>,
}

impl std::fmt::Display for StepInterruptedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.name.as_deref() {
            Some(name) => write!(f, "Step interrupted: {} ({})", name, self.step_id),
            None => write!(f, "Step interrupted: {}", self.step_id),
        }
    }
}

impl std::error::Error for StepInterruptedError {}

impl DurableContextHandle {
    /// Execute a durable step with automatic retry and checkpointing.
    ///
    /// Steps are the fundamental building blocks of durable functions. Each step
    /// is checkpointed after completion, providing replay-safe semantics across
    /// Lambda restarts. Use [`StepSemantics`](crate::types::StepSemantics) to
    /// choose at-least-once vs at-most-once per retry cycle.
    ///
    /// # Arguments
    ///
    /// * `name` - Optional name for tracking, debugging, and replay validation.
    ///   Providing meaningful names helps with debugging and observability.
    /// * `step_fn` - Async function to execute. Receives a [`StepContext`] for logging.
    /// * `config` - Optional [`StepConfig`] for retry strategy and execution semantics.
    ///
    /// # Returns
    ///
    /// The result of the step function, either fresh or from cache on replay.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use lambda_durable_execution_rust::prelude::*;
    /// # use lambda_durable_execution_rust::retry::ExponentialBackoff;
    /// # async fn fetch_data() -> Result<String, Box<dyn std::error::Error + Send + Sync>> { Ok("data".to_string()) }
    /// async fn example(ctx: DurableContextHandle) -> DurableResult<(u32, String, u32)> {
    ///     // Basic step
    ///     let a: u32 = ctx
    ///         .step(Some("compute-value"), |_| async { Ok(42u32) }, None)
    ///         .await?;
    ///
    ///     // Step with logging
    ///     let data: String = ctx
    ///         .step(
    ///             Some("fetch-data"),
    ///             |step_ctx| async move {
    ///                 step_ctx.info("Starting fetch");
    ///                 let result = fetch_data().await?;
    ///                 step_ctx.info("Fetch complete");
    ///                 Ok(result)
    ///             },
    ///             None,
    ///         )
    ///         .await?;
    ///
    ///     // Step with custom retry strategy
    ///     let retry = ExponentialBackoff::builder()
    ///         .max_attempts(5)
    ///         .initial_delay(Duration::seconds(1))
    ///         .build();
    ///     let config = StepConfig::<u32>::new().with_retry_strategy(Arc::new(retry));
    ///     let b: u32 = ctx
    ///         .step(Some("risky-operation"), |_| async { Ok(7u32) }, Some(config))
    ///         .await?;
    ///
    ///     Ok((a, data, b))
    /// }
    /// ```
    ///
    /// # Replay Behavior
    ///
    /// On replay (when Lambda restarts), if the step was previously completed,
    /// the cached result is returned without re-executing the step function.
    /// This ensures idempotency.
    pub async fn step<T, F, Fut>(
        &self,
        name: Option<&str>,
        step_fn: F,
        config: Option<StepConfig<T>>,
    ) -> DurableResult<T>
    where
        T: Serialize + DeserializeOwned + Send + Sync + 'static,
        F: FnOnce(StepContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, Box<dyn std::error::Error + Send + Sync>>> + Send + 'static,
    {
        self.inner.step(name, step_fn, config).await
    }
}

impl DurableContextImpl {
    /// Execute a step.
    pub async fn step<T, F, Fut>(
        &self,
        name: Option<&str>,
        step_fn: F,
        config: Option<StepConfig<T>>,
    ) -> DurableResult<T>
    where
        T: Serialize + DeserializeOwned + Send + Sync + 'static,
        F: FnOnce(StepContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, Box<dyn std::error::Error + Send + Sync>>> + Send + 'static,
    {
        let step_id = self.execution_ctx.next_operation_id(name);
        let hashed_id = Self::hash_id(&step_id);
        let config = config.unwrap_or_default();
        let semantics = config.semantics;
        let retry_strategy = config.retry_strategy.unwrap_or_else(|| presets::default());
        let serdes = config.serdes.clone();

        let parent_id = self.execution_ctx.get_parent_id().await;
        let operation = self.execution_ctx.get_step_data(&hashed_id).await;

        // Replay handling: short-circuit completed, surface failures, or suspend.
        if let Some(result) = replay::handle_replay(
            operation.as_ref(),
            &step_id,
            &hashed_id,
            name,
            semantics,
            &retry_strategy,
            serdes.clone(),
            &parent_id,
            &self.execution_ctx,
        )
        .await?
        {
            return Ok(result);
        }

        execute::run_step_execution(
            self,
            name,
            step_fn,
            step_id,
            hashed_id,
            operation,
            semantics,
            retry_strategy,
            serdes,
            parent_id,
        )
        .await
    }
}
