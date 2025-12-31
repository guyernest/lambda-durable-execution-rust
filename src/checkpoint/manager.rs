//! Checkpoint manager for persisting operation state.

use crate::error::{DurableError, DurableResult};
use crate::termination::TerminationManager;
use crate::types::{
    LambdaService, Operation, OperationAction, OperationStatus, OperationType, OperationUpdate,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};
use tracing::{debug, error, info, warn};

mod coalesce;
mod hash;
mod lifecycle;
mod queue;

/// Maximum payload size for a checkpoint batch (750KB).
///
/// This is a conservative batching limit aligned with the JS/Python SDKs and is
/// not an AWS-documented service quota. It helps avoid oversized checkpoint
/// requests once JSON overhead is included.
const MAX_PAYLOAD_SIZE: usize = 750 * 1024;

/// Cooldown period before termination to ensure queue completion.
const TERMINATION_COOLDOWN_MS: u64 = 50;

/// Lifecycle state of an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationLifecycle {
    /// Operation has not started.
    NotStarted,
    /// Operation is currently executing.
    Executing,
    /// Operation is waiting for retry.
    RetryWaiting,
    /// Operation is idle and not awaited.
    IdleNotAwaited,
    /// Operation is idle and awaited.
    IdleAwaited,
    /// Operation has completed (success or failure).
    Completed,
}

/// Information about an operation being tracked.
#[derive(Debug, Clone)]
pub struct OperationInfo {
    /// Operation ID.
    pub id: String,
    /// Parent operation ID.
    pub parent_id: Option<String>,
    /// Operation type.
    pub operation_type: OperationType,
    /// Current lifecycle state.
    pub lifecycle: OperationLifecycle,
    /// Whether this operation has been awaited.
    pub awaited: bool,
}

/// A queued checkpoint operation.
#[derive(Debug)]
struct QueuedCheckpoint {
    /// Step/operation ID.
    step_id: String,
    /// The update to send.
    update: OperationUpdate,
    /// Notification when this checkpoint completes.
    notify: Arc<Notify>,
}

/// Manages checkpointing of operation state to the durable execution backend.
///
/// The CheckpointManager handles:
/// - Batching multiple operation updates into single API calls
/// - Queue management with size limits (750KB per batch)
/// - Operation lifecycle tracking for termination decisions
/// - Integration with the TerminationManager
pub struct CheckpointManager {
    /// ARN of the durable execution.
    durable_execution_arn: String,

    /// Lambda service for API calls.
    lambda_service: Arc<dyn LambdaService>,

    /// Termination manager reference.
    termination_manager: Arc<TerminationManager>,

    /// Current checkpoint token.
    checkpoint_token: Arc<Mutex<String>>,

    /// Step data from replay (operation ID -> operation).
    step_data: Arc<Mutex<HashMap<String, Operation>>>,

    /// Queue of pending checkpoints.
    queue: Arc<Mutex<VecDeque<QueuedCheckpoint>>>,

    /// Whether the queue is currently being processed.
    is_processing: Arc<Mutex<bool>>,

    /// Whether termination has been triggered.
    is_terminating: Arc<AtomicBool>,

    /// Notification when queue is empty.
    queue_empty_notify: Arc<Notify>,

    /// Operation lifecycle tracking.
    operations: Arc<Mutex<HashMap<String, OperationInfo>>>,

    /// Operations that have pending completions.
    pending_completions: Arc<Mutex<HashSet<String>>>,
}

