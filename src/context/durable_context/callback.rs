use super::*;

impl<T> CallbackHandle<T> {
    /// Get the callback ID to provide to external systems.
    ///
    /// The external system should use this ID when completing the callback
    /// via the AWS Lambda CompleteCallback API.
    pub fn callback_id(&self) -> &str {
        &self.callback_id
    }

    /// Wait for the callback to be completed by an external system.
    ///
    /// This suspends the Lambda function until the callback is completed.
    /// The external system must call the AWS Lambda CompleteCallback API
    /// with the callback ID.
    pub async fn wait(self) -> DurableResult<T>
    where
        T: serde::de::DeserializeOwned + Send + Sync + 'static,
    {
        if let Some(operation) = self.execution_ctx.get_step_data(&self.hashed_id).await {
            match operation.status {
                OperationStatus::Succeeded => {
                    if let Some(details) = operation.callback_details.as_ref() {
                        if let Some(payload) = details.result.as_ref() {
                            if let Some(val) = safe_deserialize(
                                self.serdes.clone(),
                                Some(payload.as_str()),
                                &self.hashed_id,
                                Some(&self.step_id),
                                &self.execution_ctx,
                            )
                            .await
                            {
                                return Ok(val);
                            }
                        }
                    }
                    return Err(DurableError::Internal(
                        "Missing callback result in replay".to_string(),
                    ));
                }
                OperationStatus::Failed => {
                    let error_msg = operation
                        .callback_details
                        .as_ref()
                        .and_then(|d| d.error.as_ref())
                        .map(|e| e.error_message.clone())
                        .unwrap_or_else(|| "Unknown error".to_string());
                    return Err(DurableError::CallbackFailed {
                        name: self.step_id,
                        message: error_msg,
                    });
                }
                _ => {
                    // Pending/started - suspend.
                }
            }
        }

        // Mark this operation as awaited before suspending
        self.execution_ctx
            .checkpoint_manager
            .mark_awaited(&self.hashed_id)
            .await;

        self.execution_ctx
            .termination_manager
            .terminate_for_callback()
            .await;

        std::future::pending::<()>().await;
        unreachable!()
    }

    /// Wait for the callback to complete and return the raw payload string.
    ///
    /// This is primarily used internally by `wait_for_callback` to mirror the
    /// JS SDK two-phase behavior (child context returns a string; parent
    /// deserializes with custom Serdes).
    pub async fn wait_raw(self) -> DurableResult<String> {
        if let Some(operation) = self.execution_ctx.get_step_data(&self.hashed_id).await {
            match operation.status {
                OperationStatus::Succeeded => {
                    if let Some(details) = operation.callback_details.as_ref() {
                        if let Some(payload) = details.result.as_ref() {
                            return Ok(payload.clone());
                        }
                    }
                    return Err(DurableError::Internal(
                        "Missing callback result in replay".to_string(),
                    ));
                }
                OperationStatus::Failed => {
                    let error_msg = operation
                        .callback_details
                        .as_ref()
                        .and_then(|d| d.error.as_ref())
                        .map(|e| e.error_message.clone())
                        .unwrap_or_else(|| "Unknown error".to_string());
                    return Err(DurableError::CallbackFailed {
                        name: self.step_id,
                        message: error_msg,
                    });
                }
                _ => {
                    // Pending/started - suspend.
                }
            }
        }

        // Mark this operation as awaited before suspending
        self.execution_ctx
            .checkpoint_manager
            .mark_awaited(&self.hashed_id)
            .await;

        self.execution_ctx
            .termination_manager
            .terminate_for_callback()
            .await;

        std::future::pending::<()>().await;
        unreachable!()
    }
}

impl<T> std::fmt::Debug for CallbackHandle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallbackHandle")
            .field("callback_id", &self.callback_id)
            .field("step_id", &self.step_id)
            .field("hashed_id", &self.hashed_id)
            .field("serdes", &self.serdes.is_some())
            .finish()
    }
}

