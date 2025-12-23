use super::helpers::*;

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
