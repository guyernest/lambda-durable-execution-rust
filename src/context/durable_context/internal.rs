use crate::context::ExecutionContext;
use crate::error::{DurableError, DurableResult};
use crate::types::{BatchCompletionReason, Serdes, SerdesContext};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::sync::Arc;

pub(super) async fn safe_serialize<T>(
    serdes: Option<Arc<dyn Serdes<T>>>,
    value: Option<&T>,
    entity_id: &str,
    name: Option<&str>,
    execution_ctx: &ExecutionContext,
) -> Option<String>
where
    T: Serialize + Send + Sync,
{
    if let Some(serdes) = serdes {
        match serdes
            .serialize(
                value,
                SerdesContext {
                    entity_id: entity_id.to_string(),
                    durable_execution_arn: execution_ctx.durable_execution_arn.clone(),
                },
            )
            .await
        {
            Ok(v) => v,
            Err(e) => {
                let msg = format!(
                    "Serialization failed for {}({}): {}",
                    name.unwrap_or("operation"),
                    entity_id,
                    e
                );
                execution_ctx
                    .termination_manager
                    .terminate_for_serdes_failure(msg)
                    .await;
                std::future::pending::<()>().await;
                unreachable!()
            }
        }
    } else {
        match value {
            Some(v) => match serde_json::to_string(v) {
                Ok(s) => Some(s),
                Err(e) => {
                    let msg = format!(
                        "Serialization failed for {}({}): {}",
                        name.unwrap_or("operation"),
                        entity_id,
                        e
                    );
                    execution_ctx
                        .termination_manager
                        .terminate_for_serdes_failure(msg)
                        .await;
                    std::future::pending::<()>().await;
                    unreachable!()
                }
            },
            None => None,
        }
    }
}

pub(super) async fn safe_deserialize<T>(
    serdes: Option<Arc<dyn Serdes<T>>>,
    data: Option<&str>,
    entity_id: &str,
    name: Option<&str>,
    execution_ctx: &ExecutionContext,
) -> Option<T>
where
    T: DeserializeOwned + Send + Sync + 'static,
{
    if let Some(serdes) = serdes {
        match serdes
            .deserialize(
                data,
                SerdesContext {
                    entity_id: entity_id.to_string(),
                    durable_execution_arn: execution_ctx.durable_execution_arn.clone(),
                },
            )
            .await
        {
            Ok(v) => v,
            Err(e) => {
                let msg = format!(
                    "Deserialization failed for {}({}): {}",
                    name.unwrap_or("operation"),
                    entity_id,
                    e
                );
                execution_ctx
                    .termination_manager
                    .terminate_for_serdes_failure(msg)
                    .await;
                std::future::pending::<()>().await;
                unreachable!()
            }
        }
    } else {
        match data {
            Some(d) => match serde_json::from_str::<T>(d) {
                Ok(v) => Some(v),
                Err(e) => {
                    let msg = format!(
                        "Deserialization failed for {}({}): {}",
                        name.unwrap_or("operation"),
                        entity_id,
                        e
                    );
                    execution_ctx
                        .termination_manager
                        .terminate_for_serdes_failure(msg)
                        .await;
                    std::future::pending::<()>().await;
                    unreachable!()
                }
            },
            None => None,
        }
    }
}

pub(super) async fn safe_serialize_required_with_serdes<T>(
    serdes: Arc<dyn Serdes<T>>,
    value: &T,
    entity_id: &str,
    name: Option<&str>,
    execution_ctx: &ExecutionContext,
) -> String
where
    T: Send + Sync,
{
    match serdes
        .serialize(
            Some(value),
            SerdesContext {
                entity_id: entity_id.to_string(),
                durable_execution_arn: execution_ctx.durable_execution_arn.clone(),
            },
        )
        .await
    {
        Ok(Some(v)) => v,
        Ok(None) => {
            let msg = format!(
                "Serialization returned None for {}({})",
                name.unwrap_or("operation"),
                entity_id
            );
            execution_ctx
                .termination_manager
                .terminate_for_serdes_failure(msg)
                .await;
            std::future::pending::<()>().await;
            unreachable!()
        }
        Err(e) => {
            let msg = format!(
                "Serialization failed for {}({}): {}",
                name.unwrap_or("operation"),
                entity_id,
                e
            );
            execution_ctx
                .termination_manager
                .terminate_for_serdes_failure(msg)
                .await;
            std::future::pending::<()>().await;
            unreachable!()
        }
    }
}