impl DurableContextHandle {
    /// Wait for an external system to complete a callback.
    ///
    /// This is the primary way to integrate with external systems that need
    /// to signal completion asynchronously. Common use cases include:
    ///
    /// - Human approval workflows
    /// - Webhook-based integrations
    /// - Long-running external jobs
    /// - Payment confirmations
    ///
    /// # Arguments
    ///
    /// * `name` - Optional name for tracking and debugging
    /// * `submitter` - Function called with the callback ID to notify external system
    /// * `config` - Optional configuration including timeout
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use lambda_durable_execution_rust::prelude::*;
    /// # use serde::Deserialize;
    /// # #[derive(Deserialize)]
    /// # struct ApprovalDecision { approved: bool }
    /// # async fn send_approval_email(
    /// #     _callback_id: &str,
    /// #     _approver_email: &str,
    /// # ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    /// #     Ok(())
    /// # }
    /// async fn example(ctx: DurableContextHandle) -> DurableResult<()> {
    ///     let approver_email = "approver@example.com".to_string();
    ///
    ///     let approval: ApprovalDecision = ctx
    ///         .wait_for_callback(
    ///             Some("await-approval"),
    ///             |callback_id, step_ctx| async move {
    ///                 step_ctx.info(&format!("Callback ID: {}", callback_id));
    ///                 send_approval_email(&callback_id, &approver_email).await
    ///             },
    ///             Some(CallbackConfig::new().with_timeout(Duration::hours(24))),
    ///         )
    ///         .await?;
    ///
    ///     if approval.approved {
    ///         // Continue with approved action
    ///     }
    ///     Ok(())
    /// }
    /// ```
    ///
    /// # How It Works
    ///
    /// 1. The SDK generates a unique callback ID
    /// 2. Your submitter function is called with this ID
    /// 3. Lambda suspends (no compute cost while waiting)
    /// 4. External system calls AWS Lambda CompleteCallback API with the ID
    /// 5. Lambda resumes and returns the result
    pub async fn wait_for_callback<T, F, Fut>(
        &self,
        name: Option<&str>,
        submitter: F,
        config: Option<CallbackConfig<T>>,
    ) -> DurableResult<T>
    where
        T: DeserializeOwned + Send + Sync + 'static,
        F: FnOnce(String, StepContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send + 'static,
    {
        self.inner.wait_for_callback(name, submitter, config).await
    }

    /// Create a callback handle for external systems.
    ///
    /// Use this when you need more control over the callback workflow than
    /// [`wait_for_callback`](Self::wait_for_callback) provides. This creates
    /// a callback handle immediately, allowing you to:
    ///
    /// - Send the callback ID before starting to wait
    /// - Perform other operations between creating and waiting
    /// - Implement custom waiting logic
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use lambda_durable_execution_rust::prelude::*;
    /// # use serde::Deserialize;
    /// # #[derive(Deserialize)]
    /// # struct PaymentResult { ok: bool }
    /// # async fn initiate_payment(
    /// #     _callback_id: &str,
    /// #     _amount: u32,
    /// # ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> { Ok(()) }
    /// # async fn prepare_order_for_fulfillment() -> Result<(), Box<dyn std::error::Error + Send + Sync>> { Ok(()) }
    /// async fn example(ctx: DurableContextHandle) -> DurableResult<PaymentResult> {
    ///     let amount = 100u32;
    ///
    ///     // Create callback handle
    ///     let handle: CallbackHandle<PaymentResult> =
    ///         ctx.create_callback(Some("payment-callback"), None).await?;
    ///
    ///     // Send callback ID to payment processor
    ///     initiate_payment(handle.callback_id(), amount)
    ///         .await
    ///         .map_err(|e| DurableError::step_failed_msg("initiate-payment", 1, e.to_string()))?;
    ///
    ///     // Do other work while payment processes
    ///     ctx.step(
    ///         Some("prepare-fulfillment"),
    ///         |_| async move { prepare_order_for_fulfillment().await },
    ///         None,
    ///     )
    ///     .await?;
    ///
    ///     // Wait for payment completion
    ///     let result = handle.wait().await?;
    ///     Ok(result)
    /// }
    /// ```
    pub async fn create_callback<T>(
        &self,
        name: Option<&str>,
        config: Option<CallbackConfig<T>>,
    ) -> DurableResult<CallbackHandle<T>>
    where
        T: DeserializeOwned + Send + Sync + 'static,
    {
        let step_id = self.inner.execution_ctx.next_operation_id(name);
        let hashed_id = DurableContextImpl::hash_id(&step_id);
        let serdes = config.as_ref().and_then(|c| c.serdes.clone());

        // If this callback hasn't been started yet, checkpoint a START operation.
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
                .operation_type(OperationType::Callback)
                .sub_type("Callback")
                .action(OperationAction::Start);

            if let Some(pid) = parent_id {
                builder = builder.parent_id(pid);
            }
            if let Some(n) = name {
                builder = builder.name(n);
            }
            if let Some(ref cfg) = config {
                let cb_options = crate::types::CallbackUpdateOptions {
                    timeout_seconds: cfg.timeout.map(|d| d.to_seconds_i32_saturating()),
                    heartbeat_timeout_seconds: cfg
                        .heartbeat_timeout
                        .map(|d| d.to_seconds_i32_saturating()),
                };
                builder = builder.callback_options(cb_options);
            }

            self.inner
                .execution_ctx
                .checkpoint_manager
                .checkpoint(
                    step_id.clone(),
                    builder.build().map_err(|e| {
                        DurableError::Internal(format!(
                            "Failed to build callback START update: {e}",
                        ))
                    })?,
                )
                .await?;
        }

        // Try to use the service-generated callback id if available.
        let callback_id = self
            .inner
            .execution_ctx
            .get_step_data(&hashed_id)
            .await
            .and_then(|op| op.callback_details.and_then(|d| d.callback_id))
            .unwrap_or_else(|| {
                format!(
                    "{}:{}",
                    self.inner.execution_ctx.durable_execution_arn, hashed_id
                )
            });

        Ok(CallbackHandle {
            callback_id,
            step_id,
            hashed_id,
            execution_ctx: self.inner.execution_ctx.clone(),
            serdes,
            _phantom: std::marker::PhantomData,
        })
    }
}

