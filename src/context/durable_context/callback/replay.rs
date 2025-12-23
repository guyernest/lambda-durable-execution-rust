use super::super::*;

pub(super) async fn handle_replay<T>(
    operation: Option<crate::types::Operation>,
    serdes: Option<Arc<dyn Serdes<T>>>,
    hashed_id: &str,
    step_id: &str,
    name: Option<&str>,
    execution_ctx: &ExecutionContext,
) -> DurableResult<Option<T>>
where
    T: DeserializeOwned + Send + Sync + 'static,
{
    let Some(operation) = operation else {
        return Ok(None);
    };

    if operation.operation_type != OperationType::Callback {
        return Ok(None);
    }

    match operation.status {
        OperationStatus::Succeeded => {
            if let Some(ref details) = operation.callback_details {
                if let Some(ref payload) = details.result {
                    if let Some(val) = safe_deserialize(
                        serdes,
                        Some(payload.as_str()),
                        hashed_id,
                        name,
                        execution_ctx,
                    )
                    .await
                    {
                        return Ok(Some(val));
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
                name: step_id.to_string(),
                message: error_msg,
            })
        }
        _ => {
            execution_ctx.set_mode(ExecutionMode::Execution).await;
            execution_ctx
                .termination_manager
                .terminate_for_callback()
                .await;
            std::future::pending::<()>().await;
            unreachable!()
        }
    }
}
