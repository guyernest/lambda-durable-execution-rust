use super::*;

impl DurableContextHandle {
    /// Wait for a specified duration.
    ///
    /// The Lambda function suspends during the wait, so you don't pay for
    /// compute time while waiting. This is ideal for:
    ///
    /// - Implementing delays between operations
    /// - Rate limiting
    /// - Waiting for external processes to complete
    /// - Scheduled tasks
    ///
    /// # Arguments
    ///
    /// * `name` - Optional name for tracking and debugging
    /// * `duration` - How long to wait
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use lambda_durable_execution_rust::prelude::*;
    /// async fn example(ctx: DurableContextHandle) -> DurableResult<()> {
    ///     // Wait 5 minutes between operations
    ///     ctx.wait(Some("rate-limit-delay"), Duration::minutes(5)).await?;
    ///
    ///     // Wait 1 hour for a scheduled task
    ///     ctx.wait(Some("scheduled-wait"), Duration::hours(1)).await?;
    ///     Ok(())
    /// }
    /// ```
    ///
    /// # Cost Efficiency
    ///
    /// Unlike `tokio::time::sleep`, this wait is "free" in terms of compute cost.
    /// The Lambda function returns control to AWS, which will re-invoke it after
    /// the specified duration. You only pay for the brief execution time before
    /// and after the wait.
    pub async fn wait(&self, name: Option<&str>, duration: Duration) -> DurableResult<()> {
        self.inner.wait(name, duration).await
    }
}

impl DurableContextImpl {
    /// Wait for a duration.
    pub async fn wait(&self, name: Option<&str>, duration: Duration) -> DurableResult<()> {
        let step_id = self.execution_ctx.next_operation_id(name);
        let hashed_id = Self::hash_id(&step_id);

        // If the requested duration is zero, avoid scheduling/suspending.
        // Returning PENDING with no pending ops is invalid and can happen if the wait completes immediately.
        if duration.is_zero() {
            return Ok(());
        }

        // Replay handling: if the wait already exists, never re-start it.
        if let Some(operation) = self.execution_ctx.get_step_data(&hashed_id).await {
            match operation.status {
                OperationStatus::Succeeded => return Ok(()),
                OperationStatus::Failed => {
                    return Err(DurableError::Internal("Wait failed in replay".to_string()))
                }
                _ => {
                    self.execution_ctx.set_mode(ExecutionMode::Execution).await;
                    self.execution_ctx
                        .termination_manager
                        .terminate_for_wait()
                        .await;
                    std::future::pending::<()>().await;
                    unreachable!()
                }
            }
        }

        // Checkpoint the wait
        let parent_id = self.execution_ctx.get_parent_id().await;
        let mut builder = OperationUpdate::builder()
            .id(&hashed_id)
            .operation_type(OperationType::Wait)
            .sub_type("Wait")
            .action(OperationAction::Start)
            .wait_options(crate::types::WaitUpdateOptions {
                wait_seconds: Some(duration.to_seconds_i32_saturating()),
            });

        if let Some(pid) = parent_id {
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
                    DurableError::Internal(format!("Failed to build wait START update: {e}"))
                })?,
            )
            .await?;

        // The backend can resolve short waits quickly; re-check after the START checkpoint
        // to avoid suspending when there is nothing left pending.
        if let Some(operation) = self.execution_ctx.get_step_data(&hashed_id).await {
            if operation.status == OperationStatus::Succeeded {
                return Ok(());
            }
            if operation.status == OperationStatus::Failed {
                return Err(DurableError::Internal(
                    "Wait failed immediately after checkpoint".to_string(),
                ));
            }
        }

        // Mark this operation as awaited before suspending
        self.execution_ctx
            .checkpoint_manager
            .mark_awaited(&hashed_id)
            .await;

        // Trigger termination for wait
        self.execution_ctx
            .termination_manager
            .terminate_for_wait()
            .await;

        // This point is never reached during normal execution
        std::future::pending::<()>().await;
        unreachable!()
    }
}