impl DurableContextImpl {
    /// Wait for an external callback.
    pub async fn wait_for_callback<T, F, Fut>(
        &self,
        name: Option<&str>,
        submitter: F,
        config: Option<CallbackConfig<T>>,
    ) -> DurableResult<T>
    where
        T: DeserializeOwned + Send + Sync + 'static,
        F: FnOnce(String, StepContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send + 'static,
    {
        let step_id = self.execution_ctx.next_operation_id(name);
        let hashed_id = Self::hash_id(&step_id);
        let config = config.unwrap_or_default();
        let serdes = config.serdes.clone();

        // Backwards compatibility: if an older Callback operation exists at this id,
        // return/await it directly.
        if let Some(operation) = self.execution_ctx.get_step_data(&hashed_id).await {
            if operation.operation_type == OperationType::Callback {
                return match operation.status {
                    OperationStatus::Succeeded => {
                        if let Some(ref details) = operation.callback_details {
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
                        Err(DurableError::Internal(
                            "Missing callback result in replay".to_string(),
                        ))
                    }
                    OperationStatus::Failed => {
                        let error_msg = operation
                            .callback_details
                            .as_ref()
                            .and_then(|d| d.error.as_ref())
                            .map(|e| e.error_message.clone())
                            .unwrap_or_else(|| "Unknown error".to_string());
                        Err(DurableError::CallbackFailed {
                            name: step_id,
                            message: error_msg,
                        })
                    }
                    _ => {
                        // Pending/started - suspend.
                        self.execution_ctx.set_mode(ExecutionMode::Execution).await;
                        self.execution_ctx
                            .termination_manager
                            .terminate_for_callback()
                            .await;
                        std::future::pending::<()>().await;
                        unreachable!()
                    }
                };
            }
        }

        // Wrap callback creation + submitter in a child context.
        let submitter_retry = config.retry_strategy.clone();
        let callback_cfg_for_child = config.clone();

        let raw_payload: String = self
            .run_in_child_context_with_ids(
                step_id.clone(),
                hashed_id.clone(),
                name,
                move |child_ctx| async move {
                    let handle: CallbackHandle<T> = child_ctx
                        .create_callback(None, Some(callback_cfg_for_child))
                        .await?;
                    let callback_id = handle.callback_id().to_string();

                    let step_cfg = submitter_retry
                        .clone()
                        .map(|s| StepConfig::<()>::new().with_retry_strategy(s));

                    child_ctx
                        .step(
                            Some("submitter"),
                            move |step_ctx| async move {
                                submitter(callback_id, step_ctx).await?;
                                Ok(())
                            },
                            step_cfg,
                        )
                        .await?;

                    handle.wait_raw().await
                },
                Some(ChildContextConfig::<String> {
                    sub_type: Some("WaitForCallback".to_string()),
                    ..Default::default()
                }),
            )
            .await?;

        if let Some(val) = safe_deserialize(
            serdes,
            Some(raw_payload.as_str()),
            &hashed_id,
            name,
            &self.execution_ctx,
        )
        .await
        {
            Ok(val)
        } else {
            Err(DurableError::Internal(
                "Missing callback result after wait".to_string(),
            ))
        }
    }
}
