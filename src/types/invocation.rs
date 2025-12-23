//! Types for durable execution invocation input and output.

use crate::error::ErrorObject;
use serde::{Deserialize, Serialize};

/// Timestamp value that may be represented as either a string (ISO/RFC3339) or
/// a number (epoch milliseconds) in durable execution state payloads.
///
/// The durable execution service and different SDKs can emit timestamps in
/// different JSON representations. We accept both to avoid deserialization
/// failures during replay/resume.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FlexibleTimestamp {
    /// String timestamp (e.g., RFC3339).
    String(String),
    /// Numeric timestamp (epoch milliseconds).
    Millis(i64),
}

/// Status of a durable execution invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InvocationStatus {
    /// Execution completed successfully.
    Succeeded,
    /// Execution failed with an error.
    Failed,
    /// Execution is pending (Lambda terminated but workflow continues).
    Pending,
}

/// Input provided to a durable Lambda function by the runtime.
///
/// This structure is automatically deserialized from the Lambda event payload
/// when a durable function is invoked.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DurableExecutionInvocationInput {
    /// ARN of the durable execution.
    pub durable_execution_arn: String,

    /// Token for checkpointing state.
    pub checkpoint_token: String,

    /// Initial execution state containing operation history.
    pub initial_execution_state: InitialExecutionState,
}

/// Initial state of a durable execution, including operation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct InitialExecutionState {
    /// List of operations from previous invocations.
    #[serde(default)]
    pub operations: Vec<Operation>,

    /// Pagination marker for fetching more operations.
    ///
    /// The Durable Execution service and JS SDK use `NextMarker` for pagination.
    pub next_marker: Option<String>,
}

/// Represents a single operation in the execution history.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Operation {
    /// Unique identifier for this operation (hashed).
    pub id: String,

    /// Parent operation ID for operations in child contexts.
    pub parent_id: Option<String>,

    /// Customer-provided name for this operation.
    pub name: Option<String>,

    /// Type of operation (Step, Wait, Callback, etc.).
    #[serde(rename = "Type")]
    pub operation_type: OperationType,

    /// Subtype providing additional categorization.
    pub sub_type: Option<String>,

    /// Current status of the operation.
    pub status: OperationStatus,

    /// Details specific to step operations.
    pub step_details: Option<StepDetails>,

    /// Details specific to callback operations.
    pub callback_details: Option<CallbackDetails>,

    /// Details specific to wait operations.
    pub wait_details: Option<WaitDetails>,

    /// Details specific to execution operations.
    pub execution_details: Option<ExecutionDetails>,

    /// Details specific to context operations.
    pub context_details: Option<ContextDetails>,

    /// Details specific to chained invoke operations.
    pub chained_invoke_details: Option<ChainedInvokeDetails>,
}

/// Type of operation in a durable execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OperationType {
    /// Atomic step operation.
    Step,
    /// Time-based wait operation.
    Wait,
    /// External callback operation.
    Callback,
    /// Chained Lambda invocation.
    ChainedInvoke,
    /// Child context operation.
    Context,
    /// Top-level execution operation.
    Execution,
}

/// Status of an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OperationStatus {
    /// Operation is ready to start (scheduled but not yet started).
    Ready,
    /// Operation has started but not completed.
    Started,
    /// Operation is pending (waiting for something).
    Pending,
    /// Operation completed successfully.
    Succeeded,
    /// Operation failed.
    Failed,
    /// Unknown/forward-compatible status from the service.
    #[serde(other)]
    Unknown,
}

/// Details for step operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StepDetails {
    /// Current attempt number for this step.
    pub attempt: Option<u32>,

    /// Timestamp when the next attempt is scheduled (only when pending).
    pub next_attempt_timestamp: Option<FlexibleTimestamp>,

    /// JSON response payload from the step operation.
    pub result: Option<String>,

    /// Error information if the step failed.
    pub error: Option<ErrorObject>,
}

/// Details for callback operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CallbackDetails {
    /// Unique callback ID for external systems.
    pub callback_id: Option<String>,

    /// Result payload from external system.
    pub result: Option<String>,

    /// Error from external system.
    pub error: Option<ErrorObject>,
}

/// Details for wait operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct WaitDetails {
    /// Timestamp when wait is scheduled to complete.
    pub scheduled_end_timestamp: Option<FlexibleTimestamp>,
}

