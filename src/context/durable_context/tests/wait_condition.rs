use super::helpers::*;
use crate::error::BoxError;
use async_trait::async_trait;

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
        Ok(Some(10))
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
async fn test_wait_for_condition_replay_failed_defaults_message() {
    let arn = "arn:test:durable";
    let step_id = "wait_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "STEP",
        "SubType": "WaitForCondition",
        "Status": "FAILED",
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
        .expect_err("wait_for_condition should fail in replay");

    match err {
        DurableError::StepFailed { message, .. } => {
            assert!(message.contains("Wait for condition failed"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_wait_for_condition_execution_stop_succeeds() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let config = WaitConditionConfig::new(
        0u32,
        Arc::new(|_state: &u32, _attempt: u32| WaitConditionDecision::Stop),
    );
    let value = ctx
        .wait_for_condition(
            Some("wait"),
            |state, _step_ctx| async move { Ok(state + 1) },
            config,
        )
        .await
        .unwrap();

    assert_eq!(value, 1);

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    assert!(updates.iter().any(|update| {
        update.operation_type == OperationType::Step
            && update.action == OperationAction::Succeed
            && update.sub_type.as_deref() == Some("WaitForCondition")
    }));
}

#[tokio::test]
async fn test_wait_for_condition_execution_continue_retries() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    ctx.execution_context()
        .set_parent_id(Some("parent-wait".to_string()))
        .await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let config = WaitConditionConfig::new(
        0u32,
        Arc::new(
            |_state: &u32, _attempt: u32| WaitConditionDecision::Continue {
                delay: Duration::seconds(2),
            },
        ),
    );
    let result = tokio::time::timeout(
        StdDuration::from_millis(50),
        ctx.wait_for_condition(
            Some("wait"),
            |state, _step_ctx| async move { Ok(state + 1) },
            config,
        ),
    )
    .await;

    assert!(result.is_err(), "wait_for_condition should suspend");

    let termination = ctx
        .execution_context()
        .termination_manager
        .get_termination_result()
        .expect("termination should be recorded");
    assert_eq!(termination.reason, TerminationReason::RetryScheduled);

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    let start_update = updates
        .iter()
        .find(|update| update.action == OperationAction::Start)
        .expect("start update");
    assert_eq!(start_update.parent_id.as_deref(), Some("parent-wait"));

    let retry_update = updates
        .iter()
        .find(|update| update.action == OperationAction::Retry)
        .expect("retry update");
    let options = retry_update.step_options.as_ref().expect("step options");
    assert_eq!(options.next_attempt_delay_seconds, Some(2));
}

#[tokio::test]
async fn test_wait_for_condition_execution_check_fn_error_returns_error() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let config = WaitConditionConfig::new(
        0u32,
        Arc::new(|_state: &u32, _attempt: u32| WaitConditionDecision::Stop),
    );
    let err = ctx
        .wait_for_condition(
            Some("wait"),
            |_state, _step_ctx| async move {
                Err::<u32, _>(DurableError::Internal("boom".to_string()))
            },
            config,
        )
        .await
        .expect_err("check_fn error should surface");

    match err {
        DurableError::Internal(message) => assert!(message.contains("boom")),
        other => panic!("unexpected error: {other:?}"),
    }

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    assert!(updates.iter().any(|update| {
        update.operation_type == OperationType::Step
            && update.action == OperationAction::Start
            && update.sub_type.as_deref() == Some("WaitForCondition")
    }));
}

#[tokio::test]
async fn test_wait_for_condition_execution_max_attempts_exceeded() {
    let arn = "arn:test:durable";
    let step_id = "wait_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "STEP",
        "SubType": "WaitForCondition",
        "Status": "STARTED",
        "StepDetails": { "Attempt": 1 },
    });

    let lambda_service = Arc::new(MockLambdaService::new());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let ctx = make_replay_context_with_service(arn, vec![op], lambda_service).await;
    let config = WaitConditionConfig::new(
        0u32,
        Arc::new(|_state: &u32, _attempt: u32| WaitConditionDecision::Stop),
    )
    .with_max_attempts(1);

    let err = ctx
        .wait_for_condition(
            Some("wait"),
            |_state, _step_ctx| async move {
                panic!("check_fn should not run when attempts exceeded");
                #[allow(unreachable_code)]
                Ok(0u32)
            },
            config,
        )
        .await
        .expect_err("max attempts should error");

    match err {
        DurableError::WaitConditionExceeded { attempts, .. } => {
            assert_eq!(attempts, 2);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_wait_for_condition_execution_max_attempts_not_exceeded() {
    let arn = "arn:test:durable";
    let step_id = "wait_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "STEP",
        "SubType": "WaitForCondition",
        "Status": "STARTED",
        "StepDetails": { "Attempt": 0 },
    });

    let lambda_service = Arc::new(MockLambdaService::new());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let ctx = make_replay_context_with_service(arn, vec![op], lambda_service).await;
    let config = WaitConditionConfig::new(
        1u32,
        Arc::new(|_state: &u32, _attempt: u32| WaitConditionDecision::Stop),
    )
    .with_max_attempts(2);

    let value = ctx
        .wait_for_condition(
            Some("wait"),
            |state, _step_ctx| async move { Ok(state + 1) },
            config,
        )
        .await
        .expect("wait_for_condition should succeed");

    assert_eq!(value, 2);
}

#[tokio::test]
async fn test_wait_for_condition_execution_uses_replayed_state() {
    let arn = "arn:test:durable";
    let step_id = "wait_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let payload = serde_json::to_string(&5u32).unwrap();
    let op = json!({
        "Id": hashed_id,
        "Type": "STEP",
        "SubType": "WaitForCondition",
        "Status": "STARTED",
        "StepDetails": { "Attempt": 0, "Result": payload },
    });

    let lambda_service = Arc::new(MockLambdaService::new());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let ctx = make_replay_context_with_service(arn, vec![op], lambda_service).await;
    let config = WaitConditionConfig::new(
        0u32,
        Arc::new(|_state: &u32, _attempt: u32| WaitConditionDecision::Stop),
    );

    let value = ctx
        .wait_for_condition(
            Some("wait"),
            |state, _step_ctx| async move { Ok(state + 1) },
            config,
        )
        .await
        .unwrap();

    assert_eq!(value, 6u32);
}

#[tokio::test]
async fn test_wait_for_condition_execution_stop_without_payload() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let config = WaitConditionConfig::new(
        0u32,
        Arc::new(|_state: &u32, _attempt: u32| WaitConditionDecision::Stop),
    )
    .with_serdes(Arc::new(SerializeNoneSerdes));

    let value = ctx
        .wait_for_condition(
            Some("wait"),
            |state, _step_ctx| async move { Ok(state + 1) },
            config,
        )
        .await
        .unwrap();

    assert_eq!(value, 1u32);

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    let succeed_update = updates
        .iter()
        .find(|update| update.action == OperationAction::Succeed)
        .expect("succeed update");
    assert!(succeed_update.payload.is_none());
}

