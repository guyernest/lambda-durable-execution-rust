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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use crate::error::BoxError;
    use crate::mock::MockLambdaService;
    use crate::types::{
        DurableExecutionInvocationInput, ExecutionDetails, InitialExecutionState, Operation,
        OperationStatus, OperationType,
    };
    use std::sync::Arc;

    struct NoneSerdes;

    #[async_trait]
    impl Serdes<u32> for NoneSerdes {
        async fn serialize(
            &self,
            _value: Option<&u32>,
            _context: SerdesContext,
        ) -> Result<Option<String>, BoxError> {
            Ok(None)
        }

        async fn deserialize(
            &self,
            _data: Option<&str>,
            _context: SerdesContext,
        ) -> Result<Option<u32>, BoxError> {
            Ok(None)
        }
    }

    struct ErrorSerdes;

    #[async_trait]
    impl Serdes<u32> for ErrorSerdes {
        async fn serialize(
            &self,
            _value: Option<&u32>,
            _context: SerdesContext,
        ) -> Result<Option<String>, BoxError> {
            Err(Box::<dyn std::error::Error + Send + Sync>::from("serialize boom"))
        }

        async fn deserialize(
            &self,
            _data: Option<&str>,
            _context: SerdesContext,
        ) -> Result<Option<u32>, BoxError> {
            Err(Box::<dyn std::error::Error + Send + Sync>::from("deserialize boom"))
        }
    }

    async fn make_execution_context() -> ExecutionContext {
        let input = DurableExecutionInvocationInput {
            durable_execution_arn: "arn:test:durable".to_string(),
            checkpoint_token: "token-0".to_string(),
            initial_execution_state: InitialExecutionState {
                operations: vec![Operation {
                    id: "execution".to_string(),
                    parent_id: None,
                    name: None,
                    operation_type: OperationType::Execution,
                    sub_type: None,
                    status: OperationStatus::Started,
                    step_details: None,
                    callback_details: None,
                    wait_details: None,
                    execution_details: Some(ExecutionDetails {
                        input_payload: Some("{}".to_string()),
                        output_payload: None,
                    }),
                    context_details: None,
                    chained_invoke_details: None,
                }],
                next_marker: None,
            },
        };

        let lambda_service = Arc::new(MockLambdaService::new());
        ExecutionContext::new(&input, lambda_service, None, true)
            .await
            .expect("execution context should initialize")
    }

    #[tokio::test]
    async fn test_safe_serialize_none_serdes_uses_json() {
        let ctx = make_execution_context().await;
        let payload = safe_serialize(None, Some(&1u32), "id", None, &ctx).await;
        assert_eq!(payload.as_deref(), Some("1"));
    }

    #[tokio::test]
    async fn test_safe_serialize_custom_none_returns_none() {
        let ctx = make_execution_context().await;
        let payload = safe_serialize(Some(Arc::new(NoneSerdes)), Some(&1u32), "id", None, &ctx).await;
        assert!(payload.is_none());
    }

    #[tokio::test]
    async fn test_safe_deserialize_none_serdes_missing_data_returns_none() {
        let ctx = make_execution_context().await;
        let value: Option<u32> = safe_deserialize(None, None, "id", None, &ctx).await;
        assert!(value.is_none());
    }

    #[tokio::test]
    async fn test_safe_deserialize_error_terminates() {
        let ctx = make_execution_context().await;
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            safe_deserialize(Some(Arc::new(ErrorSerdes)), Some("1"), "id", None, &ctx),
        )
        .await;
        assert!(result.is_err(), "safe_deserialize should suspend on error");

        let termination = ctx
            .termination_manager
            .get_termination_result()
            .expect("termination should be recorded");
        assert_eq!(termination.reason, crate::termination::TerminationReason::SerdesFailed);
    }

    #[tokio::test]
    async fn test_safe_serialize_required_none_terminates() {
        let ctx = make_execution_context().await;
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            safe_serialize_required_with_serdes(Arc::new(NoneSerdes), &1u32, "id", None, &ctx),
        )
        .await;
        assert!(result.is_err(), "safe_serialize_required should suspend on none");
    }

    #[tokio::test]
    async fn test_safe_deserialize_required_none_terminates() {
        let ctx = make_execution_context().await;
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            safe_deserialize_required_with_serdes(Arc::new(NoneSerdes), "1", "id", None, &ctx),
        )
        .await;
        assert!(result.is_err(), "safe_deserialize_required should suspend on none");
    }
}
