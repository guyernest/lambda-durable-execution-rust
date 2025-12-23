use super::helpers::*;

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
