use super::helpers::*;

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
async fn test_invoke_replay_failed_defaults_message() {
    let arn = "arn:test:durable";
    let step_id = "invoke_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CHAINED_INVOKE",
        "Status": "FAILED",
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
            assert!(message.contains("Invoke failed"));
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
async fn test_invoke_replay_pending_suspends() {
    let arn = "arn:test:durable";
    let step_id = "invoke_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CHAINED_INVOKE",
        "Status": "STARTED",
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let result = tokio::time::timeout(
        StdDuration::from_millis(50),
        ctx.invoke::<serde_json::Value, serde_json::Value>(
            Some("invoke"),
            "fn",
            Option::<serde_json::Value>::None,
        ),
    )
    .await;

    assert!(result.is_err(), "invoke should suspend");

    let termination = ctx
        .execution_context()
        .termination_manager
        .get_termination_result()
        .expect("termination should be recorded");
    assert_eq!(termination.reason, TerminationReason::InvokePending);
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
async fn test_invoke_execution_suspends_and_checkpoints() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let result = tokio::time::timeout(
        StdDuration::from_millis(50),
        ctx.invoke::<serde_json::Value, serde_json::Value>(
            Some("invoke"),
            "fn",
            Some(json!({ "ok": true })),
        ),
    )
    .await;
    assert!(result.is_err(), "invoke should suspend");

    let termination = ctx
        .execution_context()
        .termination_manager
        .get_termination_result()
        .expect("termination should be recorded");
    assert_eq!(termination.reason, TerminationReason::InvokePending);

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    let update = updates
        .iter()
        .find(|u| u.operation_type == OperationType::ChainedInvoke)
        .expect("chained invoke update");
    assert_eq!(update.action, OperationAction::Start);
    let options = update
        .chained_invoke_options
        .as_ref()
        .expect("chained invoke options");
    assert_eq!(options.function_name, "fn");
}

#[tokio::test]
async fn test_invoke_execution_includes_parent_id() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    ctx.execution_context()
        .set_parent_id(Some("parent-invoke".to_string()))
        .await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let result = tokio::time::timeout(
        StdDuration::from_millis(50),
        ctx.invoke::<serde_json::Value, serde_json::Value>(Some("invoke"), "fn", None),
    )
    .await;
    assert!(result.is_err(), "invoke should suspend");

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    let update = updates
        .iter()
        .find(|u| u.operation_type == OperationType::ChainedInvoke)
        .expect("chained invoke update");

    assert_eq!(update.action, OperationAction::Start);
    assert_eq!(update.parent_id.as_deref(), Some("parent-invoke"));
}

#[tokio::test]
async fn test_invoke_execution_without_payload() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let result = tokio::time::timeout(
        StdDuration::from_millis(50),
        ctx.invoke::<serde_json::Value, serde_json::Value>(Some("invoke"), "fn", None),
    )
    .await;
    assert!(result.is_err(), "invoke should suspend");

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    let update = updates
        .iter()
        .find(|u| u.operation_type == OperationType::ChainedInvoke)
        .expect("chained invoke update");
    assert!(update.payload.is_none());
}
