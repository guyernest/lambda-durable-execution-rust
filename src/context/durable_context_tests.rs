use super::{BoxFuture, DurableContextHandle, DurableContextImpl, ExecutionContext};
use crate::checkpoint::CheckpointManager;
use crate::error::DurableError;
use crate::mock::{MockCheckpointConfig, MockLambdaService};
use crate::retry::NoRetry;
use crate::termination::TerminationReason;
use crate::types::{
    BatchCompletionReason, BatchItem, BatchItemStatus, BatchResult, CallbackConfig,
    CompletionConfig, DurableExecutionInvocationInput, Duration, InvokeConfig, MapConfig,
    OperationAction, OperationType, ParallelConfig, Serdes, SerdesContext, StepConfig,
    StepSemantics, WaitConditionConfig, WaitConditionDecision,
};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration as StdDuration;

fn create_replay_input<T: serde::Serialize>(
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

async fn make_replay_context(
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

async fn make_replay_context_with_service(
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

async fn make_execution_context(
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

struct StaticBatchSerdes<T> {
    items: Vec<(usize, BatchItemStatus, Option<T>)>,
    completion_reason: BatchCompletionReason,
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

struct SerializeFailSerdes;

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

struct DeserializeFailSerdes;

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

struct DeserializeNoneSerdes;

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

struct BatchSerializeFailSerdes;

#[async_trait]
impl Serdes<BatchResult<u32>> for BatchSerializeFailSerdes {
    async fn serialize(
        &self,
        _value: Option<&BatchResult<u32>>,
        _context: SerdesContext,
    ) -> Result<Option<String>, crate::error::BoxError> {
        Err(Box::<dyn std::error::Error + Send + Sync>::from(
            std::io::Error::new(std::io::ErrorKind::Other, "batch serialize failed"),
        ))
    }

    async fn deserialize(
        &self,
        _data: Option<&str>,
        _context: SerdesContext,
    ) -> Result<Option<BatchResult<u32>>, crate::error::BoxError> {
        Ok(Some(BatchResult {
            all: Vec::new(),
            completion_reason: BatchCompletionReason::AllCompleted,
        }))
    }
}

struct BatchDeserializeFailSerdes;

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
            std::io::Error::new(std::io::ErrorKind::Other, "batch deserialize failed"),
        ))
    }
}

#[derive(Clone, Copy)]
enum BranchBehavior {
    Ok(u32),
    Fail(&'static str),
    Panic(&'static str),
}

fn make_parallel_branch(
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

#[tokio::test]
async fn test_step_replay_returns_cached_result() {
    let arn = "arn:test:durable";
    let step_id = "step_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let result = serde_json::to_string(&123u32).unwrap();
    let step_op = json!({
        "Id": hashed_id,
        "Type": "STEP",
        "Status": "SUCCEEDED",
        "StepDetails": { "Result": result, "Attempt": 0 },
    });

    let ctx = make_replay_context(arn, vec![step_op]).await;
    let value: u32 = ctx
        .step(
            Some("step"),
            |_step_ctx| async move {
                panic!("step_fn should not run in replay");
            },
            None::<crate::types::StepConfig<u32>>,
        )
        .await
        .unwrap();

    assert_eq!(value, 123u32);
}

#[tokio::test]
async fn test_step_execution_success_checkpoints_succeed() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let value: u32 = ctx
        .step(
            Some("step"),
            |_step_ctx| async move { Ok(99u32) },
            None::<StepConfig<u32>>,
        )
        .await
        .unwrap();

    assert_eq!(value, 99u32);

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    assert!(updates.iter().any(|update| {
        update.operation_type == OperationType::Step && update.action == OperationAction::Succeed
    }));
}

#[tokio::test]
async fn test_step_execution_failure_no_retry_checkpoints_fail() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let config = StepConfig::<u32>::new().with_retry_strategy(Arc::new(NoRetry));
    let err = ctx
        .step(
            Some("step"),
            |_step_ctx| async move {
                Err(Box::<dyn std::error::Error + Send + Sync>::from(
                    std::io::Error::new(std::io::ErrorKind::Other, "boom"),
                ))
            },
            Some(config),
        )
        .await
        .expect_err("step should fail without retry");

    match err {
        DurableError::StepFailed {
            message, attempts, ..
        } => {
            assert!(message.contains("boom"));
            assert_eq!(attempts, 1);
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    assert!(updates.iter().any(|update| {
        update.operation_type == OperationType::Step && update.action == OperationAction::Fail
    }));
}

#[tokio::test]
async fn test_step_replay_started_at_most_once_no_retry_fails() {
    let arn = "arn:test:durable";
    let step_id = "step_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let step_op = json!({
        "Id": hashed_id,
        "Type": "STEP",
        "Status": "STARTED",
        "StepDetails": { "Attempt": 0 },
    });

    let lambda_service = Arc::new(MockLambdaService::new());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let ctx = make_replay_context_with_service(arn, vec![step_op], lambda_service.clone()).await;
    let config = StepConfig::new()
        .with_semantics(StepSemantics::AtMostOncePerRetry)
        .with_retry_strategy(Arc::new(NoRetry));
    let err = ctx
        .step(
            Some("step"),
            |_step_ctx| async move { Ok(1u32) },
            Some(config),
        )
        .await
        .expect_err("step should fail on interrupted at-most-once replay");

    match err {
        DurableError::StepFailed { message, .. } => {
            assert!(message.contains("Step interrupted"));
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    assert!(updates.iter().any(|update| {
        update.operation_type == OperationType::Step && update.action == OperationAction::Fail
    }));
}

#[tokio::test]
async fn test_step_serdes_serialize_failure_terminates() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let config = StepConfig::new().with_serdes(Arc::new(SerializeFailSerdes));
    let result = tokio::time::timeout(
        StdDuration::from_millis(50),
        ctx.step(Some("step"), |_ctx| async move { Ok(1u32) }, Some(config)),
    )
    .await;

    assert!(result.is_err(), "step should suspend on serdes failure");

    let termination = ctx
        .execution_context()
        .termination_manager
        .get_termination_result()
        .expect("termination should be recorded");
    assert_eq!(termination.reason, TerminationReason::SerdesFailed);
    let message = termination.message.unwrap_or_default();
    assert!(message.contains("Serialization failed"));
}

#[tokio::test]
async fn test_step_serdes_deserialize_failure_terminates() {
    let arn = "arn:test:durable";
    let step_id = "step_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let payload = serde_json::to_string(&1u32).unwrap();
    let step_op = json!({
        "Id": hashed_id,
        "Type": "STEP",
        "Status": "SUCCEEDED",
        "StepDetails": { "Result": payload, "Attempt": 0 },
    });

    let ctx = make_replay_context(arn, vec![step_op]).await;
    let config = StepConfig::new().with_serdes(Arc::new(DeserializeFailSerdes));
    let result = tokio::time::timeout(
        StdDuration::from_millis(50),
        ctx.step(Some("step"), |_ctx| async move { Ok(1u32) }, Some(config)),
    )
    .await;

    assert!(result.is_err(), "step should suspend on serdes failure");

    let termination = ctx
        .execution_context()
        .termination_manager
        .get_termination_result()
        .expect("termination should be recorded");
    assert_eq!(termination.reason, TerminationReason::SerdesFailed);
    let message = termination.message.unwrap_or_default();
    assert!(message.contains("Deserialization failed"));
}

#[tokio::test]
async fn test_step_replay_failed_returns_error() {
    let arn = "arn:test:durable";
    let step_id = "step_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let step_op = json!({
        "Id": hashed_id,
        "Type": "STEP",
        "Status": "FAILED",
        "StepDetails": {
            "Attempt": 1,
            "Error": { "ErrorType": "Error", "ErrorMessage": "boom" }
        },
    });

    let ctx = make_replay_context(arn, vec![step_op]).await;
    let err = ctx
        .step(
            Some("step"),
            |_step_ctx| async move {
                panic!("step_fn should not run in replay");
            },
            None::<crate::types::StepConfig<u32>>,
        )
        .await
        .expect_err("step should fail in replay");

    match err {
        crate::error::DurableError::StepFailed {
            message, attempts, ..
        } => {
            assert_eq!(message, "boom");
            assert_eq!(attempts, 2);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_step_replay_missing_output_returns_error() {
    let arn = "arn:test:durable";
    let step_id = "step_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let step_op = json!({
        "Id": hashed_id,
        "Type": "STEP",
        "Status": "SUCCEEDED",
        "StepDetails": { "Attempt": 1 },
    });

    let ctx = make_replay_context(arn, vec![step_op]).await;
    let err = ctx
        .step(
            Some("step"),
            |_step_ctx| async move {
                panic!("step_fn should not run in replay");
            },
            None::<crate::types::StepConfig<u32>>,
        )
        .await
        .expect_err("missing payload should error");

    match err {
        DurableError::Internal(message) => {
            assert!(message.contains("Missing step output"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_step_replay_failed_defaults_message() {
    let arn = "arn:test:durable";
    let step_id = "step_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let step_op = json!({
        "Id": hashed_id,
        "Type": "STEP",
        "Status": "FAILED",
        "StepDetails": { "Attempt": 2 },
    });

    let ctx = make_replay_context(arn, vec![step_op]).await;
    let err = ctx
        .step(
            Some("step"),
            |_step_ctx| async move {
                panic!("step_fn should not run in replay");
            },
            None::<crate::types::StepConfig<u32>>,
        )
        .await
        .expect_err("step should fail in replay");

    match err {
        DurableError::StepFailed {
            message, attempts, ..
        } => {
            assert_eq!(message, "Replayed failure");
            assert_eq!(attempts, 3);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_wait_replay_succeeded_returns_ok() {
    let arn = "arn:test:durable";
    let step_id = "wait_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let wait_op = json!({
        "Id": hashed_id,
        "Type": "WAIT",
        "Status": "SUCCEEDED",
    });

    let ctx = make_replay_context(arn, vec![wait_op]).await;
    ctx.wait(Some("wait"), Duration::seconds(5)).await.unwrap();
}

#[tokio::test]
async fn test_wait_execution_suspends_after_checkpoint() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let result = tokio::time::timeout(
        StdDuration::from_millis(50),
        ctx.wait(Some("wait"), Duration::seconds(1)),
    )
    .await;
    assert!(result.is_err(), "wait should suspend");

    let termination = ctx
        .execution_context()
        .termination_manager
        .get_termination_result()
        .expect("termination reason should be recorded");
    assert_eq!(termination.reason, TerminationReason::WaitScheduled);
}

#[tokio::test]
async fn test_wait_replay_failed_returns_error() {
    let arn = "arn:test:durable";
    let step_id = "wait_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let wait_op = json!({
        "Id": hashed_id,
        "Type": "WAIT",
        "Status": "FAILED",
    });

    let ctx = make_replay_context(arn, vec![wait_op]).await;
    let err = ctx
        .wait(Some("wait"), Duration::seconds(5))
        .await
        .expect_err("wait should fail in replay");
    assert!(err.to_string().contains("Wait failed in replay"));
}

#[tokio::test]
async fn test_wait_zero_duration_returns_immediately() {
    let arn = "arn:test:durable";
    let ctx = make_replay_context(arn, vec![]).await;

    ctx.wait(Some("wait"), Duration::seconds(0))
        .await
        .expect("zero duration should return");
}

#[tokio::test]
async fn test_create_callback_execution_checkpoints_start_with_options() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let config = CallbackConfig::<serde_json::Value>::new()
        .with_timeout(Duration::seconds(10))
        .with_heartbeat_timeout(Duration::seconds(3));

    let handle = ctx
        .create_callback(Some("callback"), Some(config))
        .await
        .unwrap();

    let hashed_id = CheckpointManager::hash_id("callback_0");
    assert_eq!(handle.callback_id(), format!("{arn}:{hashed_id}"));

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    let update = updates
        .iter()
        .find(|u| u.operation_type == OperationType::Callback)
        .expect("callback update");
    assert_eq!(update.action, OperationAction::Start);
    let options = update.callback_options.as_ref().expect("callback options");
    assert_eq!(options.timeout_seconds, Some(10));
    assert_eq!(options.heartbeat_timeout_seconds, Some(3));
}

#[tokio::test]
async fn test_run_in_child_context_execution_success_checkpoints_succeed() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let value: u32 = ctx
        .run_in_child_context(Some("child"), |_child_ctx| async move { Ok(7u32) }, None)
        .await
        .unwrap();

    assert_eq!(value, 7u32);

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    assert!(updates.iter().any(|update| {
        update.operation_type == OperationType::Context && update.action == OperationAction::Succeed
    }));
}

#[tokio::test]
async fn test_run_in_child_context_execution_failure_checkpoints_fail() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let err = ctx
        .run_in_child_context(
            Some("child"),
            |_child_ctx| async move { Err(DurableError::Internal("boom".to_string())) },
            None::<crate::types::ChildContextConfig<u32>>,
        )
        .await
        .expect_err("child context should fail");

    match err {
        DurableError::ChildContextFailed { message, .. } => {
            assert!(message.contains("boom"));
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    assert!(updates.iter().any(|update| {
        update.operation_type == OperationType::Context && update.action == OperationAction::Fail
    }));
}

#[tokio::test]
async fn test_map_empty_items_with_batch_serdes() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let batch_serdes = Arc::new(StaticBatchSerdes::<u32> {
        items: Vec::new(),
        completion_reason: BatchCompletionReason::AllCompleted,
    });
    let config = MapConfig::new().with_serdes(batch_serdes);

    let batch: BatchResult<u32> = ctx
        .map(
            Some("map"),
            Vec::<u32>::new(),
            |_item, _child_ctx, _idx| async move {
                panic!("map_fn should not run for empty items");
                #[allow(unreachable_code)]
                Ok::<u32, crate::error::DurableError>(0)
            },
            Some(config),
        )
        .await
        .unwrap();

    assert!(batch.all.is_empty());
    assert_eq!(batch.completion_reason, BatchCompletionReason::AllCompleted);

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    assert!(updates.iter().any(|update| {
        update.operation_type == OperationType::Context
            && update.action == OperationAction::Succeed
            && update.payload.as_deref() == Some("payload")
    }));
}

#[tokio::test]
async fn test_map_batch_serdes_serialize_failure_terminates() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let config = MapConfig::new().with_serdes(Arc::new(BatchSerializeFailSerdes));
    let result = tokio::time::timeout(
        StdDuration::from_millis(50),
        ctx.map(
            Some("map"),
            Vec::<u32>::new(),
            |_item, _child_ctx, _idx| async move {
                panic!("map_fn should not run for empty items");
                #[allow(unreachable_code)]
                Ok::<u32, DurableError>(0)
            },
            Some(config),
        ),
    )
    .await;

    assert!(
        result.is_err(),
        "map should suspend on batch serdes failure"
    );

    let termination = ctx
        .execution_context()
        .termination_manager
        .get_termination_result()
        .expect("termination should be recorded");
    assert_eq!(termination.reason, TerminationReason::SerdesFailed);
    let message = termination.message.unwrap_or_default();
    assert!(message.contains("Serialization failed"));
}

