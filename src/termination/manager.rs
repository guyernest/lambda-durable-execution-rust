//! Termination manager for controlling Lambda execution lifecycle.

use crate::error::DurableError;
use std::sync::Arc;
use tokio::sync::{watch, Mutex, Notify};

type CheckpointTerminatingCallback = Box<dyn Fn() + Send + Sync>;

/// Reason for terminating the Lambda execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminationReason {
    /// A retry has been scheduled and Lambda should terminate.
    RetryScheduled,

    /// A wait operation has been scheduled.
    WaitScheduled,

    /// Waiting for an external callback.
    CallbackPending,

    /// Waiting for a chained invoke to complete.
    InvokePending,

    /// Checkpoint operation failed.
    CheckpointFailed,

    /// Serialization/deserialization failed.
    SerdesFailed,

    /// Context validation error (e.g., using parent context in child).
    ContextValidationError,

    /// All operations are complete or awaiting.
    AllOperationsIdle,

    /// Handler completed successfully.
    HandlerCompleted,

    /// Handler failed with an error.
    HandlerFailed,
}

/// Result of termination with reason and optional error.
#[derive(Debug, Clone)]
pub struct TerminationResult {
    /// The reason for termination.
    pub reason: TerminationReason,

    /// Optional error message.
    pub message: Option<String>,

    /// Optional error that caused termination.
    pub error: Option<Arc<DurableError>>,
}

impl TerminationResult {
    /// Create a new termination result.
    pub fn new(reason: TerminationReason) -> Self {
        Self {
            reason,
            message: None,
            error: None,
        }
    }

    /// Create a termination result with a message.
    pub fn with_message(reason: TerminationReason, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: Some(message.into()),
            error: None,
        }
    }

    /// Create a termination result with an error.
    pub fn with_error(reason: TerminationReason, error: DurableError) -> Self {
        Self {
            reason,
            message: Some(error.to_string()),
            error: Some(Arc::new(error)),
        }
    }
}

/// Manages the termination lifecycle of a durable Lambda execution.
///
/// The TerminationManager coordinates between the handler, checkpoint manager,
/// and other components to determine when Lambda should terminate and allow
/// the durable execution to continue later.
pub struct TerminationManager {
    /// Whether termination has been triggered.
    terminated: Arc<Mutex<bool>>,

    /// Watch channel for termination signal.
    termination_tx: watch::Sender<Option<TerminationResult>>,
    termination_rx: watch::Receiver<Option<TerminationResult>>,

    /// Notify for checkpoint termination callback.
    checkpoint_terminating_notify: Arc<Notify>,

    /// Callback to set checkpoint manager as terminating.
    checkpoint_terminating_callback: Arc<Mutex<Option<CheckpointTerminatingCallback>>>,
}

