use super::super::*;
use super::replay;

pub(super) async fn wait_for_callback<T>(handle: CallbackHandle<T>) -> DurableResult<T>
where
    T: serde::de::DeserializeOwned + Send + Sync + 'static,
{
    if let Some(operation) = handle.execution_ctx.get_step_data(&handle.hashed_id).await {
        match operation.status {
            OperationStatus::Succeeded => {
                if let Some(details) = operation.callback_details.as_ref() {
                    if let Some(payload) = details.result.as_ref() {
                        if let Some(val) = safe_deserialize(
                            handle.serdes.clone(),
                            Some(payload.as_str()),
                            &handle.hashed_id,
                            Some(&handle.step_id),
                            &handle.execution_ctx,
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
                    name: handle.step_id,
                    message: error_msg,
                });
            }
            _ => {
                // Pending/started - suspend.
            }
        }
    }

    // Mark this operation as awaited before suspending
    handle
        .execution_ctx
        .checkpoint_manager
        .mark_awaited(&handle.hashed_id)
        .await;

    handle
        .execution_ctx
        .termination_manager
        .terminate_for_callback()
        .await;

    std::future::pending::<()>().await;
    unreachable!()
}

pub(super) async fn wait_for_callback_raw<T>(handle: CallbackHandle<T>) -> DurableResult<String> {
    if let Some(operation) = handle.execution_ctx.get_step_data(&handle.hashed_id).await {
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
                    name: handle.step_id,
                    message: error_msg,
                });
            }
            _ => {
                // Pending/started - suspend.
            }
        }
    }

    // Mark this operation as awaited before suspending
    handle
        .execution_ctx
        .checkpoint_manager
        .mark_awaited(&handle.hashed_id)
        .await;

    handle
        .execution_ctx
        .termination_manager
        .terminate_for_callback()
        .await;

    std::future::pending::<()>().await;
    unreachable!()
}

pub(super) async fn run_wait_for_callback<T, F, Fut>(
    ctx: &DurableContextImpl,
    name: Option<&str>,
    submitter: F,
    config: Option<CallbackConfig<T>>,
) -> DurableResult<T>
where
    T: DeserializeOwned + Send + Sync + 'static,
    F: FnOnce(String, StepContext) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send + 'static,
{
    let step_id = ctx.execution_ctx.next_operation_id(name);
    let hashed_id = DurableContextImpl::hash_id(&step_id);
    let config = config.unwrap_or_default();
    let serdes = config.serdes.clone();

    // Backwards compatibility: if an older Callback operation exists at this id,
    // return/await it directly.
    if let Some(result) = replay::handle_replay(
        ctx.execution_ctx.get_step_data(&hashed_id).await,
        serdes.clone(),
        &hashed_id,
        &step_id,
        name,
        &ctx.execution_ctx,
    )
    .await?
    {
        return Ok(result);
    }

    // Wrap callback creation + submitter in a child context.
    let submitter_retry = config.retry_strategy.clone();
    let callback_cfg_for_child = config.clone();

    let raw_payload: String = ctx
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
        &ctx.execution_ctx,
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
