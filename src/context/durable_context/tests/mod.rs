use super::*;
use crate::types::CompletionConfig;

mod callback;
mod child;
mod helpers;
mod invoke;
mod map;
mod parallel;
mod step;
mod wait;
mod wait_condition;

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
