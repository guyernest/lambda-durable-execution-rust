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
