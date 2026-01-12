use super::super::*;

pub(super) async fn handle_replay<O>(
    operation: Option<crate::types::Operation>,
    result_serdes: Option<Arc<dyn Serdes<O>>>,
    hashed_id: &str,
    name: Option<&str>,
    function_id: &str,
    execution_ctx: &ExecutionContext,
) -> DurableResult<Option<O>>
where
    O: DeserializeOwned + Send + Sync + 'static,
{
    let Some(operation) = operation else {
        return Ok(None);
    };

    match operation.status {
        OperationStatus::Succeeded => {
            if let Some(ref details) = operation.chained_invoke_details {
                if let Some(ref payload) = details.result {
                    let val = safe_deserialize(
                        result_serdes,
                        Some(payload.as_str()),
                        hashed_id,
                        name,
                        execution_ctx,
                    )
                    .await
                    .ok_or_else(|| {
                        DurableError::Internal("Missing invoke result in replay".to_string())
                    })?;
                    return Ok(Some(val));
                }
            }
            Err(DurableError::Internal(
                "Missing invoke result in replay".to_string(),
            ))
        }
        OperationStatus::Failed => {
            let msg = operation
                .chained_invoke_details
                .as_ref()
                .and_then(|d| d.error.as_ref())
                .map(|e| e.error_message.clone())
                .unwrap_or_else(|| "Invoke failed".to_string());
            Err(DurableError::InvocationFailed {
                function: function_id.to_string(),
                message: msg,
                source: None,
            })
        }
        _ => {
            // The backend can complete a chained invoke between the time the invocation state is
            // captured and when this replay executes. If we immediately suspend based on stale
            // `Started/Pending` state, the service can reject the invocation with:
            // "Cannot return PENDING status with no pending operations."
            //
            // Mirror the JS/Python SDK behavior by forcing a checkpoint refresh before
            // suspending, then re-checking the latest operation status.
            if let Err(e) = execution_ctx.checkpoint_manager.force_checkpoint().await {
                tracing::warn!(
                    "Failed to force checkpoint refresh while replaying invoke {hashed_id}: {e}"
                );
            }

            if let Some(refreshed) = execution_ctx.get_step_data(hashed_id).await {
                match refreshed.status {
                    OperationStatus::Succeeded => {
                        if let Some(ref details) = refreshed.chained_invoke_details {
                            if let Some(ref payload) = details.result {
                                let val = safe_deserialize(
                                    result_serdes,
                                    Some(payload.as_str()),
                                    hashed_id,
                                    name,
                                    execution_ctx,
                                )
                                .await
                                .ok_or_else(|| {
                                    DurableError::Internal(
                                        "Missing invoke result in replay".to_string(),
                                    )
                                })?;
                                return Ok(Some(val));
                            }
                        }
                        return Err(DurableError::Internal(
                            "Missing invoke result in replay".to_string(),
                        ));
                    }
                    OperationStatus::Failed => {
                        let msg = refreshed
                            .chained_invoke_details
                            .as_ref()
                            .and_then(|d| d.error.as_ref())
                            .map(|e| e.error_message.clone())
                            .unwrap_or_else(|| "Invoke failed".to_string());
                        return Err(DurableError::InvocationFailed {
                            function: function_id.to_string(),
                            message: msg,
                            source: None,
                        });
                    }
                    _ => {}
                }
            }

            execution_ctx.set_mode(ExecutionMode::Execution).await;
            execution_ctx
                .termination_manager
                .terminate_for_invoke()
                .await;
            std::future::pending::<()>().await;
            unreachable!()
        }
    }
}
