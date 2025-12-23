//! Internal execution context for managing execution state.

use crate::checkpoint::CheckpointManager;
use crate::error::{DurableError, DurableResult};
use crate::termination::TerminationManager;
use crate::types::{
    DurableExecutionInvocationInput, DurableLogger, LambdaService, Operation, TracingLogger,
};
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

    /// Lambda service for API calls.
    pub lambda_service: Arc<dyn LambdaService>,

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
        lambda_service: Arc<dyn LambdaService>,
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

            let response = lambda_service
                .get_durable_execution_state(
                    &input.durable_execution_arn,
                    &input.checkpoint_token,
                    &marker,
                    GET_STATE_MAX_ITEMS,
                )
                .await?;

            for op in response.operations {
                step_data.insert(op.id.clone(), op);
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
            Arc::clone(&lambda_service),
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
            lambda_service,
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
    use crate::mock::{MockGetStateConfig, MockLambdaService};
    use crate::types::{ExecutionDetails, OperationStatus, OperationType};
    use std::sync::Arc;

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

    #[tokio::test]
    async fn test_execution_context_fetches_additional_state() {
        let mock = Arc::new(MockLambdaService::new());
        let input = DurableExecutionInvocationInput {
            durable_execution_arn: "arn:aws:lambda:us-east-1:123:function:durable".to_string(),
            checkpoint_token: "token-0".to_string(),
            initial_execution_state: crate::types::InitialExecutionState {
                operations: vec![Operation {
                    id: "execution".to_string(),
                    parent_id: None,
                    name: None,
                    operation_type: OperationType::Execution,
                    sub_type: None,
                    status: OperationStatus::Started,
                    step_details: None,
                    callback_details: None,
                    wait_details: None,
                    execution_details: Some(ExecutionDetails {
                        input_payload: Some("{}".to_string()),
                        output_payload: None,
                    }),
                    context_details: None,
                    chained_invoke_details: None,
                }],
                next_marker: Some("page-1".to_string()),
            },
        };

        mock.expect_get_state(MockGetStateConfig {
            operations: vec![Operation {
                id: "step-1".to_string(),
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
            }],
            next_marker: None,
            error: None,
        });

        let ctx = ExecutionContext::new(&input, mock.clone(), None, true)
            .await
            .expect("execution context should initialize");

        assert_eq!(ctx.get_mode().await, ExecutionMode::Replay);

        let calls = mock.get_state_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].marker, "page-1");
        assert_eq!(calls[0].max_items, GET_STATE_MAX_ITEMS);
    }

    #[tokio::test]
    async fn test_execution_context_no_pagination_stays_execution_mode() {
        let mock = Arc::new(MockLambdaService::new());
        let input = DurableExecutionInvocationInput {
            durable_execution_arn: "arn:aws:lambda:us-east-1:123:function:durable".to_string(),
            checkpoint_token: "token-0".to_string(),
            initial_execution_state: crate::types::InitialExecutionState {
                operations: vec![Operation {
                    id: "execution".to_string(),
                    parent_id: None,
                    name: None,
                    operation_type: OperationType::Execution,
                    sub_type: None,
                    status: OperationStatus::Started,
                    step_details: None,
                    callback_details: None,
                    wait_details: None,
                    execution_details: Some(ExecutionDetails {
                        input_payload: Some("{}".to_string()),
                        output_payload: None,
                    }),
                    context_details: None,
                    chained_invoke_details: None,
                }],
                next_marker: None,
            },
        };

        let ctx = ExecutionContext::new(&input, mock.clone(), None, true)
            .await
            .expect("execution context should initialize");

        assert_eq!(ctx.get_mode().await, ExecutionMode::Execution);
        assert!(mock.get_state_calls().is_empty());
    }

    #[tokio::test]
    async fn test_set_mode_updates_execution_mode() {
        let mock = Arc::new(MockLambdaService::new());
        let input = DurableExecutionInvocationInput {
            durable_execution_arn: "arn:aws:lambda:us-east-1:123:function:durable".to_string(),
            checkpoint_token: "token-0".to_string(),
            initial_execution_state: crate::types::InitialExecutionState {
                operations: vec![Operation {
                    id: "execution".to_string(),
                    parent_id: None,
                    name: None,
                    operation_type: OperationType::Execution,
                    sub_type: None,
                    status: OperationStatus::Started,
                    step_details: None,
                    callback_details: None,
                    wait_details: None,
                    execution_details: Some(ExecutionDetails {
                        input_payload: Some("{}".to_string()),
                        output_payload: None,
                    }),
                    context_details: None,
                    chained_invoke_details: None,
                }],
                next_marker: None,
            },
        };

        let ctx = ExecutionContext::new(&input, mock, None, true)
            .await
            .expect("execution context should initialize");

        assert_eq!(ctx.get_mode().await, ExecutionMode::Execution);
        ctx.set_mode(ExecutionMode::Replay).await;
        assert_eq!(ctx.get_mode().await, ExecutionMode::Replay);
    }

    #[tokio::test]
    async fn test_get_step_data_returns_operation() {
        let mock = Arc::new(MockLambdaService::new());
        let input = DurableExecutionInvocationInput {
            durable_execution_arn: "arn:aws:lambda:us-east-1:123:function:durable".to_string(),
            checkpoint_token: "token-0".to_string(),
            initial_execution_state: crate::types::InitialExecutionState {
                operations: vec![
                    Operation {
                        id: "execution".to_string(),
                        parent_id: None,
                        name: None,
                        operation_type: OperationType::Execution,
                        sub_type: None,
                        status: OperationStatus::Started,
                        step_details: None,
                        callback_details: None,
                        wait_details: None,
                        execution_details: Some(ExecutionDetails {
                            input_payload: Some("{}".to_string()),
                            output_payload: None,
                        }),
                        context_details: None,
                        chained_invoke_details: None,
                    },
                    Operation {
                        id: "step-1".to_string(),
                        parent_id: None,
                        name: Some("step".to_string()),
                        operation_type: OperationType::Step,
                        sub_type: None,
                        status: OperationStatus::Succeeded,
                        step_details: None,
                        callback_details: None,
                        wait_details: None,
                        execution_details: None,
                        context_details: None,
                        chained_invoke_details: None,
                    },
                ],
                next_marker: None,
            },
        };

        let ctx = ExecutionContext::new(&input, mock, None, true)
            .await
            .expect("execution context should initialize");

        let op = ctx.get_step_data("step-1").await.expect("step data");
        assert_eq!(op.status, OperationStatus::Succeeded);
        assert_eq!(op.name.as_deref(), Some("step"));
    }

    #[tokio::test]
    async fn test_next_operation_id_uses_prefix() {
        let mock = Arc::new(MockLambdaService::new());
        let input = DurableExecutionInvocationInput {
            durable_execution_arn: "arn:aws:lambda:us-east-1:123:function:durable".to_string(),
            checkpoint_token: "token-0".to_string(),
            initial_execution_state: crate::types::InitialExecutionState {
                operations: vec![Operation {
                    id: "execution".to_string(),
                    parent_id: None,
                    name: None,
                    operation_type: OperationType::Execution,
                    sub_type: None,
                    status: OperationStatus::Started,
                    step_details: None,
                    callback_details: None,
                    wait_details: None,
                    execution_details: Some(ExecutionDetails {
                        input_payload: Some("{}".to_string()),
                        output_payload: None,
                    }),
                    context_details: None,
                    chained_invoke_details: None,
                }],
                next_marker: None,
            },
        };

        let ctx = ExecutionContext::new(&input, mock, None, true)
            .await
            .expect("execution context should initialize");

        assert_eq!(ctx.next_operation_id(Some("step")), "step_0");
        assert_eq!(ctx.next_operation_id(None), "op_1");
    }
}
