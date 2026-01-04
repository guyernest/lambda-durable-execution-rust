use self::helpers::make_execution_context;
use super::*;
use crate::mock::MockLambdaService;
use crate::types::{
    BatchCompletionReason, CompletionConfig, DurableExecutionInvocationInput, DurableLogData,
    DurableLogLevel, DurableLogger,
};
use serde_json::json;
use std::sync::{Arc, Mutex};

mod callback;
mod child;
mod helpers;
mod invoke;
mod map;
mod parallel;
mod step;
mod wait;
mod wait_condition;

#[derive(Default)]
struct RecordingLogger {
    entries: Mutex<Vec<(DurableLogLevel, DurableLogData, String)>>,
}

impl RecordingLogger {
    fn entries(&self) -> Vec<(DurableLogLevel, DurableLogData, String)> {
        self.entries.lock().expect("entries mutex").clone()
    }
}

impl DurableLogger for RecordingLogger {
    fn log(
        &self,
        level: DurableLogLevel,
        data: &DurableLogData,
        message: &str,
        _fields: Option<&[(&'static str, String)]>,
    ) {
        self.entries.lock().expect("entries mutex").push((
            level,
            data.clone(),
            message.to_string(),
        ));
    }
}

#[test]
fn test_validate_completion_config_ok() {
    let config = CompletionConfig::new()
        .with_min_successful(1)
        .with_tolerated_failures(1)
        .with_tolerated_failure_percentage(50.0);

    validate_completion_config(&config, 2, "parallel").expect("valid config");
}

#[test]
fn test_validate_completion_config_min_successful_exceeds() {
    let config = CompletionConfig::new().with_min_successful(3);
    let err = validate_completion_config(&config, 2, "parallel").expect_err("error");

    match err {
        DurableError::InvalidConfiguration { message } => {
            assert!(message.contains("min_successful"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn test_validate_completion_config_failure_count_exceeds() {
    let config = CompletionConfig::new().with_tolerated_failures(4);
    let err = validate_completion_config(&config, 2, "parallel").expect_err("error");

    match err {
        DurableError::InvalidConfiguration { message } => {
            assert!(message.contains("tolerated_failure_count"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn test_validate_completion_config_percentage_invalid() {
    let config = CompletionConfig::new().with_tolerated_failure_percentage(-1.0);
    assert!(validate_completion_config(&config, 2, "parallel").is_err());

    let config = CompletionConfig::new().with_tolerated_failure_percentage(101.0);
    assert!(validate_completion_config(&config, 2, "parallel").is_err());

    let config = CompletionConfig::new().with_tolerated_failure_percentage(f64::NAN);
    assert!(validate_completion_config(&config, 2, "parallel").is_err());
}

#[test]
fn test_should_continue_batch_default_behavior() {
    let config = CompletionConfig::new();
    assert!(should_continue_batch(0, 3, &config));
    assert!(!should_continue_batch(1, 3, &config));
}

#[test]
fn test_should_continue_batch_tolerated_failure_count() {
    let config = CompletionConfig::new().with_tolerated_failures(1);
    assert!(should_continue_batch(1, 5, &config));
    assert!(!should_continue_batch(2, 5, &config));
}

#[test]
fn test_should_continue_batch_tolerated_failure_percentage() {
    let config = CompletionConfig::new().with_tolerated_failure_percentage(50.0);
    assert!(should_continue_batch(2, 4, &config));
    assert!(!should_continue_batch(3, 4, &config));
}

#[test]
fn test_compute_batch_completion_reason_min_successful() {
    let config = CompletionConfig::new().with_min_successful(2);
    let reason = compute_batch_completion_reason(1, 2, 3, 5, &config);
    assert_eq!(reason, BatchCompletionReason::MinSuccessfulReached);
}

#[test]
fn test_compute_batch_completion_reason_failure_tolerance_exceeded() {
    let config = CompletionConfig::new().with_tolerated_failures(0);
    let reason = compute_batch_completion_reason(1, 0, 1, 3, &config);
    assert_eq!(reason, BatchCompletionReason::FailureToleranceExceeded);
}

#[test]
fn test_batch_completion_strings() {
    assert_eq!(
        batch_completion_reason_str(BatchCompletionReason::AllCompleted),
        "ALL_COMPLETED"
    );
    assert_eq!(
        batch_completion_reason_str(BatchCompletionReason::MinSuccessfulReached),
        "MIN_SUCCESSFUL_REACHED"
    );
    assert_eq!(
        batch_completion_reason_str(BatchCompletionReason::FailureToleranceExceeded),
        "FAILURE_TOLERANCE_EXCEEDED"
    );
    assert_eq!(batch_status_str(0), "SUCCEEDED");
    assert_eq!(batch_status_str(1), "FAILED");
}

#[test]
fn test_map_summary_payload_fields() {
    let payload = map_summary_payload(3, 2, 1, BatchCompletionReason::FailureToleranceExceeded);
    assert_eq!(payload["type"], "MapResult");
    assert_eq!(payload["totalCount"], 3);
    assert_eq!(payload["successCount"], 2);
    assert_eq!(payload["failureCount"], 1);
    assert_eq!(payload["completionReason"], "FAILURE_TOLERANCE_EXCEEDED");
    assert_eq!(payload["status"], "FAILED");
}

#[tokio::test]
async fn test_durable_context_handle_debug_and_accessors() {
    let arn = "arn:test:durable";
    let (ctx, _lambda_service) = make_execution_context(arn).await;

    assert_eq!(ctx.execution_context().durable_execution_arn, arn);

    let debug = format!("{ctx:?}");
    assert!(debug.contains("DurableContextHandle"));

    let exec_ctx = ctx.execution_context().clone();
    let impl_ctx = DurableContextImpl::new(exec_ctx);
    let impl_debug = format!("{impl_ctx:?}");
    assert!(impl_debug.contains("DurableContextImpl"));
}

#[tokio::test]
async fn test_context_logger_records_execution_logs() {
    let arn = "arn:test:durable";
    let input_json = helpers::create_replay_input(arn, &json!({}), vec![]);
    let input: DurableExecutionInvocationInput =
        serde_json::from_value(input_json).expect("valid invocation input");
    let lambda_service = Arc::new(MockLambdaService::new());
    let recording = Arc::new(RecordingLogger::default());
    let logger: Arc<dyn DurableLogger> = recording.clone();

    let exec_ctx = ExecutionContext::new(&input, lambda_service, Some(logger), true)
        .await
        .expect("execution context should initialize");
    let ctx = DurableContextHandle::new(Arc::new(DurableContextImpl::new(exec_ctx)));

    ctx.logger().info("hello");

    let entries = recording.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].1.durable_execution_arn, arn);
}

#[tokio::test]
async fn test_context_logger_suppresses_replay_logs() {
    let arn = "arn:test:durable";
    let operations = vec![json!({
        "Id": "step-1",
        "Type": "STEP",
        "Status": "SUCCEEDED"
    })];
    let input_json = helpers::create_replay_input(arn, &json!({}), operations);
    let input: DurableExecutionInvocationInput =
        serde_json::from_value(input_json).expect("valid invocation input");
    let lambda_service = Arc::new(MockLambdaService::new());
    let recording = Arc::new(RecordingLogger::default());
    let logger: Arc<dyn DurableLogger> = recording.clone();

    let exec_ctx = ExecutionContext::new(&input, lambda_service, Some(logger), true)
        .await
        .expect("execution context should initialize");
    let ctx = DurableContextHandle::new(Arc::new(DurableContextImpl::new(exec_ctx)));

    ctx.logger().info("should-suppress");

    let entries = recording.entries();
    assert!(entries.is_empty());
}

#[tokio::test]
async fn test_context_logger_includes_parent_operation_id() {
    let arn = "arn:test:durable";
    let input_json = helpers::create_replay_input(arn, &json!({}), vec![]);
    let input: DurableExecutionInvocationInput =
        serde_json::from_value(input_json).expect("valid invocation input");
    let lambda_service = Arc::new(MockLambdaService::new());
    let recording = Arc::new(RecordingLogger::default());
    let logger: Arc<dyn DurableLogger> = recording.clone();

    let exec_ctx = ExecutionContext::new(&input, lambda_service, Some(logger), true)
        .await
        .expect("execution context should initialize");
    exec_ctx.set_parent_id(Some("parent-op".to_string())).await;
    let ctx = DurableContextHandle::new(Arc::new(DurableContextImpl::new(exec_ctx)));

    ctx.logger().info("parent");

    let entries = recording.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].1.operation_id.as_deref(), Some("parent-op"));
}

#[test]
fn test_hash_id_matches_checkpoint_manager() {
    let id = "step_1";
    assert_eq!(
        DurableContextImpl::hash_id(id),
        CheckpointManager::hash_id(id)
    );
}

#[test]
fn test_parallel_summary_payload_fields() {
    let payload = parallel_summary_payload(4, 3, 1, 2, BatchCompletionReason::MinSuccessfulReached);
    assert_eq!(payload["type"], "ParallelResult");
    assert_eq!(payload["totalCount"], 4);
    assert_eq!(payload["successCount"], 3);
    assert_eq!(payload["failureCount"], 1);
    assert_eq!(payload["startedCount"], 2);
    assert_eq!(payload["completionReason"], "MIN_SUCCESSFUL_REACHED");
    assert_eq!(payload["status"], "FAILED");
}
