use super::super::*;

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_invoke_execution<I, O>(
    ctx: &DurableContextImpl,
    name: Option<&str>,
    function_id: &str,
    input: Option<I>,
    payload_serdes: Option<Arc<dyn Serdes<I>>>,
    tenant_id: Option<String>,
    step_id: String,
    hashed_id: String,
) -> DurableResult<O>
where
    I: Serialize + Send + Sync,
    O: DeserializeOwned + Send + Sync + 'static,
{
    // Serialize input using custom Serdes if provided.
    let input_payload = safe_serialize(
        payload_serdes,
        input.as_ref(),
        &hashed_id,
        name,
        &ctx.execution_ctx,
    )
    .await;

    // Checkpoint START for chained invoke
    let parent_id = ctx.execution_ctx.get_parent_id().await;
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

    ctx.execution_ctx
        .checkpoint_manager
        .checkpoint(
            step_id.clone(),
            builder.build().map_err(|e| {
                DurableError::Internal(format!("Failed to build chained invoke START update: {e}"))
            })?,
        )
        .await?;

    // Suspend so the service can perform the invoke.
    ctx.execution_ctx
        .termination_manager
        .terminate_for_invoke()
        .await;

    std::future::pending::<()>().await;
    unreachable!()
}
