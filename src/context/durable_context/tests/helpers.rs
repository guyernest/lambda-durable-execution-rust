pub(super) use crate::checkpoint::CheckpointManager;
pub(super) use crate::context::{
    BoxFuture, DurableContextHandle, DurableContextImpl, ExecutionContext,
};
pub(super) use crate::error::DurableError;
pub(super) use crate::mock::{MockCheckpointConfig, MockLambdaService};
pub(super) use crate::retry::NoRetry;
pub(super) use crate::termination::TerminationReason;
pub(super) use crate::types::{
    BatchCompletionReason, BatchItem, BatchItemStatus, BatchResult, CallbackConfig,
    CompletionConfig, DurableExecutionInvocationInput, Duration, InvokeConfig, MapConfig,
    OperationAction, OperationType, ParallelConfig, Serdes, SerdesContext, StepConfig,
    StepSemantics, WaitConditionConfig, WaitConditionDecision,
};
use async_trait::async_trait;
pub(super) use serde_json::json;
pub(super) use std::sync::Arc;
pub(super) use std::time::Duration as StdDuration;

pub(super) fn create_replay_input<T: serde::Serialize>(
    durable_execution_arn: &str,
    input: &T,
    operations: Vec<serde_json::Value>,
) -> serde_json::Value {
    let input_payload = serde_json::to_string(input).expect("serialize test input");

    let mut ops = vec![json!({
        "Id": "execution",
        "Type": "EXECUTION",
        "Status": "STARTED",
        "ExecutionDetails": {
            "InputPayload": input_payload
        }
    })];
    ops.extend(operations);

    json!({
        "DurableExecutionArn": durable_execution_arn,
        "CheckpointToken": "test-token-123",
        "InitialExecutionState": {
            "Operations": ops,
            "NextMarker": null
        }
    })
}

pub(super) async fn make_replay_context(
    durable_execution_arn: &str,
    operations: Vec<serde_json::Value>,
) -> DurableContextHandle {
    let input_json = create_replay_input(durable_execution_arn, &json!({}), operations);
    let input: DurableExecutionInvocationInput =
        serde_json::from_value(input_json).expect("valid invocation input");

    let lambda_service = Arc::new(MockLambdaService::new());

    let exec_ctx = ExecutionContext::new(&input, lambda_service, None, true)
        .await
        .expect("execution context should initialize");
    DurableContextHandle::new(Arc::new(DurableContextImpl::new(exec_ctx)))
}

pub(super) async fn make_replay_context_with_service(
    durable_execution_arn: &str,
    operations: Vec<serde_json::Value>,
    lambda_service: Arc<MockLambdaService>,
) -> DurableContextHandle {
    let input_json = create_replay_input(durable_execution_arn, &json!({}), operations);
    let input: DurableExecutionInvocationInput =
        serde_json::from_value(input_json).expect("valid invocation input");

    let exec_ctx = ExecutionContext::new(&input, lambda_service, None, true)
        .await
        .expect("execution context should initialize");
    DurableContextHandle::new(Arc::new(DurableContextImpl::new(exec_ctx)))
}

pub(super) async fn make_execution_context(
    durable_execution_arn: &str,
) -> (DurableContextHandle, Arc<MockLambdaService>) {
    let input_json = create_replay_input(durable_execution_arn, &json!({}), vec![]);
    let input: DurableExecutionInvocationInput =
        serde_json::from_value(input_json).expect("valid invocation input");

    let lambda_service = Arc::new(MockLambdaService::new());
    let exec_ctx = ExecutionContext::new(&input, lambda_service.clone(), None, true)
        .await
        .expect("execution context should initialize");
    (
        DurableContextHandle::new(Arc::new(DurableContextImpl::new(exec_ctx))),
        lambda_service,
    )
}

pub(super) struct StaticBatchSerdes<T> {
    pub(super) items: Vec<(usize, BatchItemStatus, Option<T>)>,
    pub(super) completion_reason: BatchCompletionReason,
}

#[async_trait]
impl<T: Clone + Send + Sync> Serdes<BatchResult<T>> for StaticBatchSerdes<T> {
    async fn serialize(
        &self,
        _value: Option<&BatchResult<T>>,
        _context: SerdesContext,
    ) -> Result<Option<String>, crate::error::BoxError> {
        Ok(Some("payload".to_string()))
    }