impl CheckpointManager {
    /// Create a new checkpoint manager.
    pub fn new(
        durable_execution_arn: String,
        lambda_service: Arc<dyn LambdaService>,
        termination_manager: Arc<TerminationManager>,
        checkpoint_token: String,
        step_data: HashMap<String, Operation>,
    ) -> Self {
        Self {
            durable_execution_arn,
            lambda_service,
            termination_manager,
            checkpoint_token: Arc::new(Mutex::new(checkpoint_token)),
            step_data: Arc::new(Mutex::new(step_data)),
            queue: Arc::new(Mutex::new(VecDeque::new())),
            is_processing: Arc::new(Mutex::new(false)),
            is_terminating: Arc::new(AtomicBool::new(false)),
            queue_empty_notify: Arc::new(Notify::new()),
            operations: Arc::new(Mutex::new(HashMap::new())),
            pending_completions: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Queue a checkpoint update for an operation.
    ///
    /// This method queues the update and triggers batch processing.
    /// It returns when the checkpoint has been acknowledged by the backend.
    pub async fn checkpoint(&self, step_id: String, update: OperationUpdate) -> DurableResult<()> {
        // Check if we're terminating
        if self.is_terminating.load(Ordering::SeqCst) {
            warn!("Checkpoint called while terminating, blocking forever");
            // Return a future that never resolves
            std::future::pending::<()>().await;
            unreachable!()
        }

        // Check for finished ancestors
        if self.has_finished_ancestor(&update).await {
            warn!("Operation has finished ancestor, blocking forever");
            std::future::pending::<()>().await;
            unreachable!()
        }

        let notify = Arc::new(Notify::new());

        // Track pending completions
        if matches!(
            update.action,
            OperationAction::Succeed | OperationAction::Fail
        ) {
            self.pending_completions
                .lock()
                .await
                .insert(step_id.clone());
        }

        // Update operation lifecycle
        self.update_operation_lifecycle(&update).await;

        // Queue the checkpoint
        let checkpoint = QueuedCheckpoint {
            step_id: step_id.clone(),
            update,
            notify: Arc::clone(&notify),
        };

        self.queue.lock().await.push_back(checkpoint);

        // Trigger processing if not already running
        self.maybe_start_processing().await;

        // Wait for this checkpoint to complete
        notify.notified().await;

        Ok(())
    }

    /// Queue a checkpoint update for background processing, without waiting for completion.
    ///
    /// This is used for "fire-and-forget" START updates in at-least-once step semantics,
    /// ensuring the START update is enqueued before subsequent updates (e.g., SUCCEED).
    pub async fn checkpoint_queued(
        &self,
        step_id: String,
        update: OperationUpdate,
    ) -> DurableResult<()> {
        // Check if we're terminating
        if self.is_terminating.load(Ordering::SeqCst) {
            warn!("Checkpoint called while terminating, blocking forever");
            std::future::pending::<()>().await;
            unreachable!()
        }

        // Check for finished ancestors
        if self.has_finished_ancestor(&update).await {
            warn!("Operation has finished ancestor, blocking forever");
            std::future::pending::<()>().await;
            unreachable!()
        }

        // Track pending completions (same as `checkpoint`, even though we don't wait here)
        if matches!(
            update.action,
            OperationAction::Succeed | OperationAction::Fail
        ) {
            self.pending_completions
                .lock()
                .await
                .insert(step_id.clone());
        }

        // Update operation lifecycle
        self.update_operation_lifecycle(&update).await;

        // Queue the checkpoint (notify is unused by the caller, but is required by the worker)
        let checkpoint = QueuedCheckpoint {
            step_id,
            update,
            notify: Arc::new(Notify::new()),
        };

        self.queue.lock().await.push_back(checkpoint);
        self.maybe_start_processing().await;

        Ok(())
    }

    /// Force an immediate checkpoint (empty batch to refresh state).
    pub async fn force_checkpoint(&self) -> DurableResult<()> {
        self.process_batch(vec![]).await
    }

    /// Set the manager to terminating state.
    pub fn set_terminating(&self) {
        self.is_terminating.store(true, Ordering::SeqCst);
    }

    /// Wait for the queue to be empty.
    pub async fn wait_for_queue_completion(&self) {
        loop {
            let notified = self.queue_empty_notify.notified();

            let is_complete = {
                let queue = self.queue.lock().await;
                let is_processing = *self.is_processing.lock().await;
                queue.is_empty() && !is_processing
            };

            if is_complete {
                return;
            }

            notified.await;
        }
    }

    /// Get the step data for a specific operation ID.
    pub async fn get_step_data(&self, hashed_id: &str) -> Option<Operation> {
        self.step_data.lock().await.get(hashed_id).cloned()
    }
}

impl Clone for CheckpointManager {
    fn clone(&self) -> Self {
        Self {
            durable_execution_arn: self.durable_execution_arn.clone(),
            lambda_service: Arc::clone(&self.lambda_service),
            termination_manager: Arc::clone(&self.termination_manager),
            checkpoint_token: Arc::clone(&self.checkpoint_token),
            step_data: Arc::clone(&self.step_data),
            queue: Arc::clone(&self.queue),
            is_processing: Arc::clone(&self.is_processing),
            is_terminating: Arc::clone(&self.is_terminating),
            queue_empty_notify: Arc::clone(&self.queue_empty_notify),
            operations: Arc::clone(&self.operations),
            pending_completions: Arc::clone(&self.pending_completions),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{MockCheckpointConfig, MockLambdaService};
    use crate::termination::{TerminationManager, TerminationReason};
    use std::collections::HashMap;
    use std::time::Duration;

    #[test]
    fn test_hash_id() {
        let id1 = CheckpointManager::hash_id("test-step-1");
        let id2 = CheckpointManager::hash_id("test-step-1");
        let id3 = CheckpointManager::hash_id("test-step-2");

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
        assert_eq!(id1.len(), 32);
    }

    #[test]
    fn test_operation_lifecycle() {
        assert_eq!(
            OperationLifecycle::NotStarted,
            OperationLifecycle::NotStarted
        );
        assert_ne!(OperationLifecycle::Executing, OperationLifecycle::Completed);
    }

    #[test]
    fn test_coalesce_updates_merges_fields() {
        let mock = Arc::new(MockLambdaService::new());
        let termination_manager = Arc::new(TerminationManager::new());
        let manager = CheckpointManager::new(
            "arn:aws:lambda:us-east-1:123:function:durable".to_string(),
            mock,
            termination_manager,
            "token-0".to_string(),
            HashMap::new(),
        );

        let update_start = OperationUpdate::builder()
            .id("op-1")
            .operation_type(OperationType::Step)
            .action(OperationAction::Start)
            .name("step-name")
            .parent_id("parent-1")
            .build()
            .unwrap();
        let update_succeed = OperationUpdate::builder()
            .id("op-1")
            .operation_type(OperationType::Step)
            .action(OperationAction::Succeed)
            .payload("{\"ok\":true}")
            .build()
            .unwrap();

        let updates = manager
            .coalesce_updates(vec![update_start, update_succeed])
            .expect("coalesce should succeed");

        assert_eq!(updates.len(), 1);
        let merged = &updates[0];
        assert_eq!(merged.action, OperationAction::Succeed);
        assert_eq!(merged.name.as_deref(), Some("step-name"));
        assert_eq!(merged.parent_id.as_deref(), Some("parent-1"));
        assert_eq!(merged.payload.as_deref(), Some("{\"ok\":true}"));
    }

    #[test]
    fn test_coalesce_updates_rejects_type_mismatch() {
        let mock = Arc::new(MockLambdaService::new());
        let termination_manager = Arc::new(TerminationManager::new());
        let manager = CheckpointManager::new(
            "arn:aws:lambda:us-east-1:123:function:durable".to_string(),
            mock,
            termination_manager,
            "token-0".to_string(),
            HashMap::new(),
        );

        let update_step = OperationUpdate::builder()
            .id("op-1")
            .operation_type(OperationType::Step)
            .action(OperationAction::Start)
            .build()
            .unwrap();
        let update_wait = OperationUpdate::builder()
            .id("op-1")
            .operation_type(OperationType::Wait)
            .action(OperationAction::Succeed)
            .build()
            .unwrap();

        let err = manager
            .coalesce_updates(vec![update_step, update_wait])
            .expect_err("type mismatch should error");

        match err {
            DurableError::ContextValidationError { message } => {
                assert!(message.contains("type mismatch"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_checkpoint_updates_token_and_step_data() {
        let mock = Arc::new(MockLambdaService::new());
        let termination_manager = Arc::new(TerminationManager::new());

        let op = Operation {
            id: "op-1".to_string(),
            parent_id: None,
            name: None,
            operation_type: OperationType::Step,
            sub_type: None,
            status: OperationStatus::Succeeded,
            step_details: None,
            callback_details: None,
            wait_details: None,
            execution_details: None,
            context_details: None,
            chained_invoke_details: None,
        };

        mock.expect_checkpoint(MockCheckpointConfig {
            checkpoint_token: Some("token-1".to_string()),
            operations: vec![op.clone()],
            next_marker: None,
            error: None,
        });
        mock.expect_checkpoint(MockCheckpointConfig::default());

        let manager = CheckpointManager::new(
            "arn:aws:lambda:us-east-1:123:function:durable".to_string(),
            mock.clone(),
            termination_manager,
            "token-0".to_string(),
            HashMap::new(),
        );

        let update = OperationUpdate::builder()
            .id(op.id.clone())
            .operation_type(OperationType::Step)
            .action(OperationAction::Succeed)
            .build()
            .unwrap();

        manager
            .checkpoint(op.id.clone(), update)
            .await
            .expect("checkpoint should succeed");

        let stored = manager.get_step_data(&op.id).await;
        assert!(stored.is_some());
        assert_eq!(stored.unwrap().status, OperationStatus::Succeeded);

        let update2 = OperationUpdate::builder()
            .id("op-2")
            .operation_type(OperationType::Step)
            .action(OperationAction::Start)
            .build()
            .unwrap();

        manager
            .checkpoint("op-2".to_string(), update2)
            .await
            .expect("checkpoint should succeed");

        let calls = mock.checkpoint_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].checkpoint_token, "token-0");
        assert_eq!(calls[1].checkpoint_token, "token-1");
    }

    #[tokio::test]
    async fn test_checkpoint_failure_triggers_termination() {
        let mock = Arc::new(MockLambdaService::new());
        let termination_manager = Arc::new(TerminationManager::new());

        mock.expect_checkpoint(MockCheckpointConfig {
            error: Some(DurableError::checkpoint_failed(
                "boom",
                false,
                None::<std::io::Error>,
            )),
            ..Default::default()
        });

        let manager = CheckpointManager::new(
            "arn:aws:lambda:us-east-1:123:function:durable".to_string(),
            mock,
            termination_manager.clone(),
            "token-0".to_string(),
            HashMap::new(),
        );

        let update = OperationUpdate::builder()
            .id("op-1")
            .operation_type(OperationType::Step)
            .action(OperationAction::Start)
            .build()
            .unwrap();

        manager
            .checkpoint("op-1".to_string(), update)
            .await
            .expect("checkpoint should return after failure");

        let result = tokio::time::timeout(Duration::from_millis(50), async {
            loop {
                if let Some(result) = termination_manager.get_termination_result() {
                    break result;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("termination should be triggered");

        assert_eq!(result.reason, TerminationReason::CheckpointFailed);
    }

    #[tokio::test]
    async fn test_has_finished_ancestor_detects_parent_completion() {
        let mock = Arc::new(MockLambdaService::new());
        let termination_manager = Arc::new(TerminationManager::new());

        let parent_op = Operation {
            id: "parent-1".to_string(),
            parent_id: None,
            name: None,
            operation_type: OperationType::Step,
            sub_type: None,
            status: OperationStatus::Succeeded,
            step_details: None,
            callback_details: None,
            wait_details: None,
            execution_details: None,
            context_details: None,
            chained_invoke_details: None,
        };

        let mut step_data = HashMap::new();
        step_data.insert(parent_op.id.clone(), parent_op);

        let manager = CheckpointManager::new(
            "arn:aws:lambda:us-east-1:123:function:durable".to_string(),
            mock,
            termination_manager,
            "token-0".to_string(),
            step_data,
        );

        let update = OperationUpdate::builder()
            .id("child-1")
            .parent_id("parent-1")
            .operation_type(OperationType::Step)
            .action(OperationAction::Start)
            .build()
            .unwrap();

        assert!(manager.has_finished_ancestor(&update).await);
    }

    #[tokio::test]
    async fn test_has_finished_ancestor_false_when_incomplete() {
        let mock = Arc::new(MockLambdaService::new());
        let termination_manager = Arc::new(TerminationManager::new());

        let parent_op = Operation {
            id: "parent-1".to_string(),
            parent_id: None,
            name: None,
            operation_type: OperationType::Step,
            sub_type: None,
            status: OperationStatus::Started,
            step_details: None,
            callback_details: None,
            wait_details: None,
            execution_details: None,
            context_details: None,
            chained_invoke_details: None,
        };

        let mut step_data = HashMap::new();
        step_data.insert(parent_op.id.clone(), parent_op);

        let manager = CheckpointManager::new(
            "arn:aws:lambda:us-east-1:123:function:durable".to_string(),
            mock,
            termination_manager,
            "token-0".to_string(),
            step_data,
        );

        let update = OperationUpdate::builder()
            .id("child-1")
            .parent_id("parent-1")
            .operation_type(OperationType::Step)
            .action(OperationAction::Start)
            .build()
            .unwrap();

        assert!(!manager.has_finished_ancestor(&update).await);
    }

    #[tokio::test]
    async fn test_mark_awaited_transitions_idle_state() {
        let mock = Arc::new(MockLambdaService::new());
        let termination_manager = Arc::new(TerminationManager::new());
        let manager = CheckpointManager::new(
            "arn:aws:lambda:us-east-1:123:function:durable".to_string(),
            mock,
            termination_manager,
            "token-0".to_string(),
            HashMap::new(),
        );

        {
            let mut operations = manager.operations.lock().await;
            operations.insert(
                "op-1".to_string(),
                OperationInfo {
                    id: "op-1".to_string(),
                    parent_id: None,
                    operation_type: OperationType::Wait,
                    lifecycle: OperationLifecycle::IdleNotAwaited,
                    awaited: false,
                },
            );
        }

        manager.mark_awaited("op-1").await;

        let operations = manager.operations.lock().await;
        let info = operations.get("op-1").expect("operation should exist");
        assert!(info.awaited);
        assert_eq!(info.lifecycle, OperationLifecycle::IdleAwaited);
    }

    #[tokio::test]
    async fn test_update_operation_lifecycle_wait_idle() {
        let mock = Arc::new(MockLambdaService::new());
        let termination_manager = Arc::new(TerminationManager::new());
        let manager = CheckpointManager::new(
            "arn:aws:lambda:us-east-1:123:function:durable".to_string(),
            mock,
            termination_manager,
            "token-0".to_string(),
            HashMap::new(),
        );

        let update = OperationUpdate::builder()
            .id("wait-1")
            .operation_type(OperationType::Wait)
            .action(OperationAction::Start)
            .build()
            .unwrap();

        manager.update_operation_lifecycle(&update).await;

        let operations = manager.operations.lock().await;
        let info = operations.get("wait-1").expect("operation should exist");
        assert_eq!(info.lifecycle, OperationLifecycle::IdleNotAwaited);
    }
}
