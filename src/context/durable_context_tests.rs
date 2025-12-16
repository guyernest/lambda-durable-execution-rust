use super::{DurableContextHandle, DurableContextImpl, ExecutionContext};
use crate::checkpoint::CheckpointManager;
use crate::types::DurableExecutionInvocationInput;
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

    // Build a Lambda client with dummy config. It will never be used in replay-mode tests.
    let config = aws_sdk_lambda::Config::builder()
        .region(aws_sdk_lambda::config::Region::new("us-east-1"))
        .behavior_version(aws_sdk_lambda::config::BehaviorVersion::latest())
        .build();
    let client = aws_sdk_lambda::Client::from_conf(config);

    let exec_ctx = ExecutionContext::new(&input, Arc::new(client), None, true)
        .await
        .expect("execution context should initialize");
    DurableContextHandle::new(Arc::new(DurableContextImpl::new(exec_ctx)))
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
