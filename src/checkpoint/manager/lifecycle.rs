use super::*;

impl CheckpointManager {
    /// Check if an operation has a finished (succeeded/failed) ancestor.
    pub(super) async fn has_finished_ancestor(&self, update: &OperationUpdate) -> bool {
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
    pub(super) async fn update_operation_lifecycle(&self, update: &OperationUpdate) {
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
}
