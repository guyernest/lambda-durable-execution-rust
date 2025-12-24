use super::helpers::*;
use std::sync::atomic::{AtomicBool, Ordering};

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
async fn test_wait_for_callback_execution_runs_submitter_and_suspends() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    for _ in 0..4 {
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    }

    let ran_submitter = Arc::new(AtomicBool::new(false));
    let ran_submitter_handle = Arc::clone(&ran_submitter);

    let result = tokio::time::timeout(
        StdDuration::from_millis(50),
        ctx.wait_for_callback::<serde_json::Value, _, _>(
            Some("callback"),
            move |_id, _step_ctx| async move {
                ran_submitter_handle.store(true, Ordering::SeqCst);
                Ok(())
            },
            None,
        ),
    )
    .await;

    assert!(result.is_err(), "callback should suspend");
    assert!(
        ran_submitter.load(Ordering::SeqCst),
        "submitter should run before suspension"
    );

    let termination = ctx
        .execution_context()
        .termination_manager
        .get_termination_result()
        .expect("termination should be recorded");
    assert_eq!(termination.reason, TerminationReason::CallbackPending);

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    assert!(updates.iter().any(|update| {
        update.operation_type == OperationType::Callback && update.action == OperationAction::Start
    }));
    assert!(updates.iter().any(|update| {
        update.operation_type == OperationType::Step
            && update.action == OperationAction::Succeed
            && update.name.as_deref() == Some("submitter")
    }));
}
