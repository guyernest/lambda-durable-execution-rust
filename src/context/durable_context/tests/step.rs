use super::helpers::*;

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
