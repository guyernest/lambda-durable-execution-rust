use super::super::*;

pub(super) enum ChildReplayDecision<T> {
    Return(T),
    ReplayChildren,
    Continue,
}

pub(super) async fn evaluate_replay<T>(
    operation: Option<crate::types::Operation>,
    serdes: Option<Arc<dyn Serdes<T>>>,
    hashed_id: &str,
    step_id: &str,
    name: Option<&str>,
    execution_ctx: &ExecutionContext,
) -> DurableResult<ChildReplayDecision<T>>
where
    T: DeserializeOwned + Send + Sync + 'static,
{
    let Some(operation) = operation else {
        return Ok(ChildReplayDecision::Continue);
    };

    match operation.status {
        OperationStatus::Succeeded => {
            if operation
                .context_details
                .as_ref()
                .and_then(|d| d.replay_children)
                == Some(true)
            {
                return Ok(ChildReplayDecision::ReplayChildren);
            }

            if let Some(ref details) = operation.context_details {
                if let Some(ref payload) = details.result {
                    if let Some(val) = safe_deserialize(
                        serdes.clone(),
                        Some(payload.as_str()),
                        hashed_id,
                        name,
                        execution_ctx,
                    )
                    .await
                    {
                        return Ok(ChildReplayDecision::Return(val));
                    }
                }
            }

            // Fallback for older payload locations.
            if let Some(ref details) = operation.execution_details {
                if let Some(ref payload) = details.output_payload {
                    if let Some(val) = safe_deserialize(
                        serdes,
                        Some(payload.as_str()),
                        hashed_id,
                        name,
                        execution_ctx,
                    )
                    .await
                    {
                        return Ok(ChildReplayDecision::Return(val));
                    }
                }
            }

            Ok(ChildReplayDecision::Continue)
        }
        OperationStatus::Failed => {
            let msg = operation
                .context_details
                .as_ref()
                .and_then(|d| d.error.as_ref())
                .map(|e| e.error_message.clone())
                .unwrap_or_else(|| "Child context failed".to_string());
            Err(DurableError::ChildContextFailed {
                name: step_id.to_string(),
                message: msg,
                source: None,
            })
        }
        _ => {
            execution_ctx.set_mode(ExecutionMode::Execution).await;
            Ok(ChildReplayDecision::Continue)
        }
    }
}
