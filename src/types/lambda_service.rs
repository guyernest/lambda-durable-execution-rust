//! Lambda service abstraction for durable execution.

use crate::error::{DurableError, DurableResult};
use crate::types::{
    CallbackDetails, ChainedInvokeDetails, ContextDetails, ExecutionDetails, FlexibleTimestamp,
    Operation, OperationAction, OperationStatus, OperationType, OperationUpdate, StepDetails,
    WaitDetails,
};
use async_trait::async_trait;
use aws_sdk_lambda::error::SdkError;
use aws_sdk_lambda::operation::checkpoint_durable_execution::CheckpointDurableExecutionError;
use aws_sdk_lambda::Client as LambdaClient;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::warn;

/// Execution state returned from Lambda durable execution APIs.
#[derive(Debug, Clone)]
pub struct ExecutionState {
    /// List of operations in the state payload.
    pub operations: Vec<Operation>,
    /// Pagination marker for additional pages, if any.
    pub next_marker: Option<String>,
}

/// Response from a checkpoint request.
#[derive(Debug, Clone)]
pub struct CheckpointResponse {
    /// Updated checkpoint token.
    pub checkpoint_token: Option<String>,
    /// Updated execution state, if returned by the service.
    pub new_execution_state: Option<ExecutionState>,
}

/// Response from a GetDurableExecutionState request.
#[derive(Debug, Clone)]
pub struct GetStateResponse {
    /// Operations returned in this page.
    pub operations: Vec<Operation>,
    /// Pagination marker for additional pages, if any.
    pub next_marker: Option<String>,
}

/// Abstraction over Lambda durable execution API calls for testability.
#[async_trait]
pub trait LambdaService: Send + Sync + Debug {
    /// Send checkpoint updates for a durable execution.
    async fn checkpoint_durable_execution(
        &self,
        durable_execution_arn: &str,
        checkpoint_token: &str,
        updates: Vec<OperationUpdate>,
    ) -> DurableResult<CheckpointResponse>;

    /// Fetch a page of durable execution state.
    async fn get_durable_execution_state(
        &self,
        durable_execution_arn: &str,
        checkpoint_token: &str,
        marker: &str,
        max_items: i32,
    ) -> DurableResult<GetStateResponse>;
}

#[cfg(test)]
mod tests {
    use super::mock::{MockCheckpointConfig, MockGetStateConfig, MockLambdaService};
    use super::{
        is_recoverable_message, sdk_error_object_to_error_object,
        sdk_operation_status_to_operation_status, sdk_operation_to_operation,
        sdk_operation_type_to_operation_type, to_sdk_operation_update, LambdaService,
    };
    use crate::error::{DurableError, ErrorObject};
    use crate::types::{
        CallbackUpdateOptions, ChainedInvokeUpdateOptions, ContextUpdateOptions, FlexibleTimestamp,
        OperationAction, OperationStatus, OperationType, OperationUpdate, StepUpdateOptions,
        WaitUpdateOptions,
    };
    use aws_sdk_lambda::types as sdk_types;
    use aws_smithy_types::DateTime;

