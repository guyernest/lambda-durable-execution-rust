use super::*;

impl CheckpointManager {
    pub(super) fn coalesce_updates(
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
}