/// Details for execution operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ExecutionDetails {
    /// Input payload to the execution.
    pub input_payload: Option<String>,

    /// Output payload from the execution.
    pub output_payload: Option<String>,
}

/// Details for context operations (child contexts, map/parallel top-level).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ContextDetails {
    /// Whether to include child state in replay (service-controlled).
    pub replay_children: Option<bool>,

    /// Result payload from the context.
    pub result: Option<String>,

    /// Error if the context failed.
    pub error: Option<ErrorObject>,
}

/// Details for chained invoke operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ChainedInvokeDetails {
    /// Result payload from the invoked function.
    pub result: Option<String>,

    /// Error from the invoked function.
    pub error: Option<ErrorObject>,
}

/// Output from a durable Lambda function invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DurableExecutionInvocationOutput {
    /// Status of the invocation.
    pub status: InvocationStatus,

    /// Result payload (for successful completion).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,

    /// Error information (for failed completion).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorObject>,
}

impl DurableExecutionInvocationOutput {
    /// Create a successful output.
    pub fn succeeded(result: Option<String>) -> Self {
        Self {
            status: InvocationStatus::Succeeded,
            result,
            error: None,
        }
    }

    /// Create a failed output.
    pub fn failed(error: ErrorObject) -> Self {
        Self {
            status: InvocationStatus::Failed,
            result: None,
            error: Some(error),
        }
    }

    /// Create a pending output (Lambda terminates but workflow continues).
    pub fn pending() -> Self {
        Self {
            status: InvocationStatus::Pending,
            result: None,
            error: None,
        }
    }
}

/// Action to take on an operation during checkpointing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OperationAction {
    /// Start a new operation.
    Start,
    /// Retry an operation.
    Retry,
    /// Mark operation as succeeded.
    Succeed,
    /// Mark operation as failed.
    Fail,
    /// Cancel an operation.
    Cancel,
}

/// Update to be sent during checkpointing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct OperationUpdate {
    /// Operation ID (hashed).
    pub id: String,

    /// Parent operation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,

    /// Operation name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Operation type.
    #[serde(rename = "Type")]
    pub operation_type: OperationType,

    /// Operation subtype.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_type: Option<String>,

    /// Action to take.
    pub action: OperationAction,

    /// Payload for successful operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,

    /// Error for failed operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorObject>,

    /// Options for context operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_options: Option<ContextUpdateOptions>,

    /// Options for step operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_options: Option<StepUpdateOptions>,

    /// Options for wait operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait_options: Option<WaitUpdateOptions>,

    /// Options for callback operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_options: Option<CallbackUpdateOptions>,

    /// Options for chained invoke operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chained_invoke_options: Option<ChainedInvokeUpdateOptions>,
}

impl OperationUpdate {
    /// Create a builder for operation updates.
    pub fn builder() -> OperationUpdateBuilder {
        OperationUpdateBuilder::default()
    }
}

/// Builder for OperationUpdate.
#[derive(Debug, Clone, Default)]
pub struct OperationUpdateBuilder {
    id: Option<String>,
    parent_id: Option<String>,
    name: Option<String>,
    operation_type: Option<OperationType>,
    sub_type: Option<String>,
    action: Option<OperationAction>,
    payload: Option<String>,
    error: Option<ErrorObject>,
    context_options: Option<ContextUpdateOptions>,
    step_options: Option<StepUpdateOptions>,
    wait_options: Option<WaitUpdateOptions>,
    callback_options: Option<CallbackUpdateOptions>,
    chained_invoke_options: Option<ChainedInvokeUpdateOptions>,
}

impl OperationUpdateBuilder {
    /// Set the operation ID.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the parent operation ID.
    pub fn parent_id(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_id = Some(parent_id.into());
        self
    }

    /// Set the operation name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the operation type.
    pub fn operation_type(mut self, op_type: OperationType) -> Self {
        self.operation_type = Some(op_type);
        self
    }

    /// Set the operation subtype.
    pub fn sub_type(mut self, sub_type: impl Into<String>) -> Self {
        self.sub_type = Some(sub_type.into());
        self
    }

    /// Set the action.
    pub fn action(mut self, action: OperationAction) -> Self {
        self.action = Some(action);
        self
    }

    /// Set the payload.
    pub fn payload(mut self, payload: impl Into<String>) -> Self {
        self.payload = Some(payload.into());
        self
    }

