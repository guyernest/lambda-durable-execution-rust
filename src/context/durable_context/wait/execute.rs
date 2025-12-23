use super::super::*;

pub(super) async fn run_wait(
    ctx: &DurableContextImpl,
    name: Option<&str>,
    duration: Duration,
    step_id: String,
    hashed_id: String,
) -> DurableResult<()> {
    // If the requested duration is zero, avoid scheduling/suspending.
    // Returning PENDING with no pending ops is invalid and can happen if the wait completes immediately.
    if duration.is_zero() {
        return Ok(());
    }

    // Replay handling: if the wait already exists, never re-start it.
    if let Some(operation) = ctx.execution_ctx.get_step_data(&hashed_id).await {
        match operation.status {
            OperationStatus::Succeeded => return Ok(()),
            OperationStatus::Failed => {
                return Err(DurableError::Internal("Wait failed in replay".to_string()))
            }
            _ => {
                ctx.execution_ctx.set_mode(ExecutionMode::Execution).await;
                ctx.execution_ctx
                    .termination_manager
                    .terminate_for_wait()
                    .await;
                std::future::pending::<()>().await;
                unreachable!()
            }
        }
    }

    // Checkpoint the wait
    let parent_id = ctx.execution_ctx.get_parent_id().await;
    let mut builder = OperationUpdate::builder()
        .id(&hashed_id)
        .operation_type(OperationType::Wait)
        .sub_type("Wait")
        .action(OperationAction::Start)
        .wait_options(crate::types::WaitUpdateOptions {
            wait_seconds: Some(duration.to_seconds_i32_saturating()),
        });

    if let Some(pid) = parent_id {
        builder = builder.parent_id(pid);
    }
    if let Some(n) = name {
        builder = builder.name(n);
    }

    ctx.execution_ctx
        .checkpoint_manager
        .checkpoint(
            step_id.clone(),
            builder.build().map_err(|e| {
                DurableError::Internal(format!("Failed to build wait START update: {e}"))
            })?,
        )
        .await?;

    // The backend can resolve short waits quickly; re-check after the START checkpoint
    // to avoid suspending when there is nothing left pending.
    if let Some(operation) = ctx.execution_ctx.get_step_data(&hashed_id).await {
        if operation.status == OperationStatus::Succeeded {
            return Ok(());
        }
        if operation.status == OperationStatus::Failed {
            return Err(DurableError::Internal(
                "Wait failed immediately after checkpoint".to_string(),
            ));
        }
    }

    // Mark this operation as awaited before suspending
    ctx.execution_ctx
        .checkpoint_manager
        .mark_awaited(&hashed_id)
        .await;

    // Trigger termination for wait
    ctx.execution_ctx
        .termination_manager
        .terminate_for_wait()
        .await;

    // This point is never reached during normal execution
    std::future::pending::<()>().await;
    unreachable!()
}
