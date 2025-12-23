use super::helpers::*;

#[tokio::test]
async fn test_map_empty_items_with_batch_serdes() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let batch_serdes = Arc::new(StaticBatchSerdes::<u32> {
        items: Vec::new(),
        completion_reason: BatchCompletionReason::AllCompleted,
    });
    let config = MapConfig::new().with_serdes(batch_serdes);

    let batch: BatchResult<u32> = ctx
        .map(
            Some("map"),
            Vec::<u32>::new(),
            |_item, _child_ctx, _idx| async move {
                panic!("map_fn should not run for empty items");
                #[allow(unreachable_code)]
                Ok::<u32, crate::error::DurableError>(0)
            },
            Some(config),
        )
        .await
        .unwrap();

    assert!(batch.all.is_empty());
    assert_eq!(batch.completion_reason, BatchCompletionReason::AllCompleted);

    let updates: Vec<_> = lambda_service
        .checkpoint_calls()
        .into_iter()
        .flat_map(|call| call.updates)
        .collect();
    assert!(updates.iter().any(|update| {
        update.operation_type == OperationType::Context
            && update.action == OperationAction::Succeed
            && update.payload.as_deref() == Some("payload")
    }));
}

#[tokio::test]
async fn test_map_batch_serdes_serialize_failure_terminates() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    lambda_service.expect_checkpoint(MockCheckpointConfig::default());

    let config = MapConfig::new().with_serdes(Arc::new(BatchSerializeFailSerdes));
    let result = tokio::time::timeout(
        StdDuration::from_millis(50),
        ctx.map(
            Some("map"),
            Vec::<u32>::new(),
            |_item, _child_ctx, _idx| async move {
                panic!("map_fn should not run for empty items");
                #[allow(unreachable_code)]
                Ok::<u32, DurableError>(0)
            },
            Some(config),
        ),
    )
    .await;

    assert!(
        result.is_err(),
        "map should suspend on batch serdes failure"
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
async fn test_map_batch_serdes_deserialize_failure_terminates() {
    let arn = "arn:test:durable";
    let step_id = "map_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "SubType": "Map",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": "payload" },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let config = MapConfig::new().with_serdes(Arc::new(BatchDeserializeFailSerdes));
    let result = tokio::time::timeout(
        StdDuration::from_millis(50),
        ctx.map(
            Some("map"),
            vec![1u32],
            |_item, _child_ctx, _idx| async move {
                panic!("map_fn should not run in replay");
                #[allow(unreachable_code)]
                Ok::<u32, DurableError>(0)
            },
            Some(config),
        ),
    )
    .await;

    assert!(
        result.is_err(),
        "map should suspend on batch serdes failure"
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
async fn test_map_replay_missing_child_payload_returns_error() {
    let arn = "arn:test:durable";
    let name = Some("map");

    let map_step_id = "map_0".to_string();
    let map_hashed_id = CheckpointManager::hash_id(&map_step_id);
    let summary = json!({
        "totalCount": 1,
        "successCount": 1,
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

    let item_name = "map-item-0".to_string();
    let child_step_id = format!("{}_{}", item_name, 1);
    let child_hashed_id = CheckpointManager::hash_id(&child_step_id);
    let child_result = serde_json::to_string(&1u32).unwrap();
    let child_op = json!({
        "Id": child_hashed_id,
        "Type": "CONTEXT",
        "SubType": "MapIteration",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": child_result },
    });

    let ctx = make_replay_context(arn, vec![map_op, child_op]).await;
    let config = MapConfig::new().with_item_serdes(Arc::new(DeserializeNoneSerdes));
    let err = ctx
        .map(
            name,
            vec![1u32],
            |_item, _child_ctx, _idx| async move {
                panic!("map_fn should not run in replay");
                #[allow(unreachable_code)]
                Ok::<u32, DurableError>(0)
            },
            Some(config),
        )
        .await
        .expect_err("map should fail when child payload is missing");

    match err {
        DurableError::Internal(message) => {
            assert!(message.contains("Missing child context output"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_map_min_successful_completes_early() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    for _ in 0..4 {
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    }

    let completion = CompletionConfig::new().with_min_successful(1);
    let config = MapConfig::new()
        .with_max_concurrency(1)
        .with_completion_config(completion);

    let batch: BatchResult<u32> = ctx
        .map(
            Some("map"),
            vec![1u32, 2u32, 3u32],
            |item, _child_ctx, idx| async move {
                if idx == 0 {
                    Ok::<u32, DurableError>(item + 1)
                } else {
                    panic!("map should stop after min_successful");
                }
            },
            Some(config),
        )
        .await
        .unwrap();

    assert_eq!(
        batch.completion_reason,
        BatchCompletionReason::MinSuccessfulReached
    );
    assert_eq!(batch.success_count(), 1);
}

#[tokio::test]
async fn test_map_failure_tolerance_exceeded() {
    let arn = "arn:test:durable";
    let (ctx, lambda_service) = make_execution_context(arn).await;

    for _ in 0..4 {
        lambda_service.expect_checkpoint(MockCheckpointConfig::default());
    }

    let completion = CompletionConfig::new().with_tolerated_failures(0);
    let config = MapConfig::new()
        .with_max_concurrency(1)
        .with_completion_config(completion);

    let batch: BatchResult<u32> = ctx
        .map(
            Some("map"),
            vec![1u32, 2u32],
            |_item, _child_ctx, idx| async move {
                if idx == 0 {
                    Err(DurableError::Internal("boom".to_string()))
                } else {
                    panic!("map should stop after failure tolerance exceeded");
                }
            },
            Some(config),
        )
        .await
        .unwrap();

    assert_eq!(
        batch.completion_reason,
        BatchCompletionReason::FailureToleranceExceeded
    );
    assert_eq!(batch.failure_count(), 1);
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
    let batch: BatchResult<u32> = ctx
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
async fn test_map_replay_failed_returns_error() {
    let arn = "arn:test:durable";
    let step_id = "map_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "SubType": "Map",
        "Status": "FAILED",
        "ContextDetails": {
            "Error": { "ErrorType": "Error", "ErrorMessage": "map boom" }
        },
    });

    let ctx = make_replay_context(arn, vec![op]).await;
    let err = ctx
        .map(
            Some("map"),
            vec![1u32],
            |_item, _child_ctx, _idx| async move {
                panic!("map_fn should not run in replay");
                #[allow(unreachable_code)]
                Ok::<u32, DurableError>(0)
            },
            None,
        )
        .await
        .expect_err("map should fail in replay");

    match err {
        DurableError::BatchOperationFailed { message, .. } => {
            assert!(message.contains("map boom"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_map_replay_batch_serdes_validation_failed() {
    let arn = "arn:test:durable";
    let step_id = "map_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "SubType": "Map",
        "Status": "SUCCEEDED",
        "ContextDetails": { "Result": "ignored" },
    });

    let batch_serdes = StaticBatchSerdes {
        items: vec![(2, BatchItemStatus::Succeeded, Some("x".to_string()))],
        completion_reason: BatchCompletionReason::AllCompleted,
    };

    let cfg = MapConfig::new().with_serdes(Arc::new(batch_serdes));

    let ctx = make_replay_context(arn, vec![op]).await;
    let err = ctx
        .map(
            Some("map"),
            vec![1u32, 2u32],
            |_item, _child_ctx, _idx| async move {
                panic!("map_fn should not run in replay");
                #[allow(unreachable_code)]
                Ok::<String, DurableError>("".to_string())
            },
            Some(cfg),
        )
        .await
        .expect_err("map should fail replay validation");

    match err {
        DurableError::ReplayValidationFailed { expected, .. } => {
            assert!(expected.contains("map totalCount"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_map_replay_batch_serdes_returns_batch() {
    let arn = "arn:test:durable";
    let step_id = "map_0".to_string();
    let hashed_id = CheckpointManager::hash_id(&step_id);
    let op = json!({
        "Id": hashed_id,
        "Type": "CONTEXT",
        "SubType": "Map",
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

    let cfg = MapConfig::new().with_serdes(Arc::new(batch_serdes));

    let ctx = make_replay_context(arn, vec![op]).await;
    let batch: BatchResult<String> = ctx
        .map(
            Some("map"),
            vec![1u32, 2u32],
            |_item, _child_ctx, _idx| async move {
                panic!("map_fn should not run in replay");
                #[allow(unreachable_code)]
                Ok::<String, DurableError>("".to_string())
            },
            Some(cfg),
        )
        .await
        .unwrap();

    assert_eq!(batch.success_count(), 2);
    assert_eq!(batch.values(), vec!["a".to_string(), "b".to_string()]);
}
