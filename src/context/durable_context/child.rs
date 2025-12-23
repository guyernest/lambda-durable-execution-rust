use super::*;

mod replay;

impl DurableContextHandle {
    /// Run operations in an isolated child context.
    ///
    /// Child contexts group related operations together, providing:
    ///
    /// - Hierarchical checkpoint organization
    /// - Scoped operation naming
    /// - Atomic completion of grouped operations
    ///
    /// # Arguments
    ///
    /// * `name` - Optional name for tracking and debugging
    /// * `context_fn` - Function receiving a new context handle for child operations
    /// * `config` - Optional configuration for the child context
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use lambda_durable_execution_rust::prelude::*;
    /// # async fn process_step_1() -> Result<u32, Box<dyn std::error::Error + Send + Sync>> { Ok(1) }
    /// # async fn process_step_2(_x: u32) -> Result<u32, Box<dyn std::error::Error + Send + Sync>> { Ok(2) }
    /// async fn example(ctx: DurableContextHandle) -> DurableResult<u32> {
    ///     // Group related operations
    ///     let processed: u32 = ctx
    ///         .run_in_child_context(
    ///             Some("batch-processing"),
    ///             |child_ctx| async move {
    ///                 let step1: u32 = child_ctx
    ///                     .step(
    ///                         Some("step-1"),
    ///                         |_| async { process_step_1().await },
    ///                         None,
    ///                     )
    ///                     .await?;
    ///
    ///                 let step2: u32 = child_ctx
    ///                     .step(
    ///                         Some("step-2"),
    ///                         move |_| async move { process_step_2(step1).await },
    ///                         None,
    ///                     )
    ///                     .await?;
    ///
    ///                 Ok(step2)
    ///             },
    ///             None,
    ///         )
    ///         .await?;
    ///     Ok(processed)
    /// }
    /// ```
    ///
    /// # Use Cases
    ///
    /// - **Batch processing**: Group items being processed together
    /// - **Transaction scopes**: Group operations that should complete atomically
    /// - **Modular workflows**: Organize complex workflows into logical units
    pub async fn run_in_child_context<T, F, Fut>(
        &self,
        name: Option<&str>,
        context_fn: F,
        config: Option<ChildContextConfig<T>>,
    ) -> DurableResult<T>
    where
        T: Serialize + DeserializeOwned + Send + Sync + 'static,
        F: FnOnce(DurableContextHandle) -> Fut + Send + 'static,
        Fut: Future<Output = DurableResult<T>> + Send + 'static,
    {
        self.inner
            .run_in_child_context(name, context_fn, config)
            .await
    }
}

impl DurableContextImpl {
    /// Run operations in a child context.
    pub async fn run_in_child_context<T, F, Fut>(
        &self,
        name: Option<&str>,
        context_fn: F,
        config: Option<ChildContextConfig<T>>,
    ) -> DurableResult<T>
    where
        T: Serialize + DeserializeOwned + Send + Sync + 'static,
        F: FnOnce(DurableContextHandle) -> Fut + Send + 'static,
        Fut: Future<Output = DurableResult<T>> + Send + 'static,
    {
        let step_id = self.execution_ctx.next_operation_id(name);
        let hashed_id = Self::hash_id(&step_id);

        self.run_in_child_context_with_ids(step_id, hashed_id, name, context_fn, config)
            .await
    }