pub(super) async fn safe_deserialize_required_with_serdes<T>(
    serdes: Arc<dyn Serdes<T>>,
    data: &str,
    entity_id: &str,
    name: Option<&str>,
    execution_ctx: &ExecutionContext,
) -> T
where
    T: Send + Sync,
{
    match serdes
        .deserialize(
            Some(data),
            SerdesContext {
                entity_id: entity_id.to_string(),
                durable_execution_arn: execution_ctx.durable_execution_arn.clone(),
            },
        )
        .await
    {
        Ok(Some(v)) => v,
        Ok(None) => {
            let msg = format!(
                "Deserialization returned None for {}({})",
                name.unwrap_or("operation"),
                entity_id
            );
            execution_ctx
                .termination_manager
                .terminate_for_serdes_failure(msg)
                .await;
            std::future::pending::<()>().await;
            unreachable!()
        }
        Err(e) => {
            let msg = format!(
                "Deserialization failed for {}({}): {}",
                name.unwrap_or("operation"),
                entity_id,
                e
            );
            execution_ctx
                .termination_manager
                .terminate_for_serdes_failure(msg)
                .await;
            std::future::pending::<()>().await;
            unreachable!()
        }
    }
}

pub(super) fn validate_completion_config(
    config: &crate::types::CompletionConfig,
    total_items: usize,
    operation_name: &str,
) -> DurableResult<()> {
    if let Some(min) = config.min_successful {
        if min > total_items {
            return Err(DurableError::InvalidConfiguration {
                message: format!(
                    "{operation_name}: min_successful ({min}) exceeds total items ({total_items})",
                ),
            });
        }
    }

    if let Some(tol) = config.tolerated_failure_count {
        if tol > total_items {
            return Err(DurableError::InvalidConfiguration {
                message: format!(
                    "{operation_name}: tolerated_failure_count ({tol}) exceeds total items ({total_items})",
                ),
            });
        }
    }

    if let Some(pct) = config.tolerated_failure_percentage {
        if !pct.is_finite() || !(0.0..=100.0).contains(&pct) {
            return Err(DurableError::InvalidConfiguration {
                message: format!(
                    "{operation_name}: tolerated_failure_percentage ({pct}) must be finite and between 0 and 100",
                ),
            });
        }
    }

    Ok(())
}

pub(super) fn has_completion_criteria(config: &crate::types::CompletionConfig) -> bool {
    config.min_successful.is_some()
        || config.tolerated_failure_count.is_some()
        || config.tolerated_failure_percentage.is_some()
}

pub(super) fn should_continue_batch(
    failure_count: usize,
    total_items: usize,
    config: &crate::types::CompletionConfig,
) -> bool {
    if !has_completion_criteria(config) {
        return failure_count == 0;
    }
    if let Some(tol) = config.tolerated_failure_count {
        if failure_count > tol {
            return false;
        }
    }
    if let Some(pct) = config.tolerated_failure_percentage {
        if total_items > 0 {
            let failure_pct = (failure_count as f64 / total_items as f64) * 100.0;
            if failure_pct > pct {
                return false;
            }
        }
    }
    true
}

pub(super) fn compute_batch_completion_reason(
    failure_count: usize,
    success_count: usize,
    completed_count: usize,
    total_items: usize,
    config: &crate::types::CompletionConfig,
) -> BatchCompletionReason {
    if !should_continue_batch(failure_count, total_items, config) {
        BatchCompletionReason::FailureToleranceExceeded
    } else if completed_count == total_items {
        BatchCompletionReason::AllCompleted
    } else if let Some(min) = config.min_successful {
        if success_count >= min {
            BatchCompletionReason::MinSuccessfulReached
        } else {
            BatchCompletionReason::AllCompleted
        }
    } else {
        BatchCompletionReason::AllCompleted
    }
}

pub(super) fn batch_completion_reason_str(reason: BatchCompletionReason) -> &'static str {
    match reason {
        BatchCompletionReason::AllCompleted => "ALL_COMPLETED",
        BatchCompletionReason::MinSuccessfulReached => "MIN_SUCCESSFUL_REACHED",
        BatchCompletionReason::FailureToleranceExceeded => "FAILURE_TOLERANCE_EXCEEDED",
    }
}

pub(super) fn batch_status_str(failure_count: usize) -> &'static str {
    if failure_count > 0 {
        "FAILED"
    } else {
        "SUCCEEDED"
    }
}

pub(super) fn map_summary_payload(
    total_count: usize,
    success_count: usize,
    failure_count: usize,
    completion_reason: BatchCompletionReason,
) -> serde_json::Value {
    serde_json::json!({
        "type": "MapResult",
        "totalCount": total_count,
        "successCount": success_count,
        "failureCount": failure_count,
        "completionReason": batch_completion_reason_str(completion_reason),
        "status": batch_status_str(failure_count),
    })
}

pub(super) fn parallel_summary_payload(
    total_count: usize,
    success_count: usize,
    failure_count: usize,
    started_count: usize,
    completion_reason: BatchCompletionReason,
) -> serde_json::Value {
    serde_json::json!({
        "type": "ParallelResult",
        "totalCount": total_count,
        "successCount": success_count,
        "failureCount": failure_count,
        "startedCount": started_count,
        "completionReason": batch_completion_reason_str(completion_reason),
        "status": batch_status_str(failure_count),
    })
}