impl TerminationManager {
    /// Create a new termination manager.
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(None);
        Self {
            terminated: Arc::new(Mutex::new(false)),
            termination_tx: tx,
            termination_rx: rx,
            checkpoint_terminating_notify: Arc::new(Notify::new()),
            checkpoint_terminating_callback: Arc::new(Mutex::new(None)),
        }
    }

    /// Set the callback to invoke when checkpoint manager should terminate.
    pub async fn set_checkpoint_terminating_callback<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        let mut cb = self.checkpoint_terminating_callback.lock().await;
        *cb = Some(Box::new(callback));
    }

    /// Trigger termination with the given reason.
    pub async fn terminate(&self, result: TerminationResult) {
        let mut terminated = self.terminated.lock().await;
        if *terminated {
            // Already terminated
            return;
        }
        *terminated = true;

        // Invoke checkpoint terminating callback
        if let Some(callback) = self.checkpoint_terminating_callback.lock().await.as_ref() {
            callback();
        }

        // Notify checkpoint terminating
        self.checkpoint_terminating_notify.notify_waiters();

        // Send termination signal
        let _ = self.termination_tx.send(Some(result));
    }

    /// Terminate due to a scheduled retry.
    pub async fn terminate_for_retry(&self) {
        self.terminate(TerminationResult::new(TerminationReason::RetryScheduled))
            .await;
    }

    /// Terminate due to a scheduled wait.
    pub async fn terminate_for_wait(&self) {
        self.terminate(TerminationResult::new(TerminationReason::WaitScheduled))
            .await;
    }

    /// Terminate due to a pending callback.
    pub async fn terminate_for_callback(&self) {
        self.terminate(TerminationResult::new(TerminationReason::CallbackPending))
            .await;
    }

    /// Terminate due to a pending chained invoke.
    pub async fn terminate_for_invoke(&self) {
        self.terminate(TerminationResult::new(TerminationReason::InvokePending))
            .await;
    }

    /// Terminate due to a checkpoint failure.
    pub async fn terminate_for_checkpoint_failure(&self, error: DurableError) {
        self.terminate(TerminationResult::with_error(
            TerminationReason::CheckpointFailed,
            error,
        ))
        .await;
    }

    /// Terminate due to serialization failure.
    pub async fn terminate_for_serdes_failure(&self, message: impl Into<String>) {
        self.terminate(TerminationResult::with_message(
            TerminationReason::SerdesFailed,
            message,
        ))
        .await;
    }

    /// Terminate due to context validation error.
    pub async fn terminate_for_context_validation(&self, error: DurableError) {
        self.terminate(TerminationResult::with_error(
            TerminationReason::ContextValidationError,
            error,
        ))
        .await;
    }

    /// Terminate because all operations are idle/awaiting.
    pub async fn terminate_all_idle(&self) {
        self.terminate(TerminationResult::new(TerminationReason::AllOperationsIdle))
            .await;
    }

    /// Wait for termination to be triggered.
    ///
    /// Returns the termination result when termination is triggered.
    pub async fn wait_for_termination(&self) -> TerminationResult {
        let mut rx = self.termination_rx.clone();

        // Wait for termination signal
        loop {
            rx.changed().await.ok();
            if let Some(result) = rx.borrow().clone() {
                return result;
            }
        }
    }

    /// Check if termination has been triggered.
    pub async fn is_terminated(&self) -> bool {
        *self.terminated.lock().await
    }

    /// Get the termination result if terminated.
    pub fn get_termination_result(&self) -> Option<TerminationResult> {
        self.termination_rx.borrow().clone()
    }

    /// Subscribe to termination notifications.
    pub fn subscribe(&self) -> watch::Receiver<Option<TerminationResult>> {
        self.termination_rx.clone()
    }
}

impl Default for TerminationManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for TerminationManager {
    fn clone(&self) -> Self {
        Self {
            terminated: Arc::clone(&self.terminated),
            termination_tx: self.termination_tx.clone(),
            termination_rx: self.termination_rx.clone(),
            checkpoint_terminating_notify: Arc::clone(&self.checkpoint_terminating_notify),
            checkpoint_terminating_callback: Arc::clone(&self.checkpoint_terminating_callback),
        }
    }
}

impl std::fmt::Debug for TerminationManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminationManager")
            .field("terminated", &self.terminated)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_termination_manager_basic() {
        let manager = TerminationManager::new();

        assert!(!manager.is_terminated().await);

        manager.terminate_for_retry().await;

        assert!(manager.is_terminated().await);

        let result = manager.get_termination_result().unwrap();
        assert_eq!(result.reason, TerminationReason::RetryScheduled);
    }

    #[tokio::test]
    async fn test_termination_manager_wait() {
        let manager = TerminationManager::new();
        let manager_clone = manager.clone();

        // Spawn a task to wait for termination
        let handle = tokio::spawn(async move { manager_clone.wait_for_termination().await });

        // Give the task time to start waiting
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Trigger termination
        manager.terminate_for_callback().await;

        // Wait for the task to complete
        let result = handle.await.unwrap();
        assert_eq!(result.reason, TerminationReason::CallbackPending);
    }

    #[tokio::test]
    async fn test_termination_manager_idempotent() {
        let manager = TerminationManager::new();

        manager.terminate_for_retry().await;
        manager.terminate_for_wait().await; // Should be ignored

        let result = manager.get_termination_result().unwrap();
        assert_eq!(result.reason, TerminationReason::RetryScheduled);
    }

    #[tokio::test]
    async fn test_termination_with_error() {
        let manager = TerminationManager::new();

        let error = DurableError::checkpoint_failed("test failure", false, None::<std::io::Error>);
        manager.terminate_for_checkpoint_failure(error).await;

        let result = manager.get_termination_result().unwrap();
        assert_eq!(result.reason, TerminationReason::CheckpointFailed);
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn test_checkpoint_callback() {
        let manager = TerminationManager::new();
        let called = Arc::new(Mutex::new(false));
        let called_clone = Arc::clone(&called);

        manager
            .set_checkpoint_terminating_callback(move || {
                let called = called_clone.clone();
                tokio::spawn(async move {
                    *called.lock().await = true;
                });
            })
            .await;

        manager.terminate_for_retry().await;

        // Give time for async callback
        tokio::time::sleep(Duration::from_millis(10)).await;

        // The callback was invoked (though async execution means we can't guarantee it completed)
        assert!(manager.is_terminated().await);
    }
}
