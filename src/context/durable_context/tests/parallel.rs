use super::helpers::*;

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