#[tokio::test]
async fn test_map_batch_serdes_deserialize_failure_terminates() {
    let arn = "arn:test:durable";
    let step_id = "map_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "SubType": "Map",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": "payload" },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let config = MapConfig::new().with_serdes(Arc::new(BatchDeserializeFailSerdes));
    let result = tokio::time::timeout(
        StdDuration::from_millis(50),
        ctx.map(
            Some("map"),
            vec![1u32],
            |_item, _child_ctx, _idx| async move {
                panic!("map_fn should not run in replay");
                #[allow(unreachable_code)]
                Ok::<u32, DurableError>(0)
            },
            Some(config),
        ),
    )
    .await;

    assert!(
        result.is_err(),
        "map should suspend on batch serdes failure"
    );

    let termination = ctx
        .execution_context()
        .termination_manager
        .get_termination_result()
        .expect("termination should be recorded");
    assert_eq!(termination.reason, TerminationReason::SerdesFailed);
    let message = termination.message.unwrap_or_default();
    assert!(message.contains("Deserialization failed"));
}

#[tokio::test]
async fn test_map_replay_missing_child_payload_returns_error() {
    let arn = "arn:test:durable";
    let name = Some("map");

    let map_step_id = "map_0".to_string();
    let map_hashed_id = CheckpointManager::hash_id(&map_step_id);
    let summary = json!({
        "totalCount": 1,
        "successCount": 1,
        "failureCount": 0,
    })
    .to_string();

    let map_op = json!({
        "Id": map_hashed_id,
        "Type": "CONTEXT",
        "SubType": "Map",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": summary },
    });

    let item_name = "map-item-0".to_string();
    let child_step_id = format!("{}_{}", item_name, 1);
    let child_hashed_id = CheckpointManager::hash_id(&child_step_id);
    let child_result = serde_json::to_string(&1u32).unwrap();
    let child_op = json!({
        "Id": child_hashed_id,
        "Type": "CONTEXT",
        "SubType": "MapIteration",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": child_result },
    });

    let ctx = make_replay_context(arn, vec![map_op, child_op]).await;
    let config = MapConfig::new().with_item_serdes(Arc::new(DeserializeNoneSerdes));
    let err = ctx
        .map(
            name,
            vec![1u32],
            |_item, _child_ctx, _idx| async move {
                panic!("map_fn should not run in replay");
                #[allow(unreachable_code)]
                Ok::<u32, DurableError>(0)
            },
            Some(config),
        )
        .await
        .expect_err("map should fail when child payload is missing");

    match err {
        DurableError::Internal(message) => {
            assert!(message.contains("Missing child context output"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_map_min_successful_completes_early() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    for _ in 0..4 {
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    }

    let completion = CompletionConfig::new().with_min_successful(1);
    let config = MapConfig::new()
        .with_max_concurrency(1)
        .with_completion_config(completion);

    let batch: BatchResult<u32> = ctx
        .map(
            Some("map"),
            vec![1u32, 2u32, 3u32],
            |item, _child_ctx, idx| async move {
                if idx == 0 {
                    Ok::<u32, DurableError>(item + 1)
                } else {
                    panic!("map should stop after min_successful");
                }
            },
            Some(config),
        )
        .await
        .unwrap();

    assert_eq!(
        batch.completion_reason,
        BatchCompletionReason::MinSuccessfulReached
    );
    assert_eq!(batch.success_count(), 1);
}

#[tokio::test]
async fn test_map_failure_tolerance_exceeded() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    for _ in 0..4 {
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    }

    let completion = CompletionConfig::new().with_tolerated_failures(0);
    let config = MapConfig::new()
        .with_max_concurrency(1)
        .with_completion_config(completion);

    let batch: BatchResult<u32> = ctx
        .map(
            Some("map"),
            vec![1u32, 2u32],
            |_item, _child_ctx, idx| async move {
                if idx == 0 {
                    Err(DurableError::Internal("boom".to_string()))
                } else {
                    panic!("map should stop after failure tolerance exceeded");
                }
            },
            Some(config),
        )
        .await
        .unwrap();

    assert_eq!(
        batch.completion_reason,
        BatchCompletionReason::FailureToleranceExceeded
    );
    assert_eq!(batch.failure_count(), 1);
}

#[tokio::test]
async fn test_map_replay_skips_incomplete_children() {
    let arn = "arn:test:durable";
    let name = Some("map");

    // Top-level map context uses counter 0.
    let map_step_id = "map_0".to_string();
    let map_hashed_id = CheckpointManager::hash_id(&map_step_id);
    let summary = json!({
        "totalCount": 2,
        "successCount": 2,
        "failureCount": 0,
    })
    .to_string();

    let map_op = json!({
        "Id": map_hashed_id,
        "Type": "CONTEXT",
        "SubType": "Map",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": summary },
    });

    // Child 0 uses counter 1.
    let item0_name = "map-item-0".to_string();
    let child0_step_id = format!("{}_{}", item0_name, 1);
    let child0_hashed_id = CheckpointManager::hash_id(&child0_step_id);
    let child0_result = serde_json::to_string(&1u32).unwrap();
    let child0_op = json!({
        "Id": child0_hashed_id,
        "Type": "CONTEXT",
        "SubType": "MapIteration",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": child0_result },
    });

    // Child 1 uses counter 2.
    let item1_name = "map-item-1".to_string();
    let child1_step_id = format!("{}_{}", item1_name, 2);
    let child1_hashed_id = CheckpointManager::hash_id(&child1_step_id);
    let child1_result = serde_json::to_string(&2u32).unwrap();
    let child1_op = json!({
        "Id": child1_hashed_id,
        "Type": "CONTEXT",
        "SubType": "MapIteration",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": child1_result },
    });

    let ctx = make_replay_context(arn, vec![map_op, child0_op, child1_op]).await;

    let items = vec![10u32, 20u32, 30u32];
    let batch: super::BatchResult<u32> = ctx
        .map(
            name,
            items,
            |_item, _child_ctx, _idx| async move {
                panic!("map_fn should not run in replay");
                #[allow(unreachable_code)]
                Ok::<u32, crate::error::DurableError>(0)
            },
            None,
        )
        .await
        .unwrap();

    assert_eq!(batch.success_count(), 2);
    assert_eq!(batch.failure_count(), 0);
    assert_eq!(batch.values(), vec![1u32, 2u32]);
}

#[tokio::test]
async fn test_invoke_replay_success_returns_result() {
    let arn = "arn:test:durable";
    let step_id = "invoke_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let payload = serde_json::to_string(&json!({"ok": true})).unwrap();
    let op = json!({
        "Id": hashed_id,
        "Type": "CHAINED_INVOKE",
        "Status": "SUCCEEDED",
        "ChainedInvokeDetails": { "Result": payload },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let value: serde_json::Value = ctx
        .invoke::<serde_json::Value, serde_json::Value>(
            Some("invoke"),
            "fn",
            Option::<serde_json::Value>::None,
        )
        .await
        .unwrap();

    assert_eq!(value, json!({"ok": true}));
}

#[tokio::test]
async fn test_invoke_replay_failure_returns_error() {
    let arn = "arn:test:durable";
    let step_id = "invoke_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CHAINED_INVOKE",
        "Status": "FAILED",
        "ChainedInvokeDetails": {
            "Error": { "ErrorType": "Error", "ErrorMessage": "boom" }
        },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let err = ctx
        .invoke::<serde_json::Value, serde_json::Value>(
            Some("invoke"),
            "fn",
            Option::<serde_json::Value>::None,
        )
        .await
        .expect_err("invoke should fail in replay");

    match err {
        DurableError::InvocationFailed { message, .. } => {
            assert!(message.contains("boom"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_invoke_replay_missing_result_returns_error() {
    let arn = "arn:test:durable";
    let step_id = "invoke_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CHAINED_INVOKE",
        "Status": "SUCCEEDED",
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let err = ctx
        .invoke::<serde_json::Value, serde_json::Value>(
            Some("invoke"),
            "fn",
            Option::<serde_json::Value>::None,
        )
        .await
        .expect_err("invoke should fail without result");

    match err {
        DurableError::Internal(message) => {
            assert!(message.contains("Missing invoke result"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_invoke_replay_serdes_failure_terminates() {
    let arn = "arn:test:durable";
    let step_id = "invoke_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let payload = serde_json::to_string(&1u32).unwrap();
    let op = json!({
        "Id": hashed_id,
        "Type": "CHAINED_INVOKE",
        "Status": "SUCCEEDED",
        "ChainedInvokeDetails": { "Result": payload },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let config = InvokeConfig::new().with_result_serdes(Arc::new(DeserializeFailSerdes));
    let result = tokio::time::timeout(
        StdDuration::from_millis(50),
        ctx.invoke_with_config::<serde_json::Value, u32>(
            Some("invoke"),
            "fn",
            Option::<serde_json::Value>::None,
            Some(config),
        ),
    )
    .await;

    assert!(result.is_err(), "invoke should suspend on serdes failure");

    let termination = ctx
        .execution_context()
        .termination_manager
        .get_termination_result()
        .expect("termination should be recorded");
    assert_eq!(termination.reason, TerminationReason::SerdesFailed);
    let message = termination.message.unwrap_or_default();
    assert!(message.contains("Deserialization failed"));
}

#[tokio::test]
async fn test_wait_for_callback_replay_missing_result_returns_error() {
    let arn = "arn:test:durable";
    let step_id = "callback_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CALLBACK",
        "Status": "SUCCEEDED",
        "CallbackDetails": {},
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let err = ctx
        .wait_for_callback::<serde_json::Value, _, _>(
            Some("callback"),
            |_id, _step_ctx| async move {
                panic!("submitter should not run in replay");
                #[allow(unreachable_code)]
                Ok(())
            },
            None,
        )
        .await
        .expect_err("callback should fail without result");

    match err {
        DurableError::Internal(message) => {
            assert!(message.contains("Missing callback result"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_wait_for_callback_replay_success_returns_result() {
    let arn = "arn:test:durable";
    let step_id = "callback_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let payload = serde_json::to_string(&json!({"approved": true})).unwrap();
    let op = json!({
        "Id": hashed_id,
        "Type": "CALLBACK",
        "Status": "SUCCEEDED",
        "CallbackDetails": { "Result": payload },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let value: serde_json::Value = ctx
        .wait_for_callback::<serde_json::Value, _, _>(
            Some("callback"),
            |_id, _step_ctx| async move {
                panic!("submitter should not run in replay");
                #[allow(unreachable_code)]
                Ok(())
            },
            None,
        )
        .await
        .unwrap();

    assert_eq!(value, json!({"approved": true}));
}

#[tokio::test]
async fn test_wait_for_callback_replay_failure_returns_error() {
    let arn = "arn:test:durable";
    let step_id = "callback_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CALLBACK",
        "Status": "FAILED",
        "CallbackDetails": {
            "Error": { "ErrorType": "Error", "ErrorMessage": "nope" }
        },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let err = ctx
        .wait_for_callback::<serde_json::Value, _, _>(
            Some("callback"),
            |_id, _step_ctx| async move {
                panic!("submitter should not run in replay");
                #[allow(unreachable_code)]
                Ok(())
            },
            None,
        )
        .await
        .expect_err("callback should fail in replay");

    match err {
        DurableError::CallbackFailed { message, .. } => {
            assert!(message.contains("nope"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_wait_for_callback_replay_serdes_failure_terminates() {
    let arn = "arn:test:durable";
    let step_id = "callback_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let payload = serde_json::to_string(&1u32).unwrap();
    let op = json!({
        "Id": hashed_id,
        "Type": "CALLBACK",
        "Status": "SUCCEEDED",
        "CallbackDetails": { "Result": payload },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let config = CallbackConfig::new().with_serdes(Arc::new(DeserializeFailSerdes));
    let result = tokio::time::timeout(
        StdDuration::from_millis(50),
        ctx.wait_for_callback::<u32, _, _>(
            Some("callback"),
            |_id, _step_ctx| async move {
                panic!("submitter should not run in replay");
                #[allow(unreachable_code)]
                Ok(())
            },
            Some(config),
        ),
    )
    .await;

    assert!(result.is_err(), "callback should suspend on serdes failure");

    let termination = ctx
        .execution_context()
        .termination_manager
        .get_termination_result()
        .expect("termination should be recorded");
    assert_eq!(termination.reason, TerminationReason::SerdesFailed);
    let message = termination.message.unwrap_or_default();
    assert!(message.contains("Deserialization failed"));
}

#[tokio::test]
async fn test_create_callback_replay_uses_existing_id_and_payload() {
    let arn = "arn:test:durable";
    let step_id = "callback_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let payload = serde_json::to_string(&json!({"ok": true})).unwrap();
    let op = json!({
        "Id": hashed_id,
        "Type": "CALLBACK",
        "Status": "SUCCEEDED",
        "CallbackDetails": {
            "CallbackId": "cb-123",
            "Result": payload,
        },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let handle = ctx
        .create_callback::<serde_json::Value>(Some("callback"), None)
        .await
        .unwrap();

    assert_eq!(handle.callback_id(), "cb-123");
    let raw = handle.wait_raw().await.unwrap();
    assert_eq!(raw, payload);
}

#[tokio::test]
async fn test_create_callback_replay_defaults_callback_id() {
    let arn = "arn:test:durable";
    let step_id = "callback_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CALLBACK",
        "Status": "STARTED",
        "CallbackDetails": {},
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let handle = ctx
        .create_callback::<serde_json::Value>(Some("callback"), None)
        .await
        .unwrap();

    assert_eq!(handle.callback_id(), format!("{arn}:{hashed_id}"));
}

#[tokio::test]
async fn test_run_in_child_context_replay_uses_context_result() {
    let arn = "arn:test:durable";
    let step_id = "child_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let payload = serde_json::to_string(&42u32).unwrap();
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": payload },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let value: u32 = ctx
        .run_in_child_context(
            Some("child"),
            |_child_ctx| async move {
                panic!("child context should not run in replay");
                #[allow(unreachable_code)]
                Ok(0u32)
            },
            None,
        )
        .await
        .unwrap();

    assert_eq!(value, 42u32);
}

#[tokio::test]
async fn test_run_in_child_context_replay_uses_execution_output_fallback() {
    let arn = "arn:test:durable";
    let step_id = "child_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let payload = serde_json::to_string(&json!({"ok": true})).unwrap();
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "Status": "SUCCEEDED",
        "ExecutionDetails": { "OutputPayload": payload },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let value: serde_json::Value = ctx
        .run_in_child_context(
            Some("child"),
            |_child_ctx| async move {
                panic!("child context should not run in replay");
                #[allow(unreachable_code)]
                Ok(json!({}))
            },
            None,
        )
        .await
        .unwrap();

    assert_eq!(value, json!({"ok": true}));
}

#[tokio::test]
async fn test_run_in_child_context_replay_failure_returns_error() {
    let arn = "arn:test:durable";
    let step_id = "child_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "Status": "FAILED",
        "ContextDetails": {
            "Error": { "ErrorType": "Error", "ErrorMessage": "boom" }
        },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let err = ctx
        .run_in_child_context(
            Some("child"),
            |_child_ctx| async move {
                panic!("child context should not run in replay");
                #[allow(unreachable_code)]
                Ok(json!({}))
            },
            None,
        )
        .await
        .expect_err("child context should fail in replay");

    match err {
        DurableError::ChildContextFailed { message, .. } => {
            assert!(message.contains("boom"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_wait_for_condition_replay_success_returns_result() {
    let arn = "arn:test:durable";
    let step_id = "wait_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let payload = serde_json::to_string(&json!({"ok": true})).unwrap();
    let op = json!({
        "Id": hashed_id,
        "Type": "STEP",
        "SubType": "WaitForCondition",
        "Status": "SUCCEEDED",
        "StepDetails": { "Result": payload, "Attempt": 1 },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let config = WaitConditionConfig::new(
        json!({"initial": true}),
        Arc::new(|_state: &serde_json::Value, _attempt: u32| WaitConditionDecision::Stop),
    );
    let value: serde_json::Value = ctx
        .wait_for_condition(
            Some("wait"),
            |_state, _step_ctx| async move {
                panic!("check_fn should not run in replay");
                #[allow(unreachable_code)]
                Ok(json!({}))
            },
            config,
        )
        .await
        .unwrap();

    assert_eq!(value, json!({"ok": true}));
}

#[tokio::test]
async fn test_wait_for_condition_replay_missing_result_returns_error() {
    let arn = "arn:test:durable";
    let step_id = "wait_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "STEP",
        "SubType": "WaitForCondition",
        "Status": "SUCCEEDED",
        "StepDetails": { "Attempt": 1 },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let config = WaitConditionConfig::new(
        0u32,
        Arc::new(|_state: &u32, _attempt: u32| WaitConditionDecision::Stop),
    );
    let err = ctx
        .wait_for_condition(
            Some("wait"),
            |_state, _step_ctx| async move {
                panic!("check_fn should not run in replay");
                #[allow(unreachable_code)]
                Ok(0u32)
            },
            config,
        )
        .await
        .expect_err("missing payload should error");

    match err {
        DurableError::Internal(message) => {
            assert!(message.contains("Missing wait-for-condition result"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_wait_for_condition_replay_failed_returns_error() {
    let arn = "arn:test:durable";
    let step_id = "wait_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "STEP",
        "SubType": "WaitForCondition",
        "Status": "FAILED",
        "StepDetails": {
            "Attempt": 2,
            "Error": { "ErrorType": "Error", "ErrorMessage": "nope" }
        },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let config = WaitConditionConfig::new(
        0u32,
        Arc::new(|_state: &u32, _attempt: u32| WaitConditionDecision::Stop),
    );
    let err = ctx
        .wait_for_condition(
            Some("wait"),
            |_state, _step_ctx| async move {
                panic!("check_fn should not run in replay");
                #[allow(unreachable_code)]
                Ok(0u32)
            },
            config,
        )
        .await
        .expect_err("wait_for_condition should fail in replay");

    match err {
        DurableError::StepFailed {
            message, attempts, ..
        } => {
            assert!(message.contains("nope"));
            assert_eq!(attempts, 2);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_map_replay_failed_returns_error() {
    let arn = "arn:test:durable";
    let step_id = "map_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "SubType": "Map",
        "Status": "FAILED",
        "ContextDetails": {
            "Error": { "ErrorType": "Error", "ErrorMessage": "map boom" }
        },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let err = ctx
        .map(
            Some("map"),
            vec![1u32],
            |_item, _child_ctx, _idx| async move {
                panic!("map_fn should not run in replay");
                #[allow(unreachable_code)]
                Ok::<u32, DurableError>(0)
            },
            None,
        )
        .await
        .expect_err("map should fail in replay");

    match err {
        DurableError::BatchOperationFailed { message, .. } => {
            assert!(message.contains("map boom"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_map_replay_batch_serdes_validation_failed() {
    let arn = "arn:test:durable";
    let step_id = "map_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "SubType": "Map",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": "ignored" },
    });

    let batch_serdes = StaticBatchSerdes {
        items: vec![(2, BatchItemStatus::Succeeded, Some("x".to_string()))],
        completion_reason: BatchCompletionReason::AllCompleted,
    };

    let cfg = MapConfig::new().with_serdes(Arc::new(batch_serdes));

    let ctx = make_replay_context(arn, vec![op]).await;
    let err = ctx
        .map(
            Some("map"),
            vec![1u32, 2u32],
            |_item, _child_ctx, _idx| async move {
                panic!("map_fn should not run in replay");
                #[allow(unreachable_code)]
                Ok::<String, DurableError>("".to_string())
            },
            Some(cfg),
        )
        .await
        .expect_err("map should fail replay validation");

    match err {
        DurableError::ReplayValidationFailed { expected, .. } => {
            assert!(expected.contains("map totalCount"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_map_replay_batch_serdes_returns_batch() {
    let arn = "arn:test:durable";
    let step_id = "map_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "SubType": "Map",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": "ignored" },
    });

    let batch_serdes = StaticBatchSerdes {
        items: vec![
            (0, BatchItemStatus::Succeeded, Some("a".to_string())),
            (1, BatchItemStatus::Succeeded, Some("b".to_string())),
        ],
        completion_reason: BatchCompletionReason::AllCompleted,
    };

    let cfg = MapConfig::new().with_serdes(Arc::new(batch_serdes));

    let ctx = make_replay_context(arn, vec![op]).await;
    let batch: BatchResult<String> = ctx
        .map(
            Some("map"),
            vec![1u32, 2u32],
            |_item, _child_ctx, _idx| async move {
                panic!("map_fn should not run in replay");
                #[allow(unreachable_code)]
                Ok::<String, DurableError>("".to_string())
            },
            Some(cfg),
        )
        .await
        .unwrap();

    assert_eq!(batch.success_count(), 2);
    assert_eq!(batch.values(), vec!["a".to_string(), "b".to_string()]);
}

#[tokio::test]
async fn test_parallel_replay_failed_returns_error() {
    let arn = "arn:test:durable";
    let step_id = "parallel_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "SubType": "Parallel",
        "Status": "FAILED",
        "ContextDetails": {
            "Error": { "ErrorType": "Error", "ErrorMessage": "parallel boom" }
        },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let branch = |_ctx: DurableContextHandle| async move {
        panic!("branch should not run in replay");
        #[allow(unreachable_code)]
        Ok::<String, DurableError>("".to_string())
    };

    let err = ctx
        .parallel(Some("parallel"), vec![branch, branch], None)
        .await
        .expect_err("parallel should fail in replay");

    match err {
        DurableError::BatchOperationFailed { message, .. } => {
            assert!(message.contains("parallel boom"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_parallel_min_successful_completes_early() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    for _ in 0..4 {
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    }

    let completion = CompletionConfig::new().with_min_successful(1);
    let config = ParallelConfig::new()
        .with_max_concurrency(1)
        .with_completion_config(completion);

    let branches = vec![
        make_parallel_branch(BranchBehavior::Ok(10)),
        make_parallel_branch(BranchBehavior::Panic(
            "parallel should stop after min_successful",
        )),
    ];

    let batch: BatchResult<u32> = ctx
        .parallel(Some("parallel"), branches, Some(config))
        .await
        .unwrap();

    assert_eq!(
        batch.completion_reason,
        BatchCompletionReason::MinSuccessfulReached
    );
    assert_eq!(batch.success_count(), 1);
}

#[tokio::test]
async fn test_parallel_failure_tolerance_exceeded() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    for _ in 0..4 {
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    }

    let completion = CompletionConfig::new().with_tolerated_failures(0);
    let config = ParallelConfig::new()
        .with_max_concurrency(1)
        .with_completion_config(completion);

    let branches = vec![
        make_parallel_branch(BranchBehavior::Fail("boom")),
        make_parallel_branch(BranchBehavior::Panic(
            "parallel should stop after failure tolerance exceeded",
        )),
    ];

    let batch: BatchResult<u32> = ctx
        .parallel(Some("parallel"), branches, Some(config))
        .await
        .unwrap();

    assert_eq!(
        batch.completion_reason,
        BatchCompletionReason::FailureToleranceExceeded
    );
    assert_eq!(batch.failure_count(), 1);
}

#[tokio::test]
async fn test_parallel_batch_serdes_serialize_failure_terminates() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    for _ in 0..3 {
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    }

    let config = ParallelConfig::new().with_serdes(Arc::new(BatchSerializeFailSerdes));
    let branches = vec![make_parallel_branch(BranchBehavior::Ok(1))];

    let result = tokio::time::timeout(
        StdDuration::from_millis(50),
        ctx.parallel(Some("parallel"), branches, Some(config)),
    )
    .await;

    assert!(
        result.is_err(),
        "parallel should suspend on batch serdes failure"
    );

    let termination = ctx
        .execution_context()
        .termination_manager
        .get_termination_result()
        .expect("termination should be recorded");
    assert_eq!(termination.reason, TerminationReason::SerdesFailed);
    let message = termination.message.unwrap_or_default();
    assert!(message.contains("Serialization failed"));
}

#[tokio::test]
async fn test_parallel_batch_serdes_deserialize_failure_terminates() {
    let arn = "arn:test:durable";
    let step_id = "parallel_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "SubType": "Parallel",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": "payload" },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let config = ParallelConfig::new().with_serdes(Arc::new(BatchDeserializeFailSerdes));
    let branches = vec![make_parallel_branch(BranchBehavior::Panic(
        "parallel branch should not run in replay",
    ))];

    let result = tokio::time::timeout(
        StdDuration::from_millis(50),
        ctx.parallel(Some("parallel"), branches, Some(config)),
    )
    .await;

    assert!(
        result.is_err(),
        "parallel should suspend on batch serdes failure"
    );

    let termination = ctx
        .execution_context()
        .termination_manager
        .get_termination_result()
        .expect("termination should be recorded");
    assert_eq!(termination.reason, TerminationReason::SerdesFailed);
    let message = termination.message.unwrap_or_default();
    assert!(message.contains("Deserialization failed"));
}

#[tokio::test]
async fn test_parallel_replay_missing_child_payload_returns_error() {
    let arn = "arn:test:durable";
    let name = Some("parallel");

    let par_step_id = "parallel_0".to_string();
    let par_hashed_id = CheckpointManager::hash_id(&par_step_id);
    let summary = json!({
        "totalCount": 1,
        "successCount": 1,
        "failureCount": 0,
    })
    .to_string();

    let par_op = json!({
        "Id": par_hashed_id,
        "Type": "CONTEXT",
        "SubType": "Parallel",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": summary },
    });

    let branch_name = "parallel-branch-0".to_string();
    let child_step_id = format!("{}_{}", branch_name, 1);
    let child_hashed_id = CheckpointManager::hash_id(&child_step_id);
    let child_result = serde_json::to_string(&1u32).unwrap();
    let child_op = json!({
        "Id": child_hashed_id,
        "Type": "CONTEXT",
        "SubType": "ParallelBranch",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": child_result },
    });

    let ctx = make_replay_context(arn, vec![par_op, child_op]).await;
    let config = ParallelConfig::new().with_item_serdes(Arc::new(DeserializeNoneSerdes));
    let branches = vec![make_parallel_branch(BranchBehavior::Panic(
        "parallel branch should not run in replay",
    ))];

    let err = ctx
        .parallel(name, branches, Some(config))
        .await
        .expect_err("parallel should fail when child payload is missing");

    match err {
        DurableError::Internal(message) => {
            assert!(message.contains("Missing child context output"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_parallel_replay_batch_serdes_validation_failed() {
    let arn = "arn:test:durable";
    let step_id = "parallel_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "SubType": "Parallel",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": "ignored" },
    });

    let batch_serdes = StaticBatchSerdes {
        items: vec![(2, BatchItemStatus::Succeeded, Some("x".to_string()))],
        completion_reason: BatchCompletionReason::AllCompleted,
    };
    let cfg = ParallelConfig::new().with_serdes(Arc::new(batch_serdes));

    let ctx = make_replay_context(arn, vec![op]).await;
    let branch = |_ctx: DurableContextHandle| async move {
        panic!("branch should not run in replay");
        #[allow(unreachable_code)]
        Ok::<String, DurableError>("".to_string())
    };

    let err = ctx
        .parallel(Some("parallel"), vec![branch, branch], Some(cfg))
        .await
        .expect_err("parallel should fail replay validation");

    match err {
        DurableError::ReplayValidationFailed { expected, .. } => {
            assert!(expected.contains("parallel totalCount"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_parallel_replay_batch_serdes_returns_batch() {
    let arn = "arn:test:durable";
    let step_id = "parallel_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "SubType": "Parallel",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": "ignored" },
    });

    let batch_serdes = StaticBatchSerdes {
        items: vec![
            (0, BatchItemStatus::Succeeded, Some("a".to_string())),
            (1, BatchItemStatus::Succeeded, Some("b".to_string())),
        ],
        completion_reason: BatchCompletionReason::AllCompleted,
    };
    let cfg = ParallelConfig::new().with_serdes(Arc::new(batch_serdes));

    let ctx = make_replay_context(arn, vec![op]).await;
    let branch = |_ctx: DurableContextHandle| async move {
        panic!("branch should not run in replay");
        #[allow(unreachable_code)]
        Ok::<String, DurableError>("".to_string())
    };

    let batch: BatchResult<String> = ctx
        .parallel(Some("parallel"), vec![branch, branch], Some(cfg))
        .await
        .unwrap();

    assert_eq!(batch.success_count(), 2);
    assert_eq!(batch.values(), vec!["a".to_string(), "b".to_string()]);
}

#[tokio::test]
async fn test_parallel_replay_skips_incomplete_children() {
    let arn = "arn:test:durable";
    let name = Some("parallel");

    // Top-level parallel context uses counter 0.
    let par_step_id = "parallel_0".to_string();
    let par_hashed_id = CheckpointManager::hash_id(&par_step_id);
    let summary = json!({
        "totalCount": 1,
        "successCount": 1,
        "failureCount": 0,
    })
    .to_string();

    let par_op = json!({
        "Id": par_hashed_id,
        "Type": "CONTEXT",
        "SubType": "Parallel",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": summary },
    });

    // Branch 0 uses counter 1.
    let branch0_name = "parallel-branch-0".to_string();
    let child0_step_id = format!("{}_{}", branch0_name, 1);
    let child0_hashed_id = CheckpointManager::hash_id(&child0_step_id);
    let child0_result = serde_json::to_string(&"ok").unwrap();
    let child0_op = json!({
        "Id": child0_hashed_id,
        "Type": "CONTEXT",
        "SubType": "ParallelBranch",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": child0_result },
    });

    let ctx = make_replay_context(arn, vec![par_op, child0_op]).await;

    let branch = |_child_ctx: DurableContextHandle| {
        Box::pin(async move {
            panic!("branch should not run in replay");
            #[allow(unreachable_code)]
            Ok::<String, crate::error::DurableError>("".to_string())
        })
    };
    let branches = vec![branch, branch];

    let batch: super::BatchResult<String> = ctx.parallel(name, branches, None).await.unwrap();
    assert_eq!(batch.success_count(), 1);
    assert_eq!(batch.failure_count(), 0);
    assert_eq!(batch.values(), vec!["ok".to_string()]);
}

#[tokio::test]
async fn test_child_context_replay_children_reconstructs_result() {
    let arn = "arn:test:durable";
    let name = Some("child");

    // Top-level child context uses counter 0.
    let child_step_id = "child_0".to_string();
    let child_hashed_id = CheckpointManager::hash_id(&child_step_id);
    let child_op = json!({
        "Id": child_hashed_id,
        "Type": "CONTEXT",
        "SubType": "RunInChildContext",
        "Status": "SUCCEEDED",
        "ContextDetails": { "ReplayChildren": true, "Result": "" },
    });

    // Child steps share the parent's operation counter; the first child step uses counter 1.
    let step_id = "step_1".to_string();
    let step_hashed_id = CheckpointManager::hash_id(&step_id);
    let step_result = serde_json::to_string(&42u32).unwrap();
    let step_op = json!({
        "Id": step_hashed_id,
        "Type": "STEP",
        "SubType": "Step",
        "Status": "SUCCEEDED",
        "StepDetails": { "Result": step_result },
    });

    let ctx = make_replay_context(arn, vec![child_op, step_op]).await;

    let out: u32 = ctx
        .run_in_child_context(
            name,
            |child_ctx| async move {
                let v: u32 = child_ctx
                    .step(Some("step"), |_step_ctx| async move { Ok(0u32) }, None)
                    .await?;
                Ok(v)
            },
            None,
        )
        .await
        .unwrap();

    assert_eq!(out, 42u32);
}
