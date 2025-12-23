use super::*;

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
        if let Some(op) = operation.as_ref() {
            match op.status {
                OperationStatus::Succeeded => {
                    if let Some(ref details) = op.step_details {
                        if let Some(ref payload) = details.result {
                            if let Some(val) = safe_deserialize(
                                serdes,
                                Some(payload.as_str()),
                                &hashed_id,
                                name,
                                &self.execution_ctx,
                            )
                            .await
                            {
                                return Ok(val);
                            }
                        }
                    }
                    return Err(DurableError::Internal(
                        "Missing step output in replay".to_string(),
                    ));
                }
                OperationStatus::Failed => {
                    let attempt_idx = op
                        .step_details
                        .as_ref()
                        .and_then(|d| d.attempt)
                        .unwrap_or(0);
                    let attempts = attempt_idx + 1;
                    let message = op
                        .step_details
                        .as_ref()
                        .and_then(|d| d.error.as_ref())
                        .map(|e| e.error_message.clone())
                        .unwrap_or_else(|| "Replayed failure".to_string());
                    return Err(DurableError::step_failed_msg(step_id, attempts, message));
                }
                OperationStatus::Pending => {
                    // Retry is scheduled; suspend until the backend advances state.
                    self.execution_ctx.set_mode(ExecutionMode::Execution).await;
                    self.execution_ctx
                        .termination_manager
                        .terminate_for_retry()
                        .await;
                    std::future::pending::<()>().await;
                    unreachable!()
                }
                OperationStatus::Started if semantics == StepSemantics::AtMostOncePerRetry => {
                    // Interrupted step in at-most-once semantics: treat as a failure and schedule retry.
                    self.execution_ctx.set_mode(ExecutionMode::Execution).await;

                    let attempt_idx = op
                        .step_details
                        .as_ref()
                        .and_then(|d| d.attempt)
                        .unwrap_or(0);
                    let attempts_made = attempt_idx + 1;
                    let interrupted = StepInterruptedError {
                        step_id: step_id.clone(),
                        name: name.map(|s| s.to_string()),
                    };

                    let decision = retry_strategy.should_retry(&interrupted, attempts_made);
                    let err_obj = ErrorObject::from_error(&interrupted);

                    if !decision.should_retry {
                        let mut builder = OperationUpdate::builder()
                            .id(&hashed_id)
                            .operation_type(OperationType::Step)
                            .sub_type("Step")
                            .action(OperationAction::Fail)
                            .error(err_obj);
                        if let Some(ref pid) = parent_id {
                            builder = builder.parent_id(pid);
                        }
                        if let Some(n) = name {
                            builder = builder.name(n);
                        }
                        self.execution_ctx
                            .checkpoint_manager
                            .checkpoint(
                                step_id.clone(),
                                builder.build().map_err(|e| {
                                    DurableError::Internal(format!(
                                        "Failed to build step FAIL update: {e}"
                                    ))
                                })?,
                            )
                            .await?;

                        return Err(DurableError::step_failed_msg(
                            step_id,
                            attempts_made,
                            interrupted.to_string(),
                        ));
                    }

                    let delay = decision.delay.unwrap_or(Duration::seconds(1));
                    let mut builder = OperationUpdate::builder()
                        .id(&hashed_id)
                        .operation_type(OperationType::Step)
                        .sub_type("Step")
                        .action(OperationAction::Retry)
                        .error(err_obj)
                        .step_options(crate::types::StepUpdateOptions {
                            next_attempt_delay_seconds: Some(delay.to_seconds_i32_saturating()),
                        });
                    if let Some(ref pid) = parent_id {
                        builder = builder.parent_id(pid);
                    }
                    if let Some(n) = name {
                        builder = builder.name(n);
                    }

                    self.execution_ctx
                        .checkpoint_manager
                        .checkpoint(
                            step_id.clone(),
                            builder.build().map_err(|e| {
                                DurableError::Internal(format!(
                                    "Failed to build step RETRY update: {e}"
                                ))
                            })?,
                        )
                        .await?;

                    self.execution_ctx
                        .termination_manager
                        .terminate_for_retry()
                        .await;

                    std::future::pending::<()>().await;
                    unreachable!()
                }
                _ => {
                    // Started (at-least-once) - continue execution without re-starting.
                }
            }
        }

        // We're executing a new (or interrupted-at-least-once) step now.
        self.execution_ctx.set_mode(ExecutionMode::Execution).await;

        let already_started = matches!(
            operation.as_ref().map(|op| op.status),
            Some(OperationStatus::Started | OperationStatus::Ready)
        );

        if !already_started {
            // Phase 1: checkpoint START depending on semantics.
            let mut builder = OperationUpdate::builder()
                .id(&hashed_id)
                .operation_type(OperationType::Step)
                .sub_type("Step")
                .action(OperationAction::Start);
            if let Some(ref pid) = parent_id {
                builder = builder.parent_id(pid);
            }
            if let Some(n) = name {
                builder = builder.name(n);
            }

            let start_update = builder.build().map_err(|e| {
                DurableError::Internal(format!("Failed to build step START update: {e}"))
            })?;
            match semantics {
                StepSemantics::AtMostOncePerRetry => {
                    self.execution_ctx
                        .checkpoint_manager
                        .checkpoint(step_id.clone(), start_update)
                        .await?;
                }
                StepSemantics::AtLeastOncePerRetry => {
                    // Enqueue without waiting, mirroring JS/Python semantics while preserving ordering
                    // with subsequent checkpoints for the same operation.
                    self.execution_ctx
                        .checkpoint_manager
                        .checkpoint_queued(step_id.clone(), start_update)
                        .await?;
                }
            }
        }

        // Create step context
        let attempt_idx = operation
            .as_ref()
            .and_then(|op| op.step_details.as_ref().and_then(|d| d.attempt))
            .unwrap_or(0);
        let attempt = attempt_idx + 1;
        let mode_now = self.execution_ctx.get_mode().await;
        let step_ctx = StepContext::new_with_logger(
            name.map(String::from),
            hashed_id.clone(),
            self.execution_ctx.durable_execution_arn.clone(),
            self.execution_ctx.logger.clone(),
            mode_now,
            self.execution_ctx.mode_aware_logging,
            Some(attempt),
        );

        // Execute step function
        match step_fn(step_ctx).await {
            Ok(result) => {
                // Checkpoint SUCCESS
                let payload =
                    safe_serialize(serdes, Some(&result), &hashed_id, name, &self.execution_ctx)
                        .await;

                let mut builder = OperationUpdate::builder()
                    .id(&hashed_id)
                    .operation_type(OperationType::Step)
                    .sub_type("Step")
                    .action(OperationAction::Succeed);

                if let Some(ref pid) = parent_id {
                    builder = builder.parent_id(pid);
                }
                if let Some(n) = name {
                    builder = builder.name(n);
                }

                if let Some(p) = payload {
                    builder = builder.payload(p);
                }

                self.execution_ctx
                    .checkpoint_manager
                    .checkpoint(
                        step_id.clone(),
                        builder.build().map_err(|e| {
                            DurableError::Internal(format!(
                                "Failed to build step SUCCEED update: {e}"
                            ))
                        })?,
                    )
                    .await?;

                Ok(result)
            }
            Err(error) => {
                let attempts_made = attempt;
                let decision = retry_strategy.should_retry(error.as_ref(), attempts_made);

                if decision.should_retry {
                    let delay = decision.delay.unwrap_or(Duration::seconds(1));
                    let error_obj = ErrorObject::from_error(error.as_ref());

                    // Checkpoint retry with delay - triggers termination
                    let mut builder = OperationUpdate::builder()
                        .id(&hashed_id)
                        .operation_type(OperationType::Step)
                        .sub_type("Step")
                        .action(OperationAction::Retry)
                        .error(error_obj)
                        .step_options(crate::types::StepUpdateOptions {
                            next_attempt_delay_seconds: Some(delay.to_seconds_i32_saturating()),
                        });

                    if let Some(ref pid) = parent_id {
                        builder = builder.parent_id(pid);
                    }
                    if let Some(n) = name {
                        builder = builder.name(n);
                    }

                    self.execution_ctx
                        .checkpoint_manager
                        .checkpoint(
                            step_id.clone(),
                            builder.build().map_err(|e| {
                                DurableError::Internal(format!(
                                    "Failed to build step RETRY update: {e}"
                                ))
                            })?,
                        )
                        .await?;

                    // Trigger termination for retry
                    self.execution_ctx
                        .termination_manager
                        .terminate_for_retry()
                        .await;

                    // Never reached
                    std::future::pending::<()>().await;
                    unreachable!()
                }

                // No more retries - fail
                let error_obj = ErrorObject::from_error(error.as_ref());

                let mut builder = OperationUpdate::builder()
                    .id(&hashed_id)
                    .operation_type(OperationType::Step)
                    .sub_type("Step")
                    .action(OperationAction::Fail)
                    .error(error_obj);

                if let Some(ref pid) = parent_id {
                    builder = builder.parent_id(pid);
                }
                if let Some(n) = name {
                    builder = builder.name(n);
                }

                self.execution_ctx
                    .checkpoint_manager
                    .checkpoint(
                        step_id.clone(),
                        builder.build().map_err(|e| {
                            DurableError::Internal(format!("Failed to build step FAIL update: {e}"))
                        })?,
                    )
                    .await?;

                Err(DurableError::step_failed_boxed(
                    step_id,
                    attempts_made,
                    error,
                ))
            }
        }
    }
}
