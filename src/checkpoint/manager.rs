//! Checkpoint manager for persisting operation state.

use crate::error::{DurableError, DurableResult};
use crate::termination::TerminationManager;
use crate::types::{Operation, OperationAction, OperationStatus, OperationType, OperationUpdate};
use aws_sdk_lambda::error::SdkError;
use aws_sdk_lambda::operation::checkpoint_durable_execution::CheckpointDurableExecutionError;
use aws_sdk_lambda::Client as LambdaClient;
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

    /// Lambda client for API calls.
    lambda_client: Arc<LambdaClient>,

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
        lambda_client: Arc<LambdaClient>,
        termination_manager: Arc<TerminationManager>,
        checkpoint_token: String,
        step_data: HashMap<String, Operation>,
    ) -> Self {
        Self {
            durable_execution_arn,
            lambda_client,
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

        // Convert our updates to SDK types
        let sdk_updates: Vec<aws_sdk_lambda::types::OperationUpdate> = updates
            .into_iter()
            .map(|u| self.to_sdk_operation_update(u))
            .collect::<Result<Vec<_>, _>>()?;

        debug!(
            "Checkpointing {} updates to {}",
            sdk_updates.len(),
            self.durable_execution_arn
        );

        let response = self
            .lambda_client
            .checkpoint_durable_execution()
            .durable_execution_arn(&self.durable_execution_arn)
            .checkpoint_token(&token)
            .set_updates(if sdk_updates.is_empty() {
                None
            } else {
                Some(sdk_updates)
            })
            .send()
            .await
            .map_err(|e| {
                let is_recoverable = self.is_recoverable_error(&e);
                let debug = format!("{e:?}");
                DurableError::checkpoint_failed(
                    format!("Failed to checkpoint: {}", debug),
                    is_recoverable,
                    Some(e),
                )
            })?;

        // Update checkpoint token
        if let Some(new_token) = response.checkpoint_token {
            *self.checkpoint_token.lock().await = new_token;
        }

        // Update step data from response
        if let Some(new_state) = response.new_execution_state {
            if let Some(operations) = new_state.operations {
                let mut step_data = self.step_data.lock().await;
                for op in operations {
                    // op.id is String in the AWS SDK
                    let id = &op.id;
                    // Convert SDK operation to our type
                    if let Ok(operation) = self.sdk_operation_to_operation(&op) {
                        step_data.insert(id.clone(), operation);
                    }
                }
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

    /// Check if an error is recoverable (should retry).
    fn is_recoverable_error(&self, error: &SdkError<CheckpointDurableExecutionError>) -> bool {
        match error {
            SdkError::TimeoutError(_)
            | SdkError::DispatchFailure(_)
            | SdkError::ResponseError(_) => true,
            SdkError::ConstructionFailure(_) => false,
            SdkError::ServiceError(context) => {
                let status = context.raw().status();
                let status_code = status.as_u16();

                match context.err() {
                    CheckpointDurableExecutionError::TooManyRequestsException(_) => true,
                    CheckpointDurableExecutionError::ServiceException(_) => true,
                    CheckpointDurableExecutionError::InvalidParameterValueException(inner) => {
                        let message = inner
                            .message()
                            .map(str::to_string)
                            .unwrap_or_else(|| inner.to_string());
                        if message.starts_with("Invalid Checkpoint Token") {
                            return true;
                        }

                        if status.is_client_error() && status_code != 429 {
                            return false;
                        }
                        if status.is_server_error() || status_code == 429 {
                            return true;
                        }

                        self.is_recoverable_message(&message)
                    }
                    other => {
                        if status.is_server_error() || status_code == 429 {
                            return true;
                        }
                        if status.is_client_error() && status_code != 429 {
                            return false;
                        }
                        let message =
                            aws_smithy_types::error::metadata::ProvideErrorMetadata::message(other)
                                .map(str::to_string)
                                .unwrap_or_else(|| other.to_string());
                        self.is_recoverable_message(&message)
                    }
                }
            }
            _ => self.is_recoverable_message(&error.to_string()),
        }
    }

    fn is_recoverable_message(&self, message: &str) -> bool {
        let error_str = message.to_lowercase();
        error_str.contains("throttl")
            || error_str.contains("rate")
            || error_str.contains("timeout")
            || error_str.contains("temporary")
    }

    /// Convert our OperationUpdate to SDK type.
    fn to_sdk_operation_update(
        &self,
        update: OperationUpdate,
    ) -> DurableResult<aws_sdk_lambda::types::OperationUpdate> {
        let builder = aws_sdk_lambda::types::OperationUpdate::builder()
            .id(&update.id)
            .r#type(self.to_sdk_operation_type(update.operation_type))
            .action(self.to_sdk_operation_action(update.action));

        let builder = if let Some(parent_id) = update.parent_id {
            builder.parent_id(parent_id)
        } else {
            builder
        };

        let builder = if let Some(name) = update.name {
            builder.name(name)
        } else {
            builder
        };

        let builder = if let Some(sub_type) = update.sub_type {
            builder.sub_type(sub_type)
        } else {
            builder
        };

        let builder = if let Some(payload) = update.payload {
            builder.payload(payload)
        } else {
            builder
        };

        let builder = if let Some(error) = update.error {
            builder.error(
                aws_sdk_lambda::types::ErrorObject::builder()
                    .error_type(&error.error_type)
                    .error_message(&error.error_message)
                    .build(),
            )
        } else {
            builder
        };

        let builder = if let Some(ctx_opts) = update.context_options {
            let mut b = aws_sdk_lambda::types::ContextOptions::builder();
            if let Some(replay_children) = ctx_opts.replay_children {
                b = b.replay_children(replay_children);
            }
            builder.context_options(b.build())
        } else {
            builder
        };

        let builder = if let Some(step_opts) = update.step_options {
            let mut b = aws_sdk_lambda::types::StepOptions::builder();
            if let Some(secs) = step_opts.next_attempt_delay_seconds {
                b = b.next_attempt_delay_seconds(secs);
            }
            builder.step_options(b.build())
        } else {
            builder
        };

        let builder = if let Some(wait_opts) = update.wait_options {
            let mut b = aws_sdk_lambda::types::WaitOptions::builder();
            if let Some(secs) = wait_opts.wait_seconds {
                b = b.wait_seconds(secs);
            }
            builder.wait_options(b.build())
        } else {
            builder
        };

        let builder = if let Some(cb_opts) = update.callback_options {
            let mut b = aws_sdk_lambda::types::CallbackOptions::builder();
            if let Some(secs) = cb_opts.timeout_seconds {
                b = b.timeout_seconds(secs);
            }
            if let Some(secs) = cb_opts.heartbeat_timeout_seconds {
                b = b.heartbeat_timeout_seconds(secs);
            }
            builder.callback_options(b.build())
        } else {
            builder
        };

        let builder = if let Some(invoke_opts) = update.chained_invoke_options {
            let mut b = aws_sdk_lambda::types::ChainedInvokeOptions::builder()
                .function_name(invoke_opts.function_name);
            if let Some(tenant_id) = invoke_opts.tenant_id {
                b = b.tenant_id(tenant_id);
            }
            let opts = b.build().map_err(|e| {
                DurableError::Internal(format!("Failed to build chained invoke options: {}", e))
            })?;
            builder.chained_invoke_options(opts)
        } else {
            builder
        };

        builder
            .build()
            .map_err(|e| DurableError::Internal(format!("Failed to build operation update: {}", e)))
    }

    /// Convert our OperationType to SDK type.
    fn to_sdk_operation_type(
        &self,
        op_type: OperationType,
    ) -> aws_sdk_lambda::types::OperationType {
        match op_type {
            OperationType::Step => aws_sdk_lambda::types::OperationType::Step,
            OperationType::Wait => aws_sdk_lambda::types::OperationType::Wait,
            OperationType::Callback => aws_sdk_lambda::types::OperationType::Callback,
            OperationType::ChainedInvoke => aws_sdk_lambda::types::OperationType::ChainedInvoke,
            OperationType::Context => aws_sdk_lambda::types::OperationType::Context,
            OperationType::Execution => aws_sdk_lambda::types::OperationType::Execution,
        }
    }

    /// Convert our OperationAction to SDK type.
    fn to_sdk_operation_action(
        &self,
        action: OperationAction,
    ) -> aws_sdk_lambda::types::OperationAction {
        match action {
            OperationAction::Start => aws_sdk_lambda::types::OperationAction::Start,
            OperationAction::Retry => aws_sdk_lambda::types::OperationAction::Retry,
            OperationAction::Succeed => aws_sdk_lambda::types::OperationAction::Succeed,
            OperationAction::Fail => aws_sdk_lambda::types::OperationAction::Fail,
            OperationAction::Cancel => aws_sdk_lambda::types::OperationAction::Cancel,
        }
    }

    /// Convert SDK Operation to our type.
    fn sdk_operation_to_operation(
        &self,
        op: &aws_sdk_lambda::types::Operation,
    ) -> DurableResult<Operation> {
        Ok(Operation {
            id: op.id.clone(),
            parent_id: op.parent_id.clone(),
            name: op.name.clone(),
            operation_type: self.sdk_operation_type_to_operation_type(op.r#type.clone()),
            sub_type: op.sub_type.clone(),
            status: self.sdk_operation_status_to_operation_status(op.status.clone()),
            step_details: op.step_details.as_ref().map(|d| crate::types::StepDetails {
                attempt: Some(d.attempt as u32),
                next_attempt_timestamp: d
                    .next_attempt_timestamp
                    .as_ref()
                    .map(|ts| crate::types::FlexibleTimestamp::String(ts.to_string())),
                result: d.result.clone(),
                error: d
                    .error
                    .as_ref()
                    .map(|e| self.sdk_error_object_to_error_object(e)),
            }),
            callback_details: op
                .callback_details
                .as_ref()
                .map(|d| crate::types::CallbackDetails {
                    callback_id: d.callback_id.clone(),
                    result: d.result.clone(),
                    error: d
                        .error
                        .as_ref()
                        .map(|e| self.sdk_error_object_to_error_object(e)),
                }),
            wait_details: op.wait_details.as_ref().map(|d| crate::types::WaitDetails {
                scheduled_end_timestamp: d
                    .scheduled_end_timestamp
                    .as_ref()
                    .map(|ts| crate::types::FlexibleTimestamp::String(ts.to_string())),
            }),
            execution_details: op.execution_details.as_ref().map(|d| {
                crate::types::ExecutionDetails {
                    input_payload: d.input_payload.clone(),
                    // Note: output_payload is not available from the AWS SDK's ExecutionDetails
                    // It's typically populated when the execution completes
                    output_payload: None,
                }
            }),
            context_details: op
                .context_details
                .as_ref()
                .map(|d| crate::types::ContextDetails {
                    replay_children: d.replay_children,
                    result: d.result.clone(),
                    error: d
                        .error
                        .as_ref()
                        .map(|e| self.sdk_error_object_to_error_object(e)),
                }),
            chained_invoke_details: op.chained_invoke_details.as_ref().map(|d| {
                crate::types::ChainedInvokeDetails {
                    result: d.result.clone(),
                    error: d
                        .error
                        .as_ref()
                        .map(|e| self.sdk_error_object_to_error_object(e)),
                }
            }),
        })
    }

    fn sdk_error_object_to_error_object(
        &self,
        e: &aws_sdk_lambda::types::ErrorObject,
    ) -> crate::error::ErrorObject {
        crate::error::ErrorObject {
            error_type: e.error_type.clone().unwrap_or_else(|| "Error".to_string()),
            error_message: e
                .error_message
                .clone()
                .unwrap_or_else(|| "Unknown error".to_string()),
            details: e
                .error_data
                .clone()
                .or_else(|| e.stack_trace.as_ref().map(|st| st.join("\n"))),
        }
    }

    /// Convert SDK OperationType to our type.
    fn sdk_operation_type_to_operation_type(
        &self,
        op_type: aws_sdk_lambda::types::OperationType,
    ) -> OperationType {
        match op_type {
            aws_sdk_lambda::types::OperationType::Step => OperationType::Step,
            aws_sdk_lambda::types::OperationType::Wait => OperationType::Wait,
            aws_sdk_lambda::types::OperationType::Callback => OperationType::Callback,
            aws_sdk_lambda::types::OperationType::ChainedInvoke => OperationType::ChainedInvoke,
            aws_sdk_lambda::types::OperationType::Context => OperationType::Context,
            aws_sdk_lambda::types::OperationType::Execution => OperationType::Execution,
            _ => {
                warn!(
                    "Unknown SDK operation type {:?}, defaulting to Step",
                    op_type
                );
                OperationType::Step
            }
        }
    }

    /// Convert SDK OperationStatus to our type.
    fn sdk_operation_status_to_operation_status(
        &self,
        status: aws_sdk_lambda::types::OperationStatus,
    ) -> OperationStatus {
        match status {
            aws_sdk_lambda::types::OperationStatus::Ready => OperationStatus::Ready,
            aws_sdk_lambda::types::OperationStatus::Started => OperationStatus::Started,
            aws_sdk_lambda::types::OperationStatus::Pending => OperationStatus::Pending,
            aws_sdk_lambda::types::OperationStatus::Succeeded => OperationStatus::Succeeded,
            aws_sdk_lambda::types::OperationStatus::Failed => OperationStatus::Failed,
            _ => {
                warn!(
                    "Unknown SDK operation status {:?}, defaulting to Started",
                    status
                );
                OperationStatus::Started
            }
        }
    }
}

impl Clone for CheckpointManager {
    fn clone(&self) -> Self {
        Self {
            durable_execution_arn: self.durable_execution_arn.clone(),
            lambda_client: Arc::clone(&self.lambda_client),
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
}