    #[tokio::test]
    async fn test_mock_service_requires_responses() {
        let mock = MockLambdaService::new();

        let update = OperationUpdate::builder()
            .id("op-1")
            .operation_type(OperationType::Step)
            .action(OperationAction::Start)
            .build()
            .unwrap();

        let err = mock
            .checkpoint_durable_execution(
                "arn:aws:lambda:us-east-1:123:function:durable",
                "t",
                vec![update],
            )
            .await
            .expect_err("missing responses should error");

        match err {
            DurableError::Internal(message) => {
                assert!(message.contains("no checkpoint responses queued"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_mock_service_records_calls() {
        let mock = MockLambdaService::new();

        mock.expect_checkpoint(MockCheckpointConfig::default());

        let update = OperationUpdate::builder()
            .id("op-1")
            .operation_type(OperationType::Step)
            .action(OperationAction::Start)
            .build()
            .unwrap();

        mock.checkpoint_durable_execution(
            "arn:aws:lambda:us-east-1:123:function:durable",
            "token-1",
            vec![update],
        )
        .await
        .expect("checkpoint should succeed");

        let calls = mock.checkpoint_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].checkpoint_token, "token-1");
    }

    #[tokio::test]
    async fn test_mock_get_state_requires_responses() {
        let mock = MockLambdaService::new();

        let err = mock
            .get_durable_execution_state(
                "arn:aws:lambda:us-east-1:123:function:durable",
                "token-1",
                "marker-1",
                50,
            )
            .await
            .expect_err("missing responses should error");

        match err {
            DurableError::Internal(message) => {
                assert!(message.contains("no get state responses queued"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_mock_get_state_records_calls() {
        let mock = MockLambdaService::new();

        mock.expect_get_state(MockGetStateConfig::default());

        mock.get_durable_execution_state(
            "arn:aws:lambda:us-east-1:123:function:durable",
            "token-1",
            "marker-1",
            50,
        )
        .await
        .expect("get state should succeed");

        let calls = mock.get_state_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].checkpoint_token, "token-1");
        assert_eq!(calls[0].marker, "marker-1");
        assert_eq!(calls[0].max_items, 50);
    }

    #[test]
    fn test_to_sdk_operation_update_maps_fields() {
        let update = OperationUpdate::builder()
            .id("op-1")
            .parent_id("parent-1")
            .name("step-1")
            .sub_type("sub")
            .operation_type(OperationType::Step)
            .action(OperationAction::Start)
            .payload("{\"ok\":true}")
            .error(ErrorObject {
                error_type: "Type".to_string(),
                error_message: "Message".to_string(),
                details: Some("detail".to_string()),
            })
            .context_options(ContextUpdateOptions {
                replay_children: Some(true),
            })
            .step_options(StepUpdateOptions {
                next_attempt_delay_seconds: Some(7),
            })
            .wait_options(WaitUpdateOptions {
                wait_seconds: Some(12),
            })
            .callback_options(CallbackUpdateOptions {
                timeout_seconds: Some(30),
                heartbeat_timeout_seconds: Some(5),
            })
            .chained_invoke_options(ChainedInvokeUpdateOptions {
                function_name: "fn-arn".to_string(),
                tenant_id: Some("tenant-1".to_string()),
            })
            .build()
            .unwrap();

        let sdk_update = to_sdk_operation_update(update).expect("sdk update");
        assert_eq!(sdk_update.id(), "op-1");
        assert_eq!(sdk_update.parent_id(), Some("parent-1"));
        assert_eq!(sdk_update.name(), Some("step-1"));
        assert_eq!(sdk_update.sub_type(), Some("sub"));
        assert_eq!(sdk_update.payload(), Some("{\"ok\":true}"));
        assert_eq!(sdk_update.r#type(), &sdk_types::OperationType::Step);
        assert_eq!(sdk_update.action(), &sdk_types::OperationAction::Start);

        let error = sdk_update.error().expect("error");
        assert_eq!(error.error_type(), Some("Type"));
        assert_eq!(error.error_message(), Some("Message"));
        assert_eq!(error.error_data(), None);

        let ctx = sdk_update.context_options().expect("context options");
        assert_eq!(ctx.replay_children(), Some(true));

        let step = sdk_update.step_options().expect("step options");
        assert_eq!(step.next_attempt_delay_seconds(), Some(7));

        let wait = sdk_update.wait_options().expect("wait options");
        assert_eq!(wait.wait_seconds(), Some(12));

        let callback = sdk_update.callback_options().expect("callback options");
        assert_eq!(callback.timeout_seconds(), 30);
        assert_eq!(callback.heartbeat_timeout_seconds(), 5);

        let chained = sdk_update.chained_invoke_options().expect("chained invoke");
        assert_eq!(chained.function_name(), "fn-arn");
        assert_eq!(chained.tenant_id(), Some("tenant-1"));
    }

    #[test]
    fn test_sdk_operation_to_operation_maps_details() {
        let error = sdk_types::ErrorObject::builder()
            .error_type("Type")
            .error_message("Message")
            .error_data("data")
            .stack_trace("trace-1")
            .build();

        let step_details = sdk_types::StepDetails::builder()
            .attempt(2)
            .next_attempt_timestamp(DateTime::from_secs(123))
            .result("{\"ok\":true}")
            .error(error.clone())
            .build();

        let wait_details = sdk_types::WaitDetails::builder()
            .scheduled_end_timestamp(DateTime::from_secs(456))
            .build();

        let callback_details = sdk_types::CallbackDetails::builder()
            .callback_id("cb-1")
            .result("{\"cb\":true}")
            .error(error.clone())
            .build();

        let execution_details = sdk_types::ExecutionDetails::builder()
            .input_payload("{\"input\":1}")
            .build();

        let context_details = sdk_types::ContextDetails::builder()
            .replay_children(true)
            .result("{\"ctx\":true}")
            .error(error.clone())
            .build();

        let chained_invoke_details = sdk_types::ChainedInvokeDetails::builder()
            .result("{\"invoke\":true}")
            .error(error.clone())
            .build();

        let op = sdk_types::Operation::builder()
            .id("op-1")
            .r#type(sdk_types::OperationType::Step)
            .status(sdk_types::OperationStatus::Succeeded)
            .start_timestamp(DateTime::from_secs(0))
            .step_details(step_details)
            .wait_details(wait_details)
            .callback_details(callback_details)
            .execution_details(execution_details)
            .context_details(context_details)
            .chained_invoke_details(chained_invoke_details)
            .build()
            .unwrap();

        let converted = sdk_operation_to_operation(&op).expect("convert");
        assert_eq!(converted.id, "op-1");
        assert_eq!(converted.operation_type, OperationType::Step);
        assert_eq!(converted.status, OperationStatus::Succeeded);

        let step_details = converted.step_details.expect("step details");
        assert_eq!(step_details.attempt, Some(2));
        assert_eq!(step_details.result.as_deref(), Some("{\"ok\":true}"));
        let step_error = step_details.error.expect("step error");
        assert_eq!(step_error.error_type, "Type");
        assert_eq!(step_error.error_message, "Message");
        assert_eq!(step_error.details.as_deref(), Some("data"));

        let callback_details = converted.callback_details.expect("callback details");
        assert_eq!(callback_details.callback_id.as_deref(), Some("cb-1"));
        assert_eq!(callback_details.result.as_deref(), Some("{\"cb\":true}"));

        let wait_details = converted.wait_details.expect("wait details");
        match wait_details.scheduled_end_timestamp {
            Some(FlexibleTimestamp::String(ts)) => assert!(!ts.is_empty()),
            other => panic!("unexpected wait timestamp: {other:?}"),
        }

        let execution_details = converted.execution_details.expect("execution details");
        assert_eq!(
            execution_details.input_payload.as_deref(),
            Some("{\"input\":1}")
        );
        assert!(execution_details.output_payload.is_none());

        let context_details = converted.context_details.expect("context details");
        assert_eq!(context_details.replay_children, Some(true));
        assert_eq!(context_details.result.as_deref(), Some("{\"ctx\":true}"));

        let chained_details = converted
            .chained_invoke_details
            .expect("chained invoke details");
        assert_eq!(chained_details.result.as_deref(), Some("{\"invoke\":true}"));
    }

    #[test]
    fn test_sdk_error_object_to_error_object_defaults_and_details() {
        let error = sdk_types::ErrorObject::builder()
            .stack_trace("trace-1")
            .stack_trace("trace-2")
            .build();

        let converted = sdk_error_object_to_error_object(&error);
        assert_eq!(converted.error_type, "Error");
        assert_eq!(converted.error_message, "Unknown error");
        assert_eq!(converted.details.as_deref(), Some("trace-1\ntrace-2"));
    }

    #[test]
    fn test_sdk_operation_type_and_status_unknown_defaults() {
        let op_type = sdk_types::OperationType::from("NEW_TYPE");
        let status = sdk_types::OperationStatus::from("NEW_STATUS");

        assert_eq!(
            sdk_operation_type_to_operation_type(op_type),
            OperationType::Step
        );
        assert_eq!(
            sdk_operation_status_to_operation_status(status),
            OperationStatus::Unknown
        );
    }

    #[test]
    fn test_is_recoverable_message_matches_keywords() {
        assert!(is_recoverable_message("Rate exceeded"));
        assert!(is_recoverable_message("temporary error"));
        assert!(is_recoverable_message("Request timeout"));
        assert!(!is_recoverable_message("access denied"));
    }
}

/// Real Lambda service implementation backed by the AWS SDK client.
#[derive(Clone)]
pub struct RealLambdaService {
    client: Arc<LambdaClient>,
}

impl RealLambdaService {
    /// Create a new Lambda service wrapper.
    pub fn new(client: Arc<LambdaClient>) -> Self {
        Self { client }
    }
}

impl std::fmt::Debug for RealLambdaService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RealLambdaService").finish_non_exhaustive()
    }
}

#[async_trait]
impl LambdaService for RealLambdaService {
    async fn checkpoint_durable_execution(
        &self,
        durable_execution_arn: &str,
        checkpoint_token: &str,
        updates: Vec<OperationUpdate>,
    ) -> DurableResult<CheckpointResponse> {
        let sdk_updates = if updates.is_empty() {
            None
        } else {
            Some(
                updates
                    .into_iter()
                    .map(to_sdk_operation_update)
                    .collect::<DurableResult<Vec<_>>>()?,
            )
        };

        let response = self
            .client
            .checkpoint_durable_execution()
            .durable_execution_arn(durable_execution_arn)
            .checkpoint_token(checkpoint_token)
            .set_updates(sdk_updates)
            .send()
            .await
            .map_err(|e| {
                let is_recoverable = is_recoverable_error(&e);
                let debug = format!("{e:?}");
                DurableError::checkpoint_failed(
                    format!("Failed to checkpoint: {debug}"),
                    is_recoverable,
                    Some(e),
                )
            })?;

        let new_execution_state = match response.new_execution_state {
            Some(state) => {
                let operations = state
                    .operations
                    .unwrap_or_default()
                    .into_iter()
                    .map(|op| sdk_operation_to_operation(&op))
                    .collect::<DurableResult<Vec<_>>>()?;
                Some(ExecutionState {
                    operations,
                    next_marker: state.next_marker,
                })
            }
            None => None,
        };

        Ok(CheckpointResponse {
            checkpoint_token: response.checkpoint_token,
            new_execution_state,
        })
    }

    async fn get_durable_execution_state(
        &self,
        durable_execution_arn: &str,
        checkpoint_token: &str,
        marker: &str,
        max_items: i32,
    ) -> DurableResult<GetStateResponse> {
        let response = self
            .client
            .get_durable_execution_state()
            .durable_execution_arn(durable_execution_arn)
            .checkpoint_token(checkpoint_token)
            .marker(marker)
            .max_items(max_items)
            .send()
            .await
            .map_err(DurableError::aws_sdk)?;

        let operations = response
            .operations
            .into_iter()
            .map(|op| sdk_operation_to_operation(&op))
            .collect::<DurableResult<Vec<_>>>()?;

        Ok(GetStateResponse {
            operations,
            next_marker: response.next_marker,
        })
    }
}

fn is_recoverable_error(error: &SdkError<CheckpointDurableExecutionError>) -> bool {
    match error {
        SdkError::TimeoutError(_) | SdkError::DispatchFailure(_) | SdkError::ResponseError(_) => {
            true
        }
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

                    is_recoverable_message(&message)
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
                    is_recoverable_message(&message)
                }
            }
        }
        _ => is_recoverable_message(&error.to_string()),
    }
}

fn is_recoverable_message(message: &str) -> bool {
    let error_str = message.to_lowercase();
    error_str.contains("throttl")
        || error_str.contains("rate")
        || error_str.contains("timeout")
        || error_str.contains("temporary")
}

fn to_sdk_operation_update(
    update: OperationUpdate,
) -> DurableResult<aws_sdk_lambda::types::OperationUpdate> {
    let builder = aws_sdk_lambda::types::OperationUpdate::builder()
        .id(&update.id)
        .r#type(to_sdk_operation_type(update.operation_type))
        .action(to_sdk_operation_action(update.action));

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
            DurableError::Internal(format!("Failed to build chained invoke options: {e}"))
        })?;
        builder.chained_invoke_options(opts)
    } else {
        builder
    };

    builder
        .build()
        .map_err(|e| DurableError::Internal(format!("Failed to build operation update: {e}")))
}