    async fn deserialize(
        &self,
        _data: Option<&str>,
        _context: SerdesContext,
    ) -> Result<Option<BatchResult<T>>, crate::error::BoxError> {
        let mut all = Vec::new();
        for (index, status, result) in &self.items {
            all.push(BatchItem {
                index: *index,
                status: *status,
                result: result.clone(),
                error: None,
            });
        }
        Ok(Some(BatchResult {
            all,
            completion_reason: self.completion_reason,
        }))
    }
}

pub(super) struct SerializeFailSerdes;

#[async_trait]
impl Serdes<u32> for SerializeFailSerdes {
    async fn serialize(
        &self,
        _value: Option<&u32>,
        _context: SerdesContext,
    ) -> Result<Option<String>, crate::error::BoxError> {
        Err(Box::<dyn std::error::Error + Send + Sync>::from(
            std::io::Error::new(std::io::ErrorKind::Other, "serialize failed"),
        ))
    }

    async fn deserialize(
        &self,
        _data: Option<&str>,
        _context: SerdesContext,
    ) -> Result<Option<u32>, crate::error::BoxError> {
        Ok(Some(1))
    }
}

pub(super) struct DeserializeFailSerdes;

#[async_trait]
impl Serdes<u32> for DeserializeFailSerdes {
    async fn serialize(
        &self,
        _value: Option<&u32>,
        _context: SerdesContext,
    ) -> Result<Option<String>, crate::error::BoxError> {
        Ok(Some("1".to_string()))
    }

    async fn deserialize(
        &self,
        _data: Option<&str>,
        _context: SerdesContext,
    ) -> Result<Option<u32>, crate::error::BoxError> {
        Err(Box::<dyn std::error::Error + Send + Sync>::from(
            std::io::Error::new(std::io::ErrorKind::Other, "deserialize failed"),
        ))
    }
}

pub(super) struct DeserializeNoneSerdes;

#[async_trait]
impl Serdes<u32> for DeserializeNoneSerdes {
    async fn serialize(
        &self,
        _value: Option<&u32>,
        _context: SerdesContext,
    ) -> Result<Option<String>, crate::error::BoxError> {
        Ok(Some("1".to_string()))
    }

    async fn deserialize(
        &self,
        _data: Option<&str>,
        _context: SerdesContext,
    ) -> Result<Option<u32>, crate::error::BoxError> {
        Ok(None)
    }
}

pub(super) struct BatchSerializeFailSerdes;

#[async_trait]
impl Serdes<BatchResult<u32>> for BatchSerializeFailSerdes {
    async fn serialize(
        &self,
        _value: Option<&BatchResult<u32>>,
        _context: SerdesContext,
    ) -> Result<Option<String>, crate::error::BoxError> {
        Err(Box::<dyn std::error::Error + Send + Sync>::from(
            std::io::Error::new(std::io::ErrorKind::Other, "serialize failed"),
        ))
    }

    async fn deserialize(
        &self,
        _data: Option<&str>,
        _context: SerdesContext,
    ) -> Result<Option<BatchResult<u32>>, crate::error::BoxError> {
        Ok(Some(BatchResult {
            all: vec![],
            completion_reason: BatchCompletionReason::AllCompleted,
        }))
    }
}

pub(super) struct BatchDeserializeFailSerdes;

#[async_trait]
impl Serdes<BatchResult<u32>> for BatchDeserializeFailSerdes {
    async fn serialize(
        &self,
        _value: Option<&BatchResult<u32>>,
        _context: SerdesContext,
    ) -> Result<Option<String>, crate::error::BoxError> {
        Ok(Some("payload".to_string()))
    }

    async fn deserialize(
        &self,
        _data: Option<&str>,
        _context: SerdesContext,
    ) -> Result<Option<BatchResult<u32>>, crate::error::BoxError> {
        Err(Box::<dyn std::error::Error + Send + Sync>::from(
            std::io::Error::new(std::io::ErrorKind::Other, "deserialize failed"),
        ))
    }
}

#[derive(Clone, Copy)]
pub(super) enum BranchBehavior {
    Ok(u32),
    Fail(&'static str),
    Panic(&'static str),
}

pub(super) fn make_parallel_branch(
    behavior: BranchBehavior,
) -> impl Fn(DurableContextHandle) -> BoxFuture<'static, Result<u32, DurableError>> + Send + Sync + 'static
{
    move |_ctx| {
        Box::pin(async move {
            match behavior {
                BranchBehavior::Ok(value) => Ok(value),
                BranchBehavior::Fail(message) => Err(DurableError::Internal(message.to_string())),
                BranchBehavior::Panic(message) => panic!("{message}"),
            }
        })
    }
}
