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

/// Maximum payload size for a checkpoint batch (750KB).
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

    /// Hash an operation ID for storage.
    ///
    /// We use SHA-256 truncated to 128 bits (32 hex chars). The JS SDK uses MD5-16 for
    /// speed and the Python SDK uses BLAKE2b-64; SHA-256 is widely understood and avoids
    /// MD5 while keeping IDs reasonably short.
    pub fn hash_id(id: &str) -> String {
        use sha2::{Digest, Sha256};
        use std::fmt::Write as _;

        let digest = Sha256::digest(id.as_bytes());
        let mut hex = String::with_capacity(32);
        for byte in digest.iter().take(16) {
            let _ = write!(hex, "{:02x}", byte);
        }
        hex
    }

    /// Check if an operation has a finished (succeeded/failed) ancestor.
    async fn has_finished_ancestor(&self, update: &OperationUpdate) -> bool {
        if let Some(parent_id) = &update.parent_id {
            if let Some(parent) = self.step_data.lock().await.get(parent_id) {
                if matches!(
                    parent.status,
                    OperationStatus::Succeeded | OperationStatus::Failed
                ) {
                    return true;
                }
            }
        }
        false
    }

    /// Update operation lifecycle based on an update.
    async fn update_operation_lifecycle(&self, update: &OperationUpdate) {
        let mut operations = self.operations.lock().await;

        // Preserve awaited flag if operation already exists
        let existing_awaited = operations
            .get(&update.id)
            .map(|op| op.awaited)
            .unwrap_or(false);

        let lifecycle = match update.action {
            OperationAction::Start => {
                // For wait/callback operations, they start and then become idle (waiting for external event)
                if matches!(
                    update.operation_type,
                    OperationType::Wait | OperationType::Callback
                ) {
                    if existing_awaited {
                        OperationLifecycle::IdleAwaited
                    } else {
                        OperationLifecycle::IdleNotAwaited
                    }
                } else {
                    OperationLifecycle::Executing
                }
            }
            OperationAction::Retry => OperationLifecycle::RetryWaiting,
            OperationAction::Succeed | OperationAction::Fail => OperationLifecycle::Completed,
            OperationAction::Cancel => OperationLifecycle::Completed,
        };

        let info = OperationInfo {
            id: update.id.clone(),
            parent_id: update.parent_id.clone(),
            operation_type: update.operation_type,
            lifecycle,
            awaited: existing_awaited,
        };

        operations.insert(update.id.clone(), info);
    }

    /// Mark an operation as awaited (i.e., user code is waiting for it).
    ///
    /// This also updates the lifecycle state if the operation is currently idle.
    pub async fn mark_awaited(&self, operation_id: &str) {
        let mut operations = self.operations.lock().await;
        if let Some(info) = operations.get_mut(operation_id) {
            info.awaited = true;
            // Update lifecycle if currently idle-not-awaited
            if info.lifecycle == OperationLifecycle::IdleNotAwaited {
                info.lifecycle = OperationLifecycle::IdleAwaited;
            }
        }
    }

    /// Maybe start processing the queue.
    async fn maybe_start_processing(&self) {
        let mut is_processing = self.is_processing.lock().await;
        if *is_processing {
            return;
        }
        *is_processing = true;
        drop(is_processing);

        // Clone self for the spawned task
        let manager = self.clone();
        tokio::spawn(async move {
            manager.process_queue().await;
        });
    }

    /// Process queued checkpoints in batches.
    async fn process_queue(&self) {
        loop {
            // Build a batch
            let batch = {
                let mut queue = self.queue.lock().await;
                if queue.is_empty() {
                    break;
                }

                let mut batch = Vec::new();
                let mut current_size = 100; // Base overhead

                while let Some(checkpoint) = queue.front() {
                    let item_size = self.estimate_update_size(&checkpoint.update);
                    if current_size + item_size > MAX_PAYLOAD_SIZE && !batch.is_empty() {
                        break;
                    }

                    let checkpoint = queue.pop_front().unwrap();
                    current_size += item_size;
                    batch.push(checkpoint);
                }

                batch
            };

            if batch.is_empty() {
                break;
            }

            // Process the batch
            let updates: Vec<_> = batch.iter().map(|c| c.update.clone()).collect();
            match self.process_batch(updates).await {
                Ok(_) => {
                    // Notify all checkpoints in batch
                    for checkpoint in &batch {
                        if matches!(
                            checkpoint.update.action,
                            OperationAction::Succeed | OperationAction::Fail
                        ) {
                            self.pending_completions
                                .lock()
                                .await
                                .remove(&checkpoint.step_id);
                        }
                        checkpoint.notify.notify_one();
                    }
                }
                Err(e) => {
                    error!("Checkpoint batch failed: {}", e);

                    // Notify all waiters in the failed batch
                    for checkpoint in &batch {
                        checkpoint.notify.notify_one();
                    }

                    // Clear pending completions to avoid unbounded growth on repeated failures.
                    self.pending_completions.lock().await.clear();

                    // Also notify all remaining queued items before clearing
                    // This prevents any callers from hanging indefinitely
                    {
                        let mut queue = self.queue.lock().await;
                        for checkpoint in queue.drain(..) {
                            checkpoint.notify.notify_one();
                        }
                    }

                    // Trigger termination
                    self.termination_manager
                        .terminate_for_checkpoint_failure(e)
                        .await;

                    break;
                }
            }

            // Check termination conditions
            self.check_and_schedule_termination().await;
        }

        // Mark processing as complete
        *self.is_processing.lock().await = false;
        self.queue_empty_notify.notify_waiters();
    }

    /// Process a batch of updates.
    async fn process_batch(&self, updates: Vec<OperationUpdate>) -> DurableResult<()> {
        let token = self.checkpoint_token.lock().await.clone();

        let updates = self.coalesce_updates(updates)?;

        debug!(
            "Checkpointing {} updates to {}",
            updates.len(),
            self.durable_execution_arn
        );

        let response = self
            .lambda_service
            .checkpoint_durable_execution(&self.durable_execution_arn, &token, updates)
            .await?;

        // Update checkpoint token
        if let Some(new_token) = response.checkpoint_token {
            *self.checkpoint_token.lock().await = new_token;
        }

        // Update step data from response
        if let Some(new_state) = response.new_execution_state {
            let mut step_data = self.step_data.lock().await;
            for op in new_state.operations {
                step_data.insert(op.id.clone(), op);
            }
        }

        info!("Checkpoint successful");
        Ok(())
    }

    fn coalesce_updates(
        &self,
        updates: Vec<OperationUpdate>,
    ) -> DurableResult<Vec<OperationUpdate>> {
        if updates.len() <= 1 {
            return Ok(updates);
        }

        let mut order: Vec<String> = Vec::new();
        let mut by_id: HashMap<String, OperationUpdate> = HashMap::new();

        for update in updates {
            let id = update.id.clone();
            let existing = match by_id.entry(id.clone()) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    order.push(id);
                    entry.insert(update);
                    continue;
                }
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            };
            if existing.operation_type != update.operation_type {
                return Err(DurableError::ContextValidationError {
                    message: format!(
                        "OperationUpdate type mismatch for id {}: {:?} vs {:?}",
                        existing.id, existing.operation_type, update.operation_type
                    ),
                });
            }

            // Keep the newest action, but preserve any metadata from earlier updates if missing.
            existing.action = update.action;
            if update.parent_id.is_some() {
                existing.parent_id = update.parent_id;
            }
            if update.name.is_some() {
                existing.name = update.name;
            }
            if update.sub_type.is_some() {
                existing.sub_type = update.sub_type;
            }
            if update.payload.is_some() {
                existing.payload = update.payload;
            }
            if update.error.is_some() {
                existing.error = update.error;
            }
            if update.context_options.is_some() {
                existing.context_options = update.context_options;
            }
            if update.step_options.is_some() {
                existing.step_options = update.step_options;
            }
            if update.wait_options.is_some() {
                existing.wait_options = update.wait_options;
            }
            if update.callback_options.is_some() {
                existing.callback_options = update.callback_options;
            }
            if update.chained_invoke_options.is_some() {
                existing.chained_invoke_options = update.chained_invoke_options;
            }
        }

        if by_id.len() != order.len() {
            warn!(
                "Coalesced updates from {} into {} (duplicate operation ids in batch)",
                order.len(),
                by_id.len()
            );
        }

        Ok(order
            .into_iter()
            .filter_map(|id| by_id.remove(&id))
            .collect())
    }

    /// Check termination conditions and schedule termination if appropriate.
    async fn check_and_schedule_termination(&self) {
        let any_retry_waiting = {
            let operations = self.operations.lock().await;

            // Check if any operation is retry-waiting
            operations
                .values()
                .any(|op| op.lifecycle == OperationLifecycle::RetryWaiting)
        };

        if any_retry_waiting {
            // Wait for cooldown then revalidate before terminating
            tokio::time::sleep(tokio::time::Duration::from_millis(TERMINATION_COOLDOWN_MS)).await;

            // Revalidate: only terminate if still retry-waiting
            let still_retry_waiting = {
                let operations = self.operations.lock().await;
                operations
                    .values()
                    .any(|op| op.lifecycle == OperationLifecycle::RetryWaiting)
            };
            if still_retry_waiting {
                self.termination_manager.terminate_for_retry().await;
            }
        }
    }

    /// Estimate the size of an operation update in bytes.
    fn estimate_update_size(&self, update: &OperationUpdate) -> usize {
        let base = 100; // Base overhead for structure
        let id = update.id.len();
        let parent_id = update.parent_id.as_ref().map(|s| s.len()).unwrap_or(0);
        let name = update.name.as_ref().map(|s| s.len()).unwrap_or(0);
        let payload = update.payload.as_ref().map(|s| s.len()).unwrap_or(0);

        base + id + parent_id + name + payload
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
}