#[tokio::test]
async fn test_wait_for_condition_replay_deserialize_none_returns_error() {
    let arn = "arn:test:durable";
    let step_id = "wait_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let payload = serde_json::to_string(&1u32).unwrap();
    let op = json!({
        "Id": hashed_id,
        "Type": "STEP",
        "SubType": "WaitForCondition",
        "Status": "SUCCEEDED",
        "StepDetails": { "Result": payload, "Attempt": 1 },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let config = WaitConditionConfig::new(
        0u32,
        Arc::new(|_state: &u32, _attempt: u32| WaitConditionDecision::Stop),
    )
    .with_serdes(Arc::new(DeserializeNoneSerdes));

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
async fn test_wait_for_condition_execution_deserialize_none_uses_initial_state() {
    let arn = "arn:test:durable";
    let step_id = "wait_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let payload = serde_json::to_string(&5u32).unwrap();
    let op = json!({
        "Id": hashed_id,
        "Type": "STEP",
        "SubType": "WaitForCondition",
        "Status": "STARTED",
        "StepDetails": { "Attempt": 0, "Result": payload },
    });

    let lambda_service = Arc::new(MockLambdaService::new());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let ctx = make_replay_context_with_service(arn, vec![op], lambda_service).await;
    let config = WaitConditionConfig::new(
        3u32,
        Arc::new(|_state: &u32, _attempt: u32| WaitConditionDecision::Stop),
    )
    .with_serdes(Arc::new(DeserializeNoneSerdes));

    let value = ctx
        .wait_for_condition(
            Some("wait"),
            |state, _step_ctx| async move { Ok(state + 1) },
            config,
        )
        .await
        .unwrap();

    assert_eq!(value, 4u32);
}

#[tokio::test]
async fn test_wait_for_condition_execution_without_name() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let config = WaitConditionConfig::new(
        0u32,
        Arc::new(|_state: &u32, _attempt: u32| WaitConditionDecision::Stop),
    );

    let value = ctx
        .wait_for_condition(None, |state, _step_ctx| async move { Ok(state) }, config)
        .await
        .unwrap();

    assert_eq!(value, 0u32);

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    assert!(updates.iter().all(|update| update.name.is_none()));
}

#[tokio::test]
async fn test_wait_for_condition_execution_continue_without_payload() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let config = WaitConditionConfig::new(
        0u32,
        Arc::new(
            |_state: &u32, _attempt: u32| WaitConditionDecision::Continue {
                delay: Duration::seconds(3),
            },
        ),
    )
    .with_serdes(Arc::new(SerializeNoneSerdes));

    let result = tokio::time::timeout(
        StdDuration::from_millis(50),
        ctx.wait_for_condition(
            Some("wait"),
            |state, _step_ctx| async move { Ok(state + 1) },
            config,
        ),
    )
    .await;

    assert!(result.is_err(), "wait_for_condition should suspend");

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    let retry_update = updates
        .iter()
        .find(|update| update.action == OperationAction::Retry)
        .expect("retry update");
    assert!(retry_update.payload.is_none());
}

#[tokio::test]
async fn test_wait_for_condition_execution_missing_step_details_uses_initial_state() {
    let arn = "arn:test:durable";
    let step_id = "wait_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "STEP",
        "SubType": "WaitForCondition",
        "Status": "STARTED",
    });

    let lambda_service = Arc::new(MockLambdaService::new());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let ctx = make_replay_context_with_service(arn, vec![op], lambda_service).await;
    let config = WaitConditionConfig::new(
        0u32,
        Arc::new(|_state: &u32, _attempt: u32| WaitConditionDecision::Stop),
    );

    let value = ctx
        .wait_for_condition(
            Some("wait"),
            |state, _step_ctx| async move { Ok(state + 2) },
            config,
        )
        .await
        .unwrap();

    assert_eq!(value, 2u32);
}
