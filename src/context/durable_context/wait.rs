use super::*;

mod execute;

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

        execute::run_wait(self, name, duration, step_id, hashed_id).await
    }
}