fn to_sdk_operation_type(op_type: OperationType) -> aws_sdk_lambda::types::OperationType {
    match op_type {
        OperationType::Step => aws_sdk_lambda::types::OperationType::Step,
        OperationType::Wait => aws_sdk_lambda::types::OperationType::Wait,
        OperationType::Callback => aws_sdk_lambda::types::OperationType::Callback,
        OperationType::ChainedInvoke => aws_sdk_lambda::types::OperationType::ChainedInvoke,
        OperationType::Context => aws_sdk_lambda::types::OperationType::Context,
        OperationType::Execution => aws_sdk_lambda::types::OperationType::Execution,
    }
}

fn to_sdk_operation_action(action: OperationAction) -> aws_sdk_lambda::types::OperationAction {
    match action {
        OperationAction::Start => aws_sdk_lambda::types::OperationAction::Start,
        OperationAction::Retry => aws_sdk_lambda::types::OperationAction::Retry,
        OperationAction::Succeed => aws_sdk_lambda::types::OperationAction::Succeed,
        OperationAction::Fail => aws_sdk_lambda::types::OperationAction::Fail,
        OperationAction::Cancel => aws_sdk_lambda::types::OperationAction::Cancel,
    }
}

fn sdk_operation_to_operation(op: &aws_sdk_lambda::types::Operation) -> DurableResult<Operation> {
    Ok(Operation {
        id: op.id.clone(),
        parent_id: op.parent_id.clone(),
        name: op.name.clone(),
        operation_type: sdk_operation_type_to_operation_type(op.r#type.clone()),
        sub_type: op.sub_type.clone(),
        status: sdk_operation_status_to_operation_status(op.status.clone()),
        step_details: op.step_details.as_ref().map(|d| StepDetails {
            attempt: Some(d.attempt as u32),
            next_attempt_timestamp: d
                .next_attempt_timestamp
                .as_ref()
                .map(|ts| FlexibleTimestamp::String(ts.to_string())),
            result: d.result.clone(),
            error: d.error.as_ref().map(sdk_error_object_to_error_object),
        }),
        callback_details: op.callback_details.as_ref().map(|d| CallbackDetails {
            callback_id: d.callback_id.clone(),
            result: d.result.clone(),
            error: d.error.as_ref().map(sdk_error_object_to_error_object),
        }),
        wait_details: op.wait_details.as_ref().map(|d| WaitDetails {
            scheduled_end_timestamp: d
                .scheduled_end_timestamp
                .as_ref()
                .map(|ts| FlexibleTimestamp::String(ts.to_string())),
        }),
        execution_details: op.execution_details.as_ref().map(|d| ExecutionDetails {
            input_payload: d.input_payload.clone(),
            output_payload: None,
        }),
        context_details: op.context_details.as_ref().map(|d| ContextDetails {
            replay_children: d.replay_children,
            result: d.result.clone(),
            error: d.error.as_ref().map(sdk_error_object_to_error_object),
        }),
        chained_invoke_details: op
            .chained_invoke_details
            .as_ref()
            .map(|d| ChainedInvokeDetails {
                result: d.result.clone(),
                error: d.error.as_ref().map(sdk_error_object_to_error_object),
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

fn sdk_operation_type_to_operation_type(
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

fn sdk_operation_status_to_operation_status(
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
                "Unknown SDK operation status {:?}, defaulting to Unknown",
                status
            );
            OperationStatus::Unknown
        }
    }
}

/// Mock Lambda service implementations for tests.
#[cfg(any(test, feature = "testutils"))]
pub mod mock {
    use super::{CheckpointResponse, ExecutionState, GetStateResponse, LambdaService};
    use crate::error::{DurableError, DurableResult};
    use crate::types::{Operation, OperationUpdate};
    use async_trait::async_trait;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Expected response for a checkpoint call.
    #[derive(Debug, Default)]
    pub struct MockCheckpointConfig {
        /// Optional checkpoint token to return.
        pub checkpoint_token: Option<String>,
        /// Operations to return in the new execution state.
        pub operations: Vec<Operation>,
        /// Optional next marker for the new execution state.
        pub next_marker: Option<String>,
        /// Optional error to return instead of success.
        pub error: Option<DurableError>,
    }

    /// Expected response for a get state call.
    #[derive(Debug, Default)]
    pub struct MockGetStateConfig {
        /// Operations to return in the state page.
        pub operations: Vec<Operation>,
        /// Optional next marker for pagination.
        pub next_marker: Option<String>,
        /// Optional error to return instead of success.
        pub error: Option<DurableError>,
    }

    /// Recorded checkpoint call parameters.
    #[derive(Debug, Clone)]
    pub struct CheckpointCall {
        /// ARN of the durable execution.
        pub durable_execution_arn: String,
        /// Checkpoint token provided in the call.
        pub checkpoint_token: String,
        /// Updates passed to the checkpoint call.
        pub updates: Vec<OperationUpdate>,
    }

    /// Recorded get state call parameters.
    #[derive(Debug, Clone)]
    pub struct GetStateCall {
        /// ARN of the durable execution.
        pub durable_execution_arn: String,
        /// Checkpoint token provided in the call.
        pub checkpoint_token: String,
        /// Marker passed for pagination.
        pub marker: String,
        /// Max items requested for the page.
        pub max_items: i32,
    }

    /// Mock Lambda service for unit tests.
    #[derive(Debug, Default)]
    pub struct MockLambdaService {
        checkpoint_responses: Mutex<VecDeque<MockCheckpointConfig>>,
        get_state_responses: Mutex<VecDeque<MockGetStateConfig>>,
        checkpoint_calls: Mutex<Vec<CheckpointCall>>,
        get_state_calls: Mutex<Vec<GetStateCall>>,
    }

    impl MockLambdaService {
        /// Create a new mock Lambda service.
        pub fn new() -> Self {
            Self::default()
        }

        /// Queue an expected checkpoint response.
        pub fn expect_checkpoint(&self, config: MockCheckpointConfig) {
            self.checkpoint_responses
                .lock()
                .expect("checkpoint responses mutex")
                .push_back(config);
        }

        /// Queue an expected get state response.
        pub fn expect_get_state(&self, config: MockGetStateConfig) {
            self.get_state_responses
                .lock()
                .expect("get state responses mutex")
                .push_back(config);
        }

        /// Return recorded checkpoint calls.
        pub fn checkpoint_calls(&self) -> Vec<CheckpointCall> {
            self.checkpoint_calls
                .lock()
                .expect("checkpoint calls mutex")
                .clone()
        }

        /// Return recorded get state calls.
        pub fn get_state_calls(&self) -> Vec<GetStateCall> {
            self.get_state_calls
                .lock()
                .expect("get state calls mutex")
                .clone()
        }
    }

    #[async_trait]
    impl LambdaService for MockLambdaService {
        async fn checkpoint_durable_execution(
            &self,
            durable_execution_arn: &str,
            checkpoint_token: &str,
            updates: Vec<OperationUpdate>,
        ) -> DurableResult<CheckpointResponse> {
            self.checkpoint_calls
                .lock()
                .expect("checkpoint calls mutex")
                .push(CheckpointCall {
                    durable_execution_arn: durable_execution_arn.to_string(),
                    checkpoint_token: checkpoint_token.to_string(),
                    updates,
                });

            let config = self
                .checkpoint_responses
                .lock()
                .expect("checkpoint responses mutex")
                .pop_front()
                .ok_or_else(|| {
                    DurableError::Internal(
                        "MockLambdaService: no checkpoint responses queued".to_string(),
                    )
                })?;

            if let Some(error) = config.error {
                return Err(error);
            }

            Ok(CheckpointResponse {
                checkpoint_token: config.checkpoint_token,
                new_execution_state: Some(ExecutionState {
                    operations: config.operations,
                    next_marker: config.next_marker,
                }),
            })
        }

        async fn get_durable_execution_state(
            &self,
            durable_execution_arn: &str,
            checkpoint_token: &str,
            marker: &str,
            max_items: i32,
        ) -> DurableResult<GetStateResponse> {
            self.get_state_calls
                .lock()
                .expect("get state calls mutex")
                .push(GetStateCall {
                    durable_execution_arn: durable_execution_arn.to_string(),
                    checkpoint_token: checkpoint_token.to_string(),
                    marker: marker.to_string(),
                    max_items,
                });

            let config = self
                .get_state_responses
                .lock()
                .expect("get state responses mutex")
                .pop_front()
                .ok_or_else(|| {
                    DurableError::Internal(
                        "MockLambdaService: no get state responses queued".to_string(),
                    )
                })?;

            if let Some(error) = config.error {
                return Err(error);
            }

            Ok(GetStateResponse {
                operations: config.operations,
                next_marker: config.next_marker,
            })
        }
    }
}
