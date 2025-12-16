//! Internal execution context for managing execution state.

use crate::checkpoint::CheckpointManager;
use crate::error::{DurableError, DurableResult};
use crate::termination::TerminationManager;
use crate::types::{
    DurableExecutionInvocationInput, DurableLogger, ExecutionDetails, Operation, OperationStatus,
    OperationType, TracingLogger,
};
use aws_sdk_lambda::Client as LambdaClient;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Default maximum number of operations to fetch per page.
///
/// Matches the AWS API maximum for `GetDurableExecutionState`.
const GET_STATE_MAX_ITEMS: i32 = 1000;

/// Safety guard to avoid infinite pagination loops if the service returns a repeating marker.
const GET_STATE_MAX_PAGES: usize = 1000;

/// Execution mode for the durable function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    /// Fresh execution (first invocation).
    Execution,
    /// Replaying from checkpointed state.
    Replay,
}

/// Internal context for managing execution state.
///
/// This is not exposed to users - they interact with `DurableContext`.
#[derive(Clone)]
pub struct ExecutionContext {
    /// ARN of the durable execution.
    pub durable_execution_arn: String,

    /// Lambda client for API calls.
    pub lambda_client: Arc<LambdaClient>,

    /// Termination manager.
    pub termination_manager: Arc<TerminationManager>,

    /// Checkpoint manager.
    pub checkpoint_manager: Arc<CheckpointManager>,

    /// Step data from replay.
    pub step_data: Arc<Mutex<HashMap<String, Operation>>>,

    /// Current execution mode.
    pub mode: Arc<Mutex<ExecutionMode>>,

    /// Operation counter for generating unique IDs.
    pub operation_counter: Arc<AtomicU64>,

    /// Current parent operation ID (for child contexts).
    pub current_parent_id: Arc<Mutex<Option<String>>>,

    /// Pending completions being tracked.
    pub pending_completions: Arc<Mutex<std::collections::HashSet<String>>>,

    /// Logger used for durable operations.
    pub logger: Arc<dyn DurableLogger>,

    /// Whether to suppress logs during replay.
    pub mode_aware_logging: bool,
}

impl ExecutionContext {
    /// Create a new execution context.
    pub async fn new(
        input: &DurableExecutionInvocationInput,
        lambda_client: Arc<LambdaClient>,
        logger: Option<Arc<dyn DurableLogger>>,
        mode_aware_logging: bool,
    ) -> DurableResult<Self> {
        // Build step data map from initial state (first page).
        let mut step_data = HashMap::new();
        for op in &input.initial_execution_state.operations {
            step_data.insert(op.id.clone(), op.clone());
        }

        // If there is more state, fetch additional pages using GetDurableExecutionState.
        let mut next_marker = input
            .initial_execution_state
            .next_marker
            .clone()
            .filter(|m| !m.is_empty());
        let mut pages = 0usize;

        while let Some(marker) = next_marker.take() {
            pages += 1;
            if pages > GET_STATE_MAX_PAGES {
                return Err(DurableError::InvalidConfiguration {
                    message: "Exceeded max durable execution state pages while paging operations"
                        .to_string(),
                });
            }

            let response = lambda_client
                .get_durable_execution_state()
                .durable_execution_arn(&input.durable_execution_arn)
                .checkpoint_token(&input.checkpoint_token)
                .marker(marker)
                .max_items(GET_STATE_MAX_ITEMS)
                .send()
                .await
                .map_err(DurableError::aws_sdk)?;

            for op in response.operations {
                let converted = sdk_operation_to_operation(&op)?;
                step_data.insert(converted.id.clone(), converted);
            }

            next_marker = response.next_marker.filter(|m| !m.is_empty());
        }

        // Determine execution mode
        let mode = if step_data.len() > 1 {
            ExecutionMode::Replay
        } else {
            ExecutionMode::Execution
        };

        let termination_manager = Arc::new(TerminationManager::new());

        let checkpoint_manager = Arc::new(CheckpointManager::new(
            input.durable_execution_arn.clone(),
            Arc::clone(&lambda_client),
            Arc::clone(&termination_manager),
            input.checkpoint_token.clone(),
            step_data.clone(),
        ));

        // Mirror the JS SDK behavior: once termination starts, stop accepting new checkpoints.
        {
            let checkpoint_manager = Arc::clone(&checkpoint_manager);
            termination_manager
                .set_checkpoint_terminating_callback(move || {
                    checkpoint_manager.set_terminating();
                })
                .await;
        }

        Ok(Self {
            durable_execution_arn: input.durable_execution_arn.clone(),
            lambda_client,
            termination_manager,
            checkpoint_manager,
            step_data: Arc::new(Mutex::new(step_data)),
            mode: Arc::new(Mutex::new(mode)),
            operation_counter: Arc::new(AtomicU64::new(0)),
            current_parent_id: Arc::new(Mutex::new(None)),
            pending_completions: Arc::new(Mutex::new(std::collections::HashSet::new())),
            logger: logger.unwrap_or_else(|| Arc::new(TracingLogger)),
            mode_aware_logging,
        })
    }

    /// Get the current execution mode.
    pub async fn get_mode(&self) -> ExecutionMode {
        *self.mode.lock().await
    }

