use super::*;

mod execute;
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

        execute::run_invoke_execution(
            self,
            name,
            function_id,
            input,
            payload_serdes,
            tenant_id,
            step_id,
            hashed_id,
        )
        .await
    }
}
