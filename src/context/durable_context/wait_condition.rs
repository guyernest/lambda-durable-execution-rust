use super::*;

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

        // Get current state from replay if present.
        let mut state = if let Some(op) = self.inner.execution_ctx.get_step_data(&hashed_id).await {
            if let Some(details) = op.step_details {
                if let Some(payload) = details.result {
                    safe_deserialize(
                        serdes.clone(),
                        Some(payload.as_str()),
                        &hashed_id,
                        name,
                        &self.inner.execution_ctx,
                    )
                    .await
                    .unwrap_or_else(|| config.initial_state.clone())
                } else {
                    config.initial_state.clone()
                }
            } else {
                config.initial_state.clone()
            }
        } else {
            config.initial_state.clone()
        };

        let attempt = self
            .inner
            .execution_ctx
            .get_step_data(&hashed_id)
            .await
            .and_then(|op| op.step_details.and_then(|d| d.attempt))
            .unwrap_or(0)
            + 1;

        if let Some(max) = config.max_attempts {
            if attempt > max {
                let error = DurableError::WaitConditionExceeded {
                    name: name.unwrap_or("wait_for_condition").to_string(),
                    attempts: attempt,
                };
                let err_obj = ErrorObject::from_durable_error(&error);
                let fail_update = OperationUpdate::builder()
                    .id(&hashed_id)
                    .operation_type(OperationType::Step)
                    .sub_type("WaitForCondition")
                    .action(OperationAction::Fail)
                    .error(err_obj)
                    .build()
                    .map_err(|e| {
                        DurableError::Internal(format!(
                            "Failed to build wait_for_condition FAIL update: {e}"
                        ))
                    })?;
                self.inner
                    .execution_ctx
                    .checkpoint_manager
                    .checkpoint(step_id.clone(), fail_update)
                    .await?;
                return Err(error);
            }
        }

        // Start step if not already started.
        if self
            .inner
            .execution_ctx
            .get_step_data(&hashed_id)
            .await
            .is_none()
        {
            let parent_id = self.inner.execution_ctx.get_parent_id().await;
            let mut builder = OperationUpdate::builder()
                .id(&hashed_id)
                .operation_type(OperationType::Step)
                .sub_type("WaitForCondition")
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
                    step_id.clone(),
                    builder.build().map_err(|e| {
                        DurableError::Internal(format!(
                            "Failed to build wait_for_condition START update: {e}"
                        ))
                    })?,
                )
                .await?;
        }

        let mode_now = self.inner.execution_ctx.get_mode().await;
        let step_ctx = StepContext::new_with_logger(
            name.map(String::from),
            hashed_id.clone(),
            self.inner.execution_ctx.durable_execution_arn.clone(),
            self.inner.execution_ctx.logger.clone(),
            mode_now,
            self.inner.execution_ctx.mode_aware_logging,
            None,
        );
        let new_state = check_fn(state, step_ctx).await?;
        state = new_state;

        let payload = safe_serialize(
            serdes.clone(),
            Some(&state),
            &hashed_id,
            name,
            &self.inner.execution_ctx,
        )
        .await;

        match (config.wait_strategy)(&state, attempt) {
            WaitConditionDecision::Stop => {
                let mut succeed_builder = OperationUpdate::builder()
                    .id(&hashed_id)
                    .operation_type(OperationType::Step)
                    .sub_type("WaitForCondition")
                    .action(OperationAction::Succeed);
                if let Some(p) = payload.clone() {
                    succeed_builder = succeed_builder.payload(p);
                }
                let succeed_update = succeed_builder.build().map_err(|e| {
                    DurableError::Internal(format!(
                        "Failed to build wait_for_condition SUCCEED update: {e}"
                    ))
                })?;
                self.inner
                    .execution_ctx
                    .checkpoint_manager
                    .checkpoint(step_id, succeed_update)
                    .await?;

                Ok(state)
            }
            WaitConditionDecision::Continue { delay } => {
                let mut retry_builder = OperationUpdate::builder()
                    .id(&hashed_id)
                    .operation_type(OperationType::Step)
                    .sub_type("WaitForCondition")
                    .action(OperationAction::Retry)
                    .step_options(crate::types::StepUpdateOptions {
                        next_attempt_delay_seconds: Some(delay.to_seconds_i32_saturating()),
                    });
                if let Some(p) = payload {
                    retry_builder = retry_builder.payload(p);
                }
                let retry_update = retry_builder.build().map_err(|e| {
                    DurableError::Internal(format!(
                        "Failed to build wait_for_condition RETRY update: {e}"
                    ))
                })?;
                self.inner
                    .execution_ctx
                    .checkpoint_manager
                    .checkpoint(step_id.clone(), retry_update)
                    .await?;

                self.inner
                    .execution_ctx
                    .termination_manager
                    .terminate_for_retry()
                    .await;

                std::future::pending::<()>().await;
                unreachable!()
            }
        }
    }
}
