use super::helpers::*;
use crate::error::BoxError;
use crate::types::ChildContextConfig;
use async_trait::async_trait;

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
async fn test_run_in_child_context_execution_without_name() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let value: u32 = ctx
        .run_in_child_context(None, |_child_ctx| async move { Ok(11u32) }, None)
        .await
        .unwrap();

    assert_eq!(value, 11u32);

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    assert!(updates.iter().all(|update| update.name.is_none()));
}

struct SerializeNoneSerdes;

#[async_trait]
impl Serdes<u32> for SerializeNoneSerdes {
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
        Ok(Some(1))
    }
}

#[tokio::test]
async fn test_run_in_child_context_execution_without_payload() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let config = ChildContextConfig::new().with_serdes(Arc::new(SerializeNoneSerdes));
    let value: u32 = ctx
        .run_in_child_context(
            Some("child"),
            |_child_ctx| async move { Ok(4u32) },
            Some(config),
        )
        .await
        .unwrap();

    assert_eq!(value, 4u32);

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    let succeed = updates
        .iter()
        .find(|update| update.action == OperationAction::Succeed)
        .expect("succeed update");
    assert!(succeed.payload.is_none());
}

#[tokio::test]
async fn test_run_in_child_context_execution_includes_parent_id() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    ctx.execution_context()
        .set_parent_id(Some("parent-context".to_string()))
        .await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let value: u32 = ctx
        .run_in_child_context(Some("child"), |_child_ctx| async move { Ok(3u32) }, None)
        .await
        .unwrap();

    assert_eq!(value, 3u32);

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    let start = updates
        .iter()
        .find(|update| update.action == OperationAction::Start)
        .expect("start update");
    assert_eq!(start.parent_id.as_deref(), Some("parent-context"));
}

#[tokio::test]
async fn test_run_in_child_context_execution_failure_checkpoints_fail() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    ctx.execution_context()
        .set_parent_id(Some("parent-context".to_string()))
        .await;

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
    let fail = updates
        .iter()
        .find(|update| {
            update.operation_type == OperationType::Context
                && update.action == OperationAction::Fail
        })
        .expect("fail update");
    assert_eq!(fail.parent_id.as_deref(), Some("parent-context"));
}

#[tokio::test]
async fn test_run_in_child_context_execution_failure_without_name() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let err = ctx
        .run_in_child_context(
            None,
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
    let fail = updates
        .iter()
        .find(|update| {
            update.operation_type == OperationType::Context
                && update.action == OperationAction::Fail
        })
        .expect("fail update");
    assert!(fail.name.is_none());
}

#[tokio::test]
async fn test_run_in_child_context_execution_large_payload_sets_replay_children() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let big = "a".repeat(super::super::CHECKPOINT_SIZE_LIMIT_BYTES + 8);
    let big_payload = big.clone();
    let value: String = ctx
        .run_in_child_context(
            Some("child"),
            |_child_ctx| async move { Ok(big_payload) },
            None,
        )
        .await
        .unwrap();

    assert_eq!(value.len(), big.len());

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    let succeed = updates
        .iter()
        .find(|update| update.action == OperationAction::Succeed)
        .expect("succeed update");
    assert_eq!(
        succeed
            .context_options
            .as_ref()
            .and_then(|opts| opts.replay_children),
        Some(true)
    );
    assert_eq!(succeed.payload.as_deref(), Some(""));
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
async fn test_run_in_child_context_replay_started_executes_again() {
    let arn = "arn:test:durable";
    let step_id = "child_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "Status": "STARTED",
    });

    let lambda_service = Arc::new(MockLambdaService::new());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let ctx = make_replay_context_with_service(arn, vec![op], lambda_service.clone()).await;
    let value: u32 = ctx
        .run_in_child_context(Some("child"), |_child_ctx| async move { Ok(11u32) }, None)
        .await
        .unwrap();

    assert_eq!(value, 11u32);
}

#[tokio::test]
async fn test_run_in_child_context_replay_started_skips_start_checkpoint() {
    let arn = "arn:test:durable";
    let step_id = "child_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "SubType": "RunInChildContext",
        "Status": "STARTED",
    });

    let lambda_service = Arc::new(MockLambdaService::new());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let ctx = make_replay_context_with_service(arn, vec![op], lambda_service.clone()).await;
    let value: u32 = ctx
        .run_in_child_context(Some("child"), |_child_ctx| async move { Ok(42u32) }, None)
        .await
        .unwrap();

    assert_eq!(value, 42);

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    assert!(updates
        .iter()
        .all(|update| update.action != OperationAction::Start));
}

#[tokio::test]
async fn test_run_in_child_context_replay_missing_payload_executes_again() {
    let arn = "arn:test:durable";
    let step_id = "child_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "Status": "SUCCEEDED",
        "ContextDetails": {},
    });

    let lambda_service = Arc::new(MockLambdaService::new());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let ctx = make_replay_context_with_service(arn, vec![op], lambda_service.clone()).await;
    let value: u32 = ctx
        .run_in_child_context(Some("child"), |_child_ctx| async move { Ok(21u32) }, None)
        .await
        .unwrap();

    assert_eq!(value, 21u32);
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
async fn test_run_in_child_context_replay_failure_defaults_message() {
    let arn = "arn:test:durable";
    let step_id = "child_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "Status": "FAILED",
        "ContextDetails": {},
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
            assert!(message.contains("Child context failed"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
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