    /// Set the error.
    pub fn error(mut self, error: ErrorObject) -> Self {
        self.error = Some(error);
        self
    }

    /// Set context options.
    pub fn context_options(mut self, options: ContextUpdateOptions) -> Self {
        self.context_options = Some(options);
        self
    }

    /// Set step options.
    pub fn step_options(mut self, options: StepUpdateOptions) -> Self {
        self.step_options = Some(options);
        self
    }

    /// Set wait options.
    pub fn wait_options(mut self, options: WaitUpdateOptions) -> Self {
        self.wait_options = Some(options);
        self
    }

    /// Set callback options.
    pub fn callback_options(mut self, options: CallbackUpdateOptions) -> Self {
        self.callback_options = Some(options);
        self
    }

    /// Set chained invoke options.
    pub fn chained_invoke_options(mut self, options: ChainedInvokeUpdateOptions) -> Self {
        self.chained_invoke_options = Some(options);
        self
    }

    /// Build the OperationUpdate.
    pub fn build(self) -> Result<OperationUpdate, &'static str> {
        if let Some(opts) = self.step_options.as_ref() {
            if let Some(secs) = opts.next_attempt_delay_seconds {
                if secs < 0 {
                    return Err("next_attempt_delay_seconds must be >= 0");
                }
            }
        }

        if let Some(opts) = self.wait_options.as_ref() {
            if let Some(secs) = opts.wait_seconds {
                if secs < 0 {
                    return Err("wait_seconds must be >= 0");
                }
            }
        }

        if let Some(opts) = self.callback_options.as_ref() {
            if let Some(secs) = opts.timeout_seconds {
                if secs < 0 {
                    return Err("timeout_seconds must be >= 0");
                }
            }
            if let Some(secs) = opts.heartbeat_timeout_seconds {
                if secs < 0 {
                    return Err("heartbeat_timeout_seconds must be >= 0");
                }
            }
        }

        if let Some(opts) = self.chained_invoke_options.as_ref() {
            if opts.function_name.trim().is_empty() {
                return Err("function_name must not be empty");
            }
        }

        Ok(OperationUpdate {
            id: self.id.ok_or("id is required")?,
            parent_id: self.parent_id,
            name: self.name,
            operation_type: self.operation_type.ok_or("operation_type is required")?,
            sub_type: self.sub_type,
            action: self.action.ok_or("action is required")?,
            payload: self.payload,
            error: self.error,
            context_options: self.context_options,
            step_options: self.step_options,
            wait_options: self.wait_options,
            callback_options: self.callback_options,
            chained_invoke_options: self.chained_invoke_options,
        })
    }
}

/// Options for context operation updates.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ContextUpdateOptions {
    /// Whether to include children state for replay.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay_children: Option<bool>,
}

/// Options for step operation updates.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StepUpdateOptions {
    /// Delay in seconds before the next retry attempt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_attempt_delay_seconds: Option<i32>,
}

/// Options for wait operation updates.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct WaitUpdateOptions {
    /// Duration to wait in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait_seconds: Option<i32>,
}

/// Options for callback operation updates.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CallbackUpdateOptions {
    /// Timeout in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<i32>,

    /// Heartbeat timeout in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heartbeat_timeout_seconds: Option<i32>,
}

