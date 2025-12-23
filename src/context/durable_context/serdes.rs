use crate::context::ExecutionContext;
use crate::types::{Serdes, SerdesContext};
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
