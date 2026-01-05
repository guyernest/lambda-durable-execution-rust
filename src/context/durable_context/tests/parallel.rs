use super::helpers::*;
use crate::types::NamedParallelBranch;

#[tokio::test]
async fn test_parallel_replay_failed_returns_error() {
    let arn = "arn:test:durable";
    let step_id = "parallel_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "SubType": "Parallel",
        "Status": "FAILED",
        "ContextDetails": {
            "Error": { "ErrorType": "Error", "ErrorMessage": "parallel boom" }
        },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let branch = |_ctx: DurableContextHandle| async move {
        panic!("branch should not run in replay");
        #[allow(unreachable_code)]
        Ok::<String, DurableError>("".to_string())
    };

    let err = ctx
        .parallel(Some("parallel"), vec![branch, branch], None)
        .await
        .expect_err("parallel should fail in replay");

    match err {
        DurableError::BatchOperationFailed { message, .. } => {
            assert!(message.contains("parallel boom"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_parallel_replay_failed_defaults_message() {
    let arn = "arn:test:durable";
    let step_id = "parallel_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "SubType": "Parallel",
        "Status": "FAILED",
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let branch = |_ctx: DurableContextHandle| async move {
        panic!("branch should not run in replay");
        #[allow(unreachable_code)]
        Ok::<String, DurableError>("".to_string())
    };

    let err = ctx
        .parallel(Some("parallel"), vec![branch], None)
        .await
        .expect_err("parallel should fail in replay");

    match err {
        DurableError::BatchOperationFailed { message, .. } => {
            assert_eq!(message, "Batch operation failed");
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_parallel_min_successful_completes_early() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    for _ in 0..4 {
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    }

    let completion = CompletionConfig::new().with_min_successful(1);
    let config = ParallelConfig::new()
        .with_max_concurrency(1)
        .with_completion_config(completion);

    let branches = vec![
        make_parallel_branch(BranchBehavior::Ok(10)),
        make_parallel_branch(BranchBehavior::Panic(
            "parallel should stop after min_successful",
        )),
    ];

    let batch: BatchResult<u32> = ctx
        .parallel(Some("parallel"), branches, Some(config))
        .await
        .unwrap();

    assert_eq!(
        batch.completion_reason,
        BatchCompletionReason::MinSuccessfulReached
    );
    assert_eq!(batch.success_count(), 1);
}

#[tokio::test]
async fn test_parallel_execution_empty_branches_with_batch_serdes() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    for _ in 0..2 {
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    }

    type BranchFn =
        fn(DurableContextHandle) -> BoxFuture<'static, crate::error::DurableResult<u32>>;
    let branches: Vec<NamedParallelBranch<BranchFn>> = Vec::new();

    let batch_serdes = StaticBatchSerdes::<u32> {
        items: Vec::new(),
        completion_reason: BatchCompletionReason::AllCompleted,
    };
    let config = ParallelConfig::new().with_serdes(Arc::new(batch_serdes));

    let batch: BatchResult<u32> = ctx
        .parallel_named(Some("parallel"), branches, Some(config))
        .await
        .unwrap();

    assert!(batch.all.is_empty());
    assert_eq!(batch.completion_reason, BatchCompletionReason::AllCompleted);
}

#[tokio::test]
async fn test_parallel_execution_empty_branches_without_batch_serdes() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    for _ in 0..2 {
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    }

    type BranchFn =
        fn(DurableContextHandle) -> BoxFuture<'static, crate::error::DurableResult<u32>>;
    let branches: Vec<BranchFn> = Vec::new();

    let batch: BatchResult<u32> = ctx
        .parallel(Some("parallel"), branches, None)
        .await
        .unwrap();

    assert!(batch.all.is_empty());
    assert_eq!(batch.completion_reason, BatchCompletionReason::AllCompleted);
}

#[tokio::test]
async fn test_parallel_execution_includes_parent_id() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    ctx.execution_context()
        .set_parent_id(Some("parent-parallel".to_string()))
        .await;

    for _ in 0..2 {
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    }

    type BranchFn =
        fn(DurableContextHandle) -> BoxFuture<'static, crate::error::DurableResult<u32>>;
    let branches: Vec<BranchFn> = Vec::new();

    let batch: BatchResult<u32> = ctx
        .parallel(Some("parallel"), branches, None)
        .await
        .unwrap();

    assert!(batch.all.is_empty());

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    let update = updates
        .iter()
        .find(|u| {
            u.operation_type == OperationType::Context
                && u.sub_type.as_deref() == Some("Parallel")
                && u.action == OperationAction::Start
        })
        .expect("parallel start update");

    assert_eq!(update.parent_id.as_deref(), Some("parent-parallel"));
}

#[tokio::test]
async fn test_parallel_branch_parent_id_links_to_parallel_context() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    for _ in 0..4 {
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    }

    let branches = vec![make_parallel_branch(BranchBehavior::Ok(1))];
    let batch: BatchResult<u32> = ctx
        .parallel(Some("parallel"), branches, None)
        .await
        .unwrap();

    assert_eq!(batch.success_count(), 1);

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    let parallel_start = updates
        .iter()
        .find(|update| {
            update.operation_type == OperationType::Context
                && update.sub_type.as_deref() == Some("Parallel")
                && update.action == OperationAction::Start
        })
        .expect("parallel start update");
    let branch_start = updates
        .iter()
        .find(|update| {
            update.operation_type == OperationType::Context
                && update.sub_type.as_deref() == Some("ParallelBranch")
                && update.action == OperationAction::Start
        })
        .expect("parallel branch start update");

    assert_eq!(
        branch_start.parent_id.as_deref(),
        Some(parallel_start.id.as_str())
    );
}

#[tokio::test]
async fn test_parallel_execution_min_successful_aborts_inflight() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    for _ in 0..6 {
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    }

    let completion = CompletionConfig::new().with_min_successful(1);
    let config = ParallelConfig::new()
        .with_max_concurrency(2)
        .with_completion_config(completion);

    #[derive(Clone, Copy)]
    enum Timing {
        Fast(u32),
        Slow(u32),
    }

    fn make_timed_branch(
        timing: Timing,
    ) -> impl Fn(DurableContextHandle) -> BoxFuture<'static, crate::error::DurableResult<u32>>
           + Send
           + Sync
           + 'static {
        move |_ctx: DurableContextHandle| {
            Box::pin(async move {
                match timing {
                    Timing::Fast(value) => Ok(value),
                    Timing::Slow(value) => {
                        tokio::time::sleep(StdDuration::from_millis(50)).await;
                        Ok(value)
                    }
                }
            })
        }
    }

    let fast = NamedParallelBranch::new(make_timed_branch(Timing::Fast(1))).with_name("fast");
    let slow = NamedParallelBranch::new(make_timed_branch(Timing::Slow(2)));

    let batch: BatchResult<u32> = ctx
        .parallel_named(Some("parallel"), vec![fast, slow], Some(config))
        .await
        .unwrap();

    assert_eq!(
        batch.completion_reason,
        BatchCompletionReason::MinSuccessfulReached
    );
    assert_eq!(batch.succeeded().len(), 1);
    assert_eq!(batch.started().len(), 1);
}

#[tokio::test]
async fn test_parallel_execution_max_concurrency_exceeds_branches() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    for _ in 0..4 {
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    }

    let config = ParallelConfig::new().with_max_concurrency(8);
    let branches = vec![make_parallel_branch(BranchBehavior::Ok(7))];

    let batch: BatchResult<u32> = ctx
        .parallel(Some("parallel"), branches, Some(config))
        .await
        .unwrap();

    assert_eq!(batch.success_count(), 1);
    assert_eq!(batch.values(), vec![7u32]);
}