/// Options for chained invoke operation updates.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ChainedInvokeUpdateOptions {
    /// Function name or ARN to invoke.
    pub function_name: String,

    /// Optional tenant identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invocation_input_deserialization() {
        let json = r#"{
            "DurableExecutionArn": "arn:aws:lambda:us-east-1:123456789012:function:my-function:durable:abc123",
            "CheckpointToken": "token123",
            "InitialExecutionState": {
                "Operations": [
                    {
                        "Id": "op-001",
                        "Type": "EXECUTION",
                        "Status": "STARTED",
                        "ExecutionDetails": {
                            "InputPayload": "{\"key\": \"value\"}"
                        }
                    }
                ]
            }
        }"#;

        let input: DurableExecutionInvocationInput = serde_json::from_str(json).unwrap();
        assert_eq!(
            input.durable_execution_arn,
            "arn:aws:lambda:us-east-1:123456789012:function:my-function:durable:abc123"
        );
        assert_eq!(input.checkpoint_token, "token123");
        assert_eq!(input.initial_execution_state.operations.len(), 1);
    }

    #[test]
    fn test_invocation_output_serialization() {
        let output =
            DurableExecutionInvocationOutput::succeeded(Some(r#"{"result": 42}"#.to_string()));
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("SUCCEEDED"));
        assert!(json.contains("result"));

        let pending = DurableExecutionInvocationOutput::pending();
        let json = serde_json::to_string(&pending).unwrap();
        assert!(json.contains("PENDING"));
    }

    #[test]
    fn test_operation_update_builder() {
        let update = OperationUpdate::builder()
            .id("op-123")
            .name("my-step")
            .operation_type(OperationType::Step)
            .sub_type("Step")
            .action(OperationAction::Start)
            .build()
            .unwrap();

        assert_eq!(update.id, "op-123");
        assert_eq!(update.name, Some("my-step".to_string()));
        assert_eq!(update.action, OperationAction::Start);
    }

    #[test]
    fn test_operation_update_builder_missing_required() {
        let result = OperationUpdate::builder().name("test").build();

        assert!(result.is_err());
    }

    #[test]
    fn test_operation_update_builder_rejects_negative_wait() {
        let update = OperationUpdate::builder()
            .id("op-1")
            .operation_type(OperationType::Wait)
            .action(OperationAction::Start)
            .wait_options(WaitUpdateOptions {
                wait_seconds: Some(-1),
            })
            .build();

        assert!(update.is_err());
    }

    #[test]
    fn test_operation_update_builder_rejects_negative_next_attempt_delay() {
        let update = OperationUpdate::builder()
            .id("op-1")
            .operation_type(OperationType::Step)
            .action(OperationAction::Retry)
            .step_options(StepUpdateOptions {
                next_attempt_delay_seconds: Some(-2),
            })
            .build();

        assert!(update.is_err());
    }

    #[test]
    fn test_flexible_timestamp_deserializes_multiple_formats() {
        let ts: FlexibleTimestamp = serde_json::from_str("\"2024-01-01T00:00:00Z\"").unwrap();
        match ts {
            FlexibleTimestamp::String(value) => assert!(value.contains("2024-01-01")),
            _ => panic!("expected string timestamp"),
        }

        let ts: FlexibleTimestamp = serde_json::from_str("1700000000000").unwrap();
        match ts {
            FlexibleTimestamp::Millis(value) => assert_eq!(value, 1700000000000),
            _ => panic!("expected millis timestamp"),
        }
    }

    #[test]
    fn test_operation_status_unknown_variant() {
        let status: OperationStatus = serde_json::from_str("\"FUTURE_STATUS\"").unwrap();
        assert_eq!(status, OperationStatus::Unknown);
    }

    #[test]
    fn test_invocation_output_failed_sets_error() {
        let error = ErrorObject {
            error_type: "Error".to_string(),
            error_message: "boom".to_string(),
            details: None,
        };
        let output = DurableExecutionInvocationOutput::failed(error);

        assert_eq!(output.status, InvocationStatus::Failed);
        let err = output.error.expect("error payload");
        assert_eq!(err.error_type, "Error");
        assert_eq!(err.error_message, "boom");
        assert!(output.result.is_none());
    }

    #[test]
    fn test_operation_update_builder_rejects_negative_timeout() {
        let update = OperationUpdate::builder()
            .id("op-1")
            .operation_type(OperationType::Callback)
            .action(OperationAction::Start)
            .callback_options(CallbackUpdateOptions {
                timeout_seconds: Some(-10),
                heartbeat_timeout_seconds: None,
            })
            .build();

        assert!(update.is_err());
    }

    #[test]
    fn test_operation_update_builder_rejects_negative_heartbeat_timeout() {
        let update = OperationUpdate::builder()
            .id("op-1")
            .operation_type(OperationType::Callback)
            .action(OperationAction::Start)
            .callback_options(CallbackUpdateOptions {
                timeout_seconds: None,
                heartbeat_timeout_seconds: Some(-5),
            })
            .build();

        assert!(update.is_err());
    }

    #[test]
    fn test_operation_update_builder_rejects_empty_invoke_name() {
        let update = OperationUpdate::builder()
            .id("op-1")
            .operation_type(OperationType::ChainedInvoke)
            .action(OperationAction::Start)
            .chained_invoke_options(ChainedInvokeUpdateOptions {
                function_name: "   ".to_string(),
                tenant_id: None,
            })
            .build();

        assert!(update.is_err());
    }
}
