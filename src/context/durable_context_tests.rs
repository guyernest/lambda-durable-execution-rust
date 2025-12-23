use super::{DurableContextHandle, DurableContextImpl, ExecutionContext};
use crate::checkpoint::CheckpointManager;
use crate::error::DurableError;
use crate::mock::MockLambdaService;
use crate::types::{DurableExecutionInvocationInput, Duration};
use serde_json::json;
use std::sync::Arc;

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