#[tokio::test]
async fn test_parallel_execution_panicking_branch_returns_join_error() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    for _ in 0..2 {
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    }

    let branches = vec![make_parallel_branch(BranchBehavior::Panic(
        "parallel panic",
    ))];

    let err = ctx
        .parallel(Some("parallel"), branches, None)
        .await
        .expect_err("panic should surface as join error");

    match err {
        DurableError::Internal(message) => {
            assert!(message.contains("Child task join error"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_parallel_failure_tolerance_exceeded() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    for _ in 0..4 {
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    }

    let completion = CompletionConfig::new().with_tolerated_failures(0);
    let config = ParallelConfig::new()
        .with_max_concurrency(1)
        .with_completion_config(completion);

    let branches = vec![
        make_parallel_branch(BranchBehavior::Fail("boom")),
        make_parallel_branch(BranchBehavior::Panic(
            "parallel should stop after failure tolerance exceeded",
        )),
    ];

    let batch: BatchResult<u32> = ctx
        .parallel(Some("parallel"), branches, Some(config))
        .await
        .unwrap();

    assert_eq!(
        batch.completion_reason,
        BatchCompletionReason::FailureToleranceExceeded
    );
    assert_eq!(batch.failure_count(), 1);
}

#[tokio::test]
async fn test_parallel_batch_serdes_serialize_failure_terminates() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    for _ in 0..3 {
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    }

    let config = ParallelConfig::new().with_serdes(Arc::new(BatchSerializeFailSerdes));
    let branches = vec![make_parallel_branch(BranchBehavior::Ok(1))];

    let result = tokio::time::timeout(
        StdDuration::from_millis(50),
        ctx.parallel(Some("parallel"), branches, Some(config)),
    )
    .await;

    assert!(
        result.is_err(),
        "parallel should suspend on batch serdes failure"
    );

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
async fn test_parallel_batch_serdes_deserialize_failure_terminates() {
    let arn = "arn:test:durable";
    let step_id = "parallel_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "SubType": "Parallel",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": "payload" },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let config = ParallelConfig::new().with_serdes(Arc::new(BatchDeserializeFailSerdes));
    let branches = vec![make_parallel_branch(BranchBehavior::Panic(
        "parallel branch should not run in replay",
    ))];

    let result = tokio::time::timeout(
        StdDuration::from_millis(50),
        ctx.parallel(Some("parallel"), branches, Some(config)),
    )
    .await;

    assert!(
        result.is_err(),
        "parallel should suspend on batch serdes failure"
    );

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
async fn test_parallel_replay_missing_child_payload_returns_error() {
    let arn = "arn:test:durable";
    let name = Some("parallel");

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

    let branch_name = "parallel-branch-0".to_string();
    let child_step_id = format!("{}_{}", branch_name, 1);
    let child_hashed_id = CheckpointManager::hash_id(&child_step_id);
    let child_result = serde_json::to_string(&1u32).unwrap();
    let child_op = json!({
        "Id": child_hashed_id,
        "Type": "CONTEXT",
        "SubType": "ParallelBranch",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": child_result },
    });

    let ctx = make_replay_context(arn, vec![par_op, child_op]).await;
    let config = ParallelConfig::new().with_item_serdes(Arc::new(DeserializeNoneSerdes));
    let branches = vec![make_parallel_branch(BranchBehavior::Panic(
        "parallel branch should not run in replay",
    ))];

    let err = ctx
        .parallel(name, branches, Some(config))
        .await
        .expect_err("parallel should fail when child payload is missing");

    match err {
        DurableError::Internal(message) => {
            assert!(message.contains("Missing child context output"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_parallel_replay_batch_serdes_validation_failed() {
    let arn = "arn:test:durable";
    let step_id = "parallel_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "SubType": "Parallel",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": "ignored" },
    });

    let batch_serdes = StaticBatchSerdes {
        items: vec![(2, BatchItemStatus::Succeeded, Some("x".to_string()))],
        completion_reason: BatchCompletionReason::AllCompleted,
    };
    let cfg = ParallelConfig::new().with_serdes(Arc::new(batch_serdes));

    let ctx = make_replay_context(arn, vec![op]).await;
    let branch = |_ctx: DurableContextHandle| async move {
        panic!("branch should not run in replay");
        #[allow(unreachable_code)]
        Ok::<String, DurableError>("".to_string())
    };

    let err = ctx
        .parallel(Some("parallel"), vec![branch, branch], Some(cfg))
        .await
        .expect_err("parallel should fail replay validation");

    match err {
        DurableError::ReplayValidationFailed { expected, .. } => {
            assert!(expected.contains("parallel totalCount"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_parallel_replay_batch_serdes_returns_batch() {
    let arn = "arn:test:durable";
    let step_id = "parallel_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "SubType": "Parallel",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": "ignored" },
    });

    let batch_serdes = StaticBatchSerdes {
        items: vec![
            (0, BatchItemStatus::Succeeded, Some("a".to_string())),
            (1, BatchItemStatus::Succeeded, Some("b".to_string())),
        ],
        completion_reason: BatchCompletionReason::AllCompleted,
    };
    let cfg = ParallelConfig::new().with_serdes(Arc::new(batch_serdes));

    let ctx = make_replay_context(arn, vec![op]).await;
    let branch = |_ctx: DurableContextHandle| async move {
        panic!("branch should not run in replay");
        #[allow(unreachable_code)]
        Ok::<String, DurableError>("".to_string())
    };

    let batch: BatchResult<String> = ctx
        .parallel(Some("parallel"), vec![branch, branch], Some(cfg))
        .await
        .unwrap();

    assert_eq!(batch.success_count(), 2);
    assert_eq!(batch.values(), vec!["a".to_string(), "b".to_string()]);
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

    let batch: BatchResult<String> = ctx.parallel(name, branches, None).await.unwrap();
    assert_eq!(batch.success_count(), 1);
    assert_eq!(batch.failure_count(), 0);
    assert_eq!(batch.values(), vec!["ok".to_string()]);
}

#[tokio::test]
async fn test_parallel_replay_includes_failed_and_started_children() {
    let arn = "arn:test:durable";
    let name = Some("parallel");

    let par_step_id = "parallel_0".to_string();
    let par_hashed_id = CheckpointManager::hash_id(&par_step_id);
    let summary = json!({
        "totalCount": 3,
        "successCount": 1,
        "failureCount": 1,
    })
    .to_string();

    let par_op = json!({
        "Id": par_hashed_id,
        "Type": "CONTEXT",
        "SubType": "Parallel",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": summary },
    });

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

    let branch1_name = "parallel-branch-1".to_string();
    let child1_step_id = format!("{}_{}", branch1_name, 2);
    let child1_hashed_id = CheckpointManager::hash_id(&child1_step_id);
    let child1_op = json!({
        "Id": child1_hashed_id,
        "Type": "CONTEXT",
        "SubType": "ParallelBranch",
        "Status": "FAILED",
        "ContextDetails": {
            "Error": { "ErrorType": "Error", "ErrorMessage": "branch boom" }
        },
    });

    let branch2_name = "parallel-branch-2".to_string();
    let child2_step_id = format!("{}_{}", branch2_name, 3);
    let child2_hashed_id = CheckpointManager::hash_id(&child2_step_id);
    let child2_op = json!({
        "Id": child2_hashed_id,
        "Type": "CONTEXT",
        "SubType": "ParallelBranch",
        "Status": "STARTED",
    });

    let ctx = make_replay_context(arn, vec![par_op, child0_op, child1_op, child2_op]).await;
    let branch = |_ctx: DurableContextHandle| async move {
        panic!("branch should not run in replay");
        #[allow(unreachable_code)]
        Ok::<String, DurableError>("".to_string())
    };

    let batch: BatchResult<String> = ctx
        .parallel(name, vec![branch, branch, branch], None)
        .await
        .unwrap();

    assert_eq!(batch.succeeded().len(), 1);
    assert_eq!(batch.failed().len(), 1);
    assert_eq!(batch.started().len(), 1);
    let err = batch.first_error().expect("failed item");
    assert!(err.to_string().contains("branch boom"));
}

#[tokio::test]
async fn test_parallel_replay_missing_op_executes_empty_branches() {
    let arn = "arn:test:durable";
    let lambda_service = Arc::new(MockLambdaService::new());

    for _ in 0..2 {
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    }

    let ctx = make_replay_context_with_service(arn, vec![], lambda_service).await;
    type BranchFn =
        fn(DurableContextHandle) -> BoxFuture<'static, crate::error::DurableResult<u32>>;
    let branches: Vec<NamedParallelBranch<BranchFn>> = Vec::new();

    let batch: BatchResult<u32> = ctx
        .parallel_named(Some("parallel"), branches, None)
        .await
        .unwrap();

    assert!(batch.all.is_empty());
    assert_eq!(batch.completion_reason, BatchCompletionReason::AllCompleted);
}
