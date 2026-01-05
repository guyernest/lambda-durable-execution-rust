use super::helpers::*;

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
async fn test_create_callback_execution_includes_parent_id() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    ctx.execution_context()
        .set_parent_id(Some("parent-callback".to_string()))
        .await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let _handle = ctx
        .create_callback::<serde_json::Value>(Some("callback"), None)
        .await
        .unwrap();

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
    assert_eq!(update.parent_id.as_deref(), Some("parent-callback"));
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
async fn test_callback_handle_wait_execution_suspends() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let handle = ctx
        .create_callback::<serde_json::Value>(Some("callback"), None)
        .await
        .unwrap();

    let result = tokio::time::timeout(StdDuration::from_millis(50), handle.wait()).await;
    assert!(result.is_err(), "callback wait should suspend");

    let termination = ctx
        .execution_context()
        .termination_manager
        .get_termination_result()
        .expect("termination should be recorded");
    assert_eq!(termination.reason, TerminationReason::CallbackPending);
}

#[tokio::test]
async fn test_callback_handle_wait_replay_success_returns_value() {
    let arn = "arn:test:durable";
    let step_id = "callback_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let payload = serde_json::to_string(&json!({"ok": true})).unwrap();
    let op = json!({
        "Id": hashed_id,
        "Type": "CALLBACK",
        "Status": "SUCCEEDED",
        "CallbackDetails": { "Result": payload },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let handle = ctx
        .create_callback::<serde_json::Value>(Some("callback"), None)
        .await
        .unwrap();

    let value = handle.wait().await.unwrap();
    assert_eq!(value, json!({"ok": true}));
}

#[tokio::test]
async fn test_callback_handle_wait_replay_missing_result_returns_error() {
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
    let handle = ctx
        .create_callback::<serde_json::Value>(Some("callback"), None)
        .await
        .unwrap();

    let err = handle
        .wait()
        .await
        .expect_err("missing result should error");
    match err {
        DurableError::Internal(message) => {
            assert!(message.contains("Missing callback result"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_callback_handle_wait_replay_failed_returns_error() {
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
    let handle = ctx
        .create_callback::<serde_json::Value>(Some("callback"), None)
        .await
        .unwrap();

    let err = handle.wait().await.expect_err("callback should fail");
    match err {
        DurableError::CallbackFailed { message, .. } => {
            assert!(message.contains("nope"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_callback_handle_wait_replay_pending_suspends() {
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

    let result = tokio::time::timeout(StdDuration::from_millis(50), handle.wait()).await;
    assert!(result.is_err(), "callback wait should suspend");

    let termination = ctx
        .execution_context()
        .termination_manager
        .get_termination_result()
        .expect("termination should be recorded");
    assert_eq!(termination.reason, TerminationReason::CallbackPending);
}

#[tokio::test]
async fn test_callback_handle_wait_raw_replay_failed_returns_error() {
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
    let handle = ctx
        .create_callback::<serde_json::Value>(Some("callback"), None)
        .await
        .unwrap();

    let err = handle
        .wait_raw()
        .await
        .expect_err("callback raw should fail");
    match err {
        DurableError::CallbackFailed { message, .. } => {
            assert!(message.contains("nope"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_callback_handle_wait_raw_replay_missing_result_returns_error() {
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
    let handle = ctx
        .create_callback::<serde_json::Value>(Some("callback"), None)
        .await
        .unwrap();

    let err = handle
        .wait_raw()
        .await
        .expect_err("callback raw should fail without result");
    match err {
        DurableError::Internal(message) => {
            assert!(message.contains("Missing callback result"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}
