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

    match operation.status {
        OperationStatus::Succeeded => {
            if let Some(details) = operation.step_details {
                if let Some(payload) = details.result {
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
                "Missing wait-for-condition result in replay".to_string(),
            ))
        }
        OperationStatus::Failed => {
            let attempts = operation
                .step_details
                .as_ref()
                .and_then(|d| d.attempt)
                .unwrap_or(1);
            let msg = operation
                .step_details
                .as_ref()
                .and_then(|d| d.error.as_ref())
                .map(|e| e.error_message.clone())
                .unwrap_or_else(|| "Wait for condition failed".to_string());
            Err(DurableError::step_failed_msg(
                step_id.to_string(),
                attempts,
                msg,
            ))
        }
        _ => {
            execution_ctx.set_mode(ExecutionMode::Execution).await;
            Ok(None)
        }
    }
}