    /// Set the execution mode.
    pub async fn set_mode(&self, mode: ExecutionMode) {
        *self.mode.lock().await = mode;
    }

    /// Generate a unique operation ID.
    pub fn next_operation_id(&self, name: Option<&str>) -> String {
        let counter = self.operation_counter.fetch_add(1, Ordering::SeqCst);
        match name {
            Some(n) => format!("{}_{}", n, counter),
            None => format!("op_{}", counter),
        }
    }

    /// Get step data for a hashed operation ID.
    pub async fn get_step_data(&self, hashed_id: &str) -> Option<Operation> {
        self.step_data.lock().await.get(hashed_id).cloned()
    }

    /// Get the current parent ID.
    pub async fn get_parent_id(&self) -> Option<String> {
        self.current_parent_id.lock().await.clone()
    }

    /// Set the current parent ID.
    pub async fn set_parent_id(&self, parent_id: Option<String>) {
        *self.current_parent_id.lock().await = parent_id;
    }

    /// Create a child context with a new parent ID.
    pub fn with_parent_id(&self, parent_id: String) -> Self {
        let mut child = self.clone();
        child.current_parent_id = Arc::new(Mutex::new(Some(parent_id)));
        child
    }
}

fn sdk_operation_to_operation(op: &aws_sdk_lambda::types::Operation) -> DurableResult<Operation> {
    Ok(Operation {
        id: op.id.clone(),
        parent_id: op.parent_id.clone(),
        name: op.name.clone(),
        operation_type: match op.r#type {
            aws_sdk_lambda::types::OperationType::Step => OperationType::Step,
            aws_sdk_lambda::types::OperationType::Wait => OperationType::Wait,
            aws_sdk_lambda::types::OperationType::Callback => OperationType::Callback,
            aws_sdk_lambda::types::OperationType::ChainedInvoke => OperationType::ChainedInvoke,
            aws_sdk_lambda::types::OperationType::Context => OperationType::Context,
            aws_sdk_lambda::types::OperationType::Execution => OperationType::Execution,
            _ => OperationType::Step,
        },
        sub_type: op.sub_type.clone(),
        status: match op.status {
            aws_sdk_lambda::types::OperationStatus::Ready => OperationStatus::Ready,
            aws_sdk_lambda::types::OperationStatus::Started => OperationStatus::Started,
            aws_sdk_lambda::types::OperationStatus::Pending => OperationStatus::Pending,
            aws_sdk_lambda::types::OperationStatus::Succeeded => OperationStatus::Succeeded,
            aws_sdk_lambda::types::OperationStatus::Failed => OperationStatus::Failed,
            _ => OperationStatus::Unknown,
        },
        step_details: op.step_details.as_ref().map(|d| crate::types::StepDetails {
            attempt: Some(d.attempt as u32),
            next_attempt_timestamp: d
                .next_attempt_timestamp
                .as_ref()
                .map(|ts| crate::types::FlexibleTimestamp::String(ts.to_string())),
            result: d.result.clone(),
            error: d.error.as_ref().map(sdk_error_object_to_error_object),
        }),
        callback_details: op
            .callback_details
            .as_ref()
            .map(|d| crate::types::CallbackDetails {
                callback_id: d.callback_id.clone(),
                result: d.result.clone(),
                error: d.error.as_ref().map(sdk_error_object_to_error_object),
            }),
        wait_details: op.wait_details.as_ref().map(|d| crate::types::WaitDetails {
            scheduled_end_timestamp: d
                .scheduled_end_timestamp
                .as_ref()
                .map(|ts| crate::types::FlexibleTimestamp::String(ts.to_string())),
        }),
        execution_details: op.execution_details.as_ref().map(|d| ExecutionDetails {
            input_payload: d.input_payload.clone(),
            output_payload: None,
        }),
        context_details: op
            .context_details
            .as_ref()
            .map(|d| crate::types::ContextDetails {
                replay_children: d.replay_children,
                result: d.result.clone(),
                error: d.error.as_ref().map(sdk_error_object_to_error_object),
            }),
        chained_invoke_details: op.chained_invoke_details.as_ref().map(|d| {
            crate::types::ChainedInvokeDetails {
                result: d.result.clone(),
                error: d.error.as_ref().map(sdk_error_object_to_error_object),
            }
        }),
    })
}

fn sdk_error_object_to_error_object(
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

impl std::fmt::Debug for ExecutionContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutionContext")
            .field("durable_execution_arn", &self.durable_execution_arn)
            .field(
                "operation_counter",
                &self.operation_counter.load(Ordering::SeqCst),
            )
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operation_id_generation() {
        let counter = AtomicU64::new(0);

        // Simulate what next_operation_id does
        let id1 = format!("op_{}", counter.fetch_add(1, Ordering::SeqCst));
        let id2 = format!("step_{}", counter.fetch_add(1, Ordering::SeqCst));

        assert_eq!(id1, "op_0");
        assert_eq!(id2, "step_1");
    }

    #[test]
    fn test_execution_mode() {
        assert_ne!(ExecutionMode::Execution, ExecutionMode::Replay);
    }
}
