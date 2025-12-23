use super::*;

impl CheckpointManager {
    /// Maybe start processing the queue.
    pub(super) async fn maybe_start_processing(&self) {
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
    pub(super) async fn process_batch(&self, updates: Vec<OperationUpdate>) -> DurableResult<()> {
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
