use super::*;

mod replay;

impl DurableContextHandle {
    /// Invoke another durable Lambda function.
    ///
    /// Calls another Lambda function and waits for its result. The invocation
    /// is checkpointed, so on replay the cached result is returned without
    /// re-invoking the function.
    ///
    /// # Arguments
    ///
    /// * `name` - Optional name for tracking and debugging
    /// * `function_id` - Lambda function ARN or name
    /// * `input` - Optional input to pass to the function
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use lambda_durable_execution_rust::prelude::*;
    /// # use serde::{Deserialize, Serialize};
    /// # #[derive(Serialize)]
    /// # struct ProcessInput { data: &'static str }
    /// # #[derive(Deserialize)]
    /// # struct ProcessResult { ok: bool }
    /// # #[derive(Serialize)]
    /// # struct ValidationInput { value: u32 }
    /// # #[derive(Deserialize)]
    /// # struct ValidationResult { valid: bool }
    /// async fn example(ctx: DurableContextHandle) -> DurableResult<(ProcessResult, ValidationResult)> {
    ///     // Invoke by function name
    ///     let a: ProcessResult = ctx
    ///         .invoke(
    ///             Some("call-processor"),
    ///             "my-processor-function",
    ///             Some(ProcessInput { data: "hello" }),
    ///         )
    ///         .await?;
    ///
    ///     // Invoke by ARN
    ///     let b: ValidationResult = ctx
    ///         .invoke(
    ///             Some("validate"),
    ///             "arn:aws:lambda:us-east-1:123456789:function:validator",
    ///             Some(ValidationInput { value: 1 }),
    ///         )
    ///         .await?;
    ///     Ok((a, b))
    /// }
    /// ```
    pub async fn invoke<I, O>(
        &self,
        name: Option<&str>,
        function_id: &str,
        input: Option<I>,
    ) -> DurableResult<O>
    where
        I: Serialize + Send + Sync,
        O: DeserializeOwned + Send + Sync + 'static,
    {
        self.inner
            .invoke_with_config(name, function_id, input, None)
            .await
    }

    /// Invoke another durable Lambda function with custom configuration.
    ///
    /// Use this when you need custom Serdes for the input/result payloads.
    pub async fn invoke_with_config<I, O>(
        &self,
        name: Option<&str>,
        function_id: &str,
        input: Option<I>,
        config: Option<InvokeConfig<I, O>>,
    ) -> DurableResult<O>
    where
        I: Serialize + Send + Sync,
        O: DeserializeOwned + Send + Sync + 'static,
    {
        self.inner
            .invoke_with_config(name, function_id, input, config)
            .await
    }
}

impl DurableContextImpl {
    /// Invoke another Lambda function (legacy wrapper).
    pub async fn invoke<I, O>(
        &self,
        name: Option<&str>,
        function_id: &str,
        input: Option<I>,
    ) -> DurableResult<O>
    where
        I: Serialize + Send + Sync,
        O: DeserializeOwned + Send + Sync + 'static,
    {
        self.invoke_with_config(name, function_id, input, None)
            .await
    }

    /// Invoke another Lambda function with custom Serdes.
    pub async fn invoke_with_config<I, O>(
        &self,
        name: Option<&str>,
        function_id: &str,
        input: Option<I>,
        config: Option<InvokeConfig<I, O>>,
    ) -> DurableResult<O>
    where
        I: Serialize + Send + Sync,
        O: DeserializeOwned + Send + Sync + 'static,
    {
        let step_id = self.execution_ctx.next_operation_id(name);
        let hashed_id = Self::hash_id(&step_id);

        let payload_serdes = config.as_ref().and_then(|c| c.payload_serdes.clone());
        let result_serdes = config.as_ref().and_then(|c| c.result_serdes.clone());
        let tenant_id = config.as_ref().and_then(|c| c.tenant_id.clone());

        // Replay handling
        if let Some(result) = replay::handle_replay(
            self.execution_ctx.get_step_data(&hashed_id).await,
            result_serdes,
            &hashed_id,
            name,
            function_id,
            &self.execution_ctx,
        )
        .await?
        {
            return Ok(result);
        }

        // Serialize input using custom Serdes if provided.
        let input_payload = safe_serialize(
            payload_serdes,
            input.as_ref(),
            &hashed_id,
            name,
            &self.execution_ctx,
        )
        .await;

        // Checkpoint START for chained invoke
        let parent_id = self.execution_ctx.get_parent_id().await;
        let mut builder = OperationUpdate::builder()
            .id(&hashed_id)
            .operation_type(OperationType::ChainedInvoke)
            .sub_type("ChainedInvoke")
            .action(OperationAction::Start)
            .chained_invoke_options(ChainedInvokeUpdateOptions {
                function_name: function_id.to_string(),
                tenant_id,
            });

        if let Some(pid) = parent_id {
            builder = builder.parent_id(pid);
        }
        if let Some(n) = name {
            builder = builder.name(n);
        }
        if let Some(payload) = input_payload {
            builder = builder.payload(payload);
        }

        self.execution_ctx
            .checkpoint_manager
            .checkpoint(
                step_id.clone(),
                builder.build().map_err(|e| {
                    DurableError::Internal(format!(
                        "Failed to build chained invoke START update: {e}"
                    ))
                })?,
            )
            .await?;

        // Suspend so the service can perform the invoke.
        self.execution_ctx
            .termination_manager
            .terminate_for_invoke()
            .await;

        std::future::pending::<()>().await;
        unreachable!()
    }
}
