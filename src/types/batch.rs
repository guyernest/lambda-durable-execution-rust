//! Batch result types for `map()` and `parallel()`.
//!
//! These mirror the JS/Python SDK concurrency models and are shared across
//! configuration (`serdes`) and runtime result handling.

use crate::error::{DurableError, DurableResult};
use std::sync::Arc;

/// Status of a batch item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchItemStatus {
    /// Item completed successfully.
    Succeeded,
    /// Item failed in its child context.
    Failed,
    /// Item was started but not completed (e.g., early completion).
    Started,
}

/// A single item in a batch result.
#[derive(Debug)]
pub struct BatchItem<T> {
    /// Index of the item in the original batch.
    pub index: usize,
    /// Execution status.
    pub status: BatchItemStatus,
    /// Result for successful items.
    pub result: Option<T>,
    /// Error for failed items.
    pub error: Option<Arc<DurableError>>,
}

/// Reason why a batch completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchCompletionReason {
    /// All started items completed.
    AllCompleted,
    /// The configured minimum successful count was reached.
    MinSuccessfulReached,
    /// Failure tolerance was exceeded.
    FailureToleranceExceeded,
}

/// Result type for batch operations (`parallel` and `map`).
///
/// Contains successful, failed, and started (incomplete) items.
#[derive(Debug)]
pub struct BatchResult<T> {
    /// All items in the batch.
    pub all: Vec<BatchItem<T>>,
    /// Completion reason.
    pub completion_reason: BatchCompletionReason,
}

impl<T> BatchResult<T> {
    /// Get only the successful items.
    pub fn succeeded(&self) -> Vec<&BatchItem<T>> {
        self.all
            .iter()
            .filter(|i| i.status == BatchItemStatus::Succeeded)
            .collect()
    }

    /// Get only the failed items.
    pub fn failed(&self) -> Vec<&BatchItem<T>> {
        self.all
            .iter()
            .filter(|i| i.status == BatchItemStatus::Failed)
            .collect()
    }

    /// Get only started (incomplete) items.
    pub fn started(&self) -> Vec<&BatchItem<T>> {
        self.all
            .iter()
            .filter(|i| i.status == BatchItemStatus::Started)
            .collect()
    }

    /// Overall batch status.
    ///
    /// Returns:
    /// - `Failed` if any item failed
    /// - `Started` if no failures but some items are incomplete
    /// - `Succeeded` if all items completed successfully
    pub fn status(&self) -> BatchItemStatus {
        if self.has_failure() {
            BatchItemStatus::Failed
        } else if self
            .all
            .iter()
            .any(|i| i.status == BatchItemStatus::Started)
        {
            BatchItemStatus::Started
        } else {
            BatchItemStatus::Succeeded
        }
    }

    /// Whether any item failed.
    pub fn has_failure(&self) -> bool {
        self.all.iter().any(|i| i.status == BatchItemStatus::Failed)
    }

    /// Compatibility alias for previous API.
    pub fn all_succeeded(&self) -> bool {
        !self.has_failure()
    }

    /// Throw the first failure if any.
    ///
    /// Returns a `BatchOperationFailed` error that wraps information about the
    /// failed item. Use [`errors()`](Self::errors) to get the original errors
    /// if you need to inspect them directly.
    pub fn throw_if_error(&self) -> DurableResult<()> {
        if let Some(item) = self
            .all
            .iter()
            .find(|i| i.status == BatchItemStatus::Failed)
        {
            if let Some(err) = item.error.as_ref() {
                return Err(DurableError::BatchOperationFailed {
                    name: format!("batch_item_{}", item.index),
                    message: err.to_string(),
                    successful_count: self.success_count(),
                    failed_count: self.failure_count(),
                });
            }
        }
        Ok(())
    }

    /// Get the first error if any, returning a reference to the original error.
    ///
    /// This preserves the original error type, unlike [`throw_if_error()`](Self::throw_if_error).
    pub fn first_error(&self) -> Option<&Arc<DurableError>> {
        self.all
            .iter()
            .find(|i| i.status == BatchItemStatus::Failed)
            .and_then(|item| item.error.as_ref())
    }

    /// Get all successful values in order, consuming the result.
    pub fn values(self) -> Vec<T> {
        let mut items: Vec<_> = self
            .all
            .into_iter()
            .filter_map(|i| i.result.map(|r| (i.index, r)))
            .collect();
        items.sort_by_key(|(i, _)| *i);
        items.into_iter().map(|(_, v)| v).collect()
    }

    /// Get all errors with their indices.
    pub fn errors(&self) -> Vec<(usize, &DurableError)> {
        self.all
            .iter()
            .filter_map(|i| i.error.as_ref().map(|e| (i.index, e.as_ref())))
            .collect()
    }

    /// Number of successful items.
    pub fn success_count(&self) -> usize {
        self.all
            .iter()
            .filter(|i| i.status == BatchItemStatus::Succeeded)
            .count()
    }

    /// Number of failed items.
    pub fn failure_count(&self) -> usize {
        self.all
            .iter()
            .filter(|i| i.status == BatchItemStatus::Failed)
            .count()
    }

    /// Number of started but incomplete items.
    pub fn started_count(&self) -> usize {
        self.all
            .iter()
            .filter(|i| i.status == BatchItemStatus::Started)
            .count()
    }

    /// Total number of items in the batch.
    pub fn total_count(&self) -> usize {
        self.all.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_batch() -> BatchResult<i32> {
        BatchResult {
            all: vec![
                BatchItem {
                    index: 0,
                    status: BatchItemStatus::Succeeded,
                    result: Some(10),
                    error: None,
                },
                BatchItem {
                    index: 2,
                    status: BatchItemStatus::Failed,
                    result: None,
                    error: Some(Arc::new(DurableError::Internal("boom".to_string()))),
                },
                BatchItem {
                    index: 1,
                    status: BatchItemStatus::Started,
                    result: None,
                    error: None,
                },
            ],
            completion_reason: BatchCompletionReason::FailureToleranceExceeded,
        }
    }

    #[test]
    fn test_batch_filters_and_counts() {
        let batch = sample_batch();

        assert_eq!(batch.succeeded().len(), 1);
        assert_eq!(batch.failed().len(), 1);
        assert_eq!(batch.started().len(), 1);

        assert_eq!(batch.success_count(), 1);
        assert_eq!(batch.failure_count(), 1);
        assert_eq!(batch.started_count(), 1);
        assert_eq!(batch.total_count(), 3);
    }

    #[test]
    fn test_batch_status_and_all_succeeded() {
        let batch = sample_batch();
        assert_eq!(batch.status(), BatchItemStatus::Failed);
        assert!(!batch.all_succeeded());

        let batch = BatchResult {
            all: vec![BatchItem {
                index: 0,
                status: BatchItemStatus::Succeeded,
                result: Some(1),
                error: None,
            }],
            completion_reason: BatchCompletionReason::AllCompleted,
        };
        assert_eq!(batch.status(), BatchItemStatus::Succeeded);
        assert!(batch.all_succeeded());
    }

    #[test]
    fn test_batch_values_sorted_and_errors() {
        let batch = sample_batch();
        let errors = batch.errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, 2);

        let values = sample_batch().values();
        assert_eq!(values, vec![10]);
    }

    #[test]
    fn test_batch_throw_if_error() {
        let batch = sample_batch();
        let err = batch.throw_if_error().expect_err("should error");

        match err {
            DurableError::BatchOperationFailed {
                successful_count,
                failed_count,
                ..
            } => {
                assert_eq!(successful_count, 1);
                assert_eq!(failed_count, 1);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