    /// Run operations in a child context using pre-generated IDs.
    ///
    /// This is used internally for deterministic concurrent execution where the parent
    /// allocates child IDs sequentially before spawning tasks.
    pub async fn run_in_child_context_with_ids<T, F, Fut>(
        &self,
        step_id: String,
        hashed_id: String,
        name: Option<&str>,
        context_fn: F,
        config: Option<ChildContextConfig<T>>,
    ) -> DurableResult<T>
    where
        T: Serialize + DeserializeOwned + Send + Sync + 'static,
        F: FnOnce(DurableContextHandle) -> Fut + Send + 'static,
        Fut: Future<Output = DurableResult<T>> + Send + 'static,
    {
        let serdes = config.as_ref().and_then(|c| c.serdes.clone());
        let sub_type = config
            .as_ref()
            .and_then(|c| c.sub_type.clone())
            .unwrap_or_else(|| "RunInChildContext".to_string());

        // Check if already completed in replay
        match replay::evaluate_replay(
            self.execution_ctx.get_step_data(&hashed_id).await,
            serdes.clone(),
            &hashed_id,
            &step_id,
            name,
            &self.execution_ctx,
        )
        .await?
        {
            replay::ChildReplayDecision::Return(val) => return Ok(val),
            replay::ChildReplayDecision::ReplayChildren => {
                // ReplayChildren mode: reconstruct the result by re-running the child
                // context while reading child operation outputs from replay state.
                let child_execution_ctx = self.execution_ctx.with_parent_id(hashed_id.clone());
                let child_impl = Arc::new(DurableContextImpl::new(child_execution_ctx));
                let child_ctx = DurableContextHandle::new(child_impl);
                return context_fn(child_ctx).await;
            }
            replay::ChildReplayDecision::Continue => {}
        }

        // Checkpoint at start if not already started. This ensures any child operations that
        // reference `ParentId` (this context) are valid to the backend.
        if self.execution_ctx.get_step_data(&hashed_id).await.is_none() {
            let parent_id = self.execution_ctx.get_parent_id().await;
            let mut builder = OperationUpdate::builder()
                .id(&hashed_id)
                .operation_type(OperationType::Context)
                .sub_type(sub_type.clone())
                .action(OperationAction::Start);

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
                        DurableError::Internal(format!(
                            "Failed to build child context START update: {e}"
                        ))
                    })?,
                )
                .await?;
        }

        // Create child context
        let child_execution_ctx = self.execution_ctx.with_parent_id(hashed_id.clone());
        let child_impl = Arc::new(DurableContextImpl::new(child_execution_ctx));
        let child_ctx = DurableContextHandle::new(child_impl);

        // Execute child context
        let result = match context_fn(child_ctx).await {
            Ok(val) => val,
            Err(error) => {
                let err_obj = ErrorObject::from_durable_error(&error);
                let parent_id = self.execution_ctx.get_parent_id().await;

                let mut builder = OperationUpdate::builder()
                    .id(&hashed_id)
                    .operation_type(OperationType::Context)
                    .sub_type(sub_type)
                    .action(OperationAction::Fail)
                    .error(err_obj.clone());

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
                            DurableError::Internal(format!(
                                "Failed to build child context FAIL update: {e}"
                            ))
                        })?,
                    )
                    .await?;

                return Err(DurableError::ChildContextFailed {
                    name: step_id,
                    message: err_obj.error_message,
                    source: Some(Arc::new(Box::new(error))),
                });
            }
        };

        // Checkpoint child context completion
        let mut payload =
            safe_serialize(serdes, Some(&result), &hashed_id, name, &self.execution_ctx).await;
        let mut replay_children = false;
        if let Some(ref p) = payload {
            if p.len() > CHECKPOINT_SIZE_LIMIT_BYTES {
                replay_children = true;
                payload = Some(String::new());
            }
        }

        let parent_id = self.execution_ctx.get_parent_id().await;
        let mut builder = OperationUpdate::builder()
            .id(&hashed_id)
            .operation_type(OperationType::Context)
            .sub_type(sub_type)
            .action(OperationAction::Succeed);

        if replay_children {
            builder = builder.context_options(ContextUpdateOptions {
                replay_children: Some(true),
            });
        }

        if let Some(p) = payload {
            builder = builder.payload(p);
        }

        if let Some(pid) = parent_id {
            builder = builder.parent_id(pid);
        }
        if let Some(n) = name {
            builder = builder.name(n);
        }
        self.execution_ctx
            .checkpoint_manager
            .checkpoint(
                step_id,
                builder.build().map_err(|e| {
                    DurableError::Internal(format!(
                        "Failed to build child context completion update: {e}"
                    ))
                })?,
            )
            .await?;

        Ok(result)
    }
}
