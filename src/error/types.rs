//! Error types for durable execution operations.
//!
//! This module defines all error types used by the durable execution SDK.
//! The main error type is [`DurableError`], which covers all failure modes.
//!
//! # Error Recovery
//!
//! Some errors are recoverable (the Lambda service will retry), while others
//! are terminal. Use the helper methods to determine how to handle errors:
//!
//! ```rust,no_run
//! # use lambda_durable_execution_rust::prelude::*;
//! fn example(result: DurableResult<u32>) -> DurableResult<u32> {
//!     match result {
//!         Ok(value) => Ok(value),
//!         Err(error) => {
//!             if error.is_recoverable() {
//!                 // Lambda will retry automatically
//!                 return Err(error);
//!             }
//!             if error.should_terminate_lambda() {
//!                 // Fatal error, must be fixed
//!                 panic!("Fatal error: {}", error);
//!             }
//!             // Handle other errors gracefully
//!             Ok(0)
//!         }
//!     }
//! }
//! ```

use crate::types::Duration;
use std::sync::Arc;
use thiserror::Error;

/// A boxed error type for generic error handling.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Result type alias for durable operations.
pub type DurableResult<T> = Result<T, DurableError>;

/// Main error type for durable execution operations.
///
/// This enum categorizes all possible errors that can occur during durable
/// execution, from step failures to checkpoint errors.
///
/// # Error Categories
///
/// | Category | Recoverable | Description |
/// |----------|-------------|-------------|
/// | [`StepFailed`](Self::StepFailed) | No | Step exhausted all retry attempts |
/// | [`CallbackTimeout`](Self::CallbackTimeout) | No | Callback wasn't completed in time |
/// | [`CallbackFailed`](Self::CallbackFailed) | No | External system failed callback |
/// | [`CheckpointFailed`](Self::CheckpointFailed) | Maybe | Checkpoint save failed |
/// | [`ReplayValidationFailed`](Self::ReplayValidationFailed) | No | Non-determinism detected |
///
/// # Handling Errors
///
/// ```rust,no_run
/// use lambda_durable_execution_rust::error::DurableError;
///
/// fn example(error: DurableError) {
///     match error {
///         DurableError::StepFailed { name, attempts, .. } => {
///             eprintln!("Step {} failed after {} attempts", name, attempts);
///         }
///         DurableError::CallbackTimeout { name, duration } => {
///             eprintln!("Callback {} timed out after {:?}", name, duration);
///         }
///         DurableError::CheckpointFailed { recoverable: true, .. } => {
///             eprintln!("Checkpoint failed (recoverable), Lambda will retry");
///         }
///         DurableError::ReplayValidationFailed { expected, actual } => {
///             eprintln!("Non-determinism: expected {}, got {}", expected, actual);
///         }
///         _ => {
///             eprintln!("Unexpected error: {}", error);
///         }
///     }
/// }
/// ```
#[derive(Error, Debug)]
pub enum DurableError {
    /// Step execution failed after all retry attempts were exhausted.
    #[error("Step '{name}' failed after {attempts} attempt(s): {message}")]
    StepFailed {
        /// Name of the step that failed.
        name: String,
        /// Number of attempts made before giving up.
        attempts: u32,
        /// Error message from the final attempt.
        message: String,
        /// The underlying error, if available.
        #[source]
        source: Option<Arc<BoxError>>,
    },

    /// Callback operation timed out waiting for external system response.
    #[error("Callback '{name}' timed out after {duration}")]
    CallbackTimeout {
        /// Name of the callback operation.
        name: String,
        /// Duration waited before timeout.
        duration: Duration,
    },

    /// Callback was explicitly failed by an external system.
    #[error("Callback '{name}' failed: {message}")]
    CallbackFailed {
        /// Name of the callback operation.
        name: String,
        /// Error message from the external system.
        message: String,
    },

    /// Lambda invocation of another function failed.
    #[error("Lambda invocation of '{function}' failed: {message}")]
    InvocationFailed {
        /// ARN or name of the invoked function.
        function: String,
        /// Error message.
        message: String,
        /// The underlying error, if available.
        #[source]
        source: Option<Arc<BoxError>>,
    },

    /// Checkpoint operation failed (may be recoverable or unrecoverable).
    #[error("Checkpoint failed: {message}")]
    CheckpointFailed {
        /// Description of the checkpoint failure.
        message: String,
        /// Whether this error is recoverable (Lambda will retry).
        recoverable: bool,
        /// The underlying error, if available.
        #[source]
        source: Option<Arc<BoxError>>,
    },

    /// Serialization or deserialization of operation data failed.
    #[error("Serialization failed for operation '{operation}': {message}")]
    SerializationFailed {
        /// Name of the operation being serialized.
        operation: String,
        /// Error message.
        message: String,
        /// The underlying serde_json error.
        #[source]
        source: Option<serde_json::Error>,
    },

    /// Replay validation detected non-deterministic behavior.
    #[error("Replay validation failed: expected {expected}, got {actual}")]
    ReplayValidationFailed {
        /// What was expected based on checkpoint data.
        expected: String,
        /// What was actually encountered during replay.
        actual: String,
    },

    /// Child context operation failed.
    #[error("Child context '{name}' failed: {message}")]
    ChildContextFailed {
        /// Name of the child context.
        name: String,
        /// Error message.
        message: String,
        /// The underlying error, if available.
        #[source]
        source: Option<Arc<BoxError>>,
    },

    /// Parallel or map operation failed to meet completion requirements.
    #[error("Batch operation '{name}' failed: {message}")]
    BatchOperationFailed {
        /// Name of the batch operation.
        name: String,
        /// Error message.
        message: String,
        /// Number of successful operations.
        successful_count: usize,
        /// Number of failed operations.
        failed_count: usize,
    },

    /// Wait for condition operation exceeded maximum attempts.
    #[error("Wait for condition '{name}' exceeded maximum attempts ({attempts})")]
    WaitConditionExceeded {
        /// Name of the wait operation.
        name: String,
        /// Number of attempts made.
        attempts: u32,
    },

    /// Invalid configuration or input.
    #[error("Invalid configuration: {message}")]
    InvalidConfiguration {
        /// Description of the configuration error.
        message: String,
    },

    /// Context was used incorrectly (e.g., parent context used in child operation).
    #[error("Context validation error: {message}")]
    ContextValidationError {
        /// Description of the validation error.
        message: String,
    },

    /// Internal SDK error that should not normally occur.
    #[error("Internal SDK error: {0}")]
    Internal(String),

    /// AWS SDK error wrapper.
    #[error("AWS SDK error: {message}")]
    AwsSdk {
        /// Error message.
        message: String,
        /// The underlying AWS SDK error.
        #[source]
        source: Option<Arc<BoxError>>,
    },
}

impl DurableError {
    /// Create a new step failed error.
    pub fn step_failed(
        name: impl Into<String>,
        attempts: u32,
        error: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::StepFailed {
            name: name.into(),
            attempts,
            message: error.to_string(),
            source: Some(Arc::new(Box::new(error))),
        }
    }

    /// Create a new step failed error with just a message.
    pub fn step_failed_msg(
        name: impl Into<String>,
        attempts: u32,
        message: impl Into<String>,
    ) -> Self {
        Self::StepFailed {
            name: name.into(),
            attempts,
            message: message.into(),
            source: None,
        }
    }

    /// Create a new step failed error from a boxed error.
    pub fn step_failed_boxed(
        name: impl Into<String>,
        attempts: u32,
        error: Box<dyn std::error::Error + Send + Sync>,
    ) -> Self {
        Self::StepFailed {
            name: name.into(),
            attempts,
            message: error.to_string(),
            source: Some(Arc::new(error)),
        }
    }

    /// Create a new serialization failed error.
    pub fn serialization_failed(operation: impl Into<String>, error: serde_json::Error) -> Self {
        Self::SerializationFailed {
            operation: operation.into(),
            message: error.to_string(),
            source: Some(error),
        }
    }

    /// Create a new checkpoint failed error.
    pub fn checkpoint_failed(
        message: impl Into<String>,
        recoverable: bool,
        error: Option<impl std::error::Error + Send + Sync + 'static>,
    ) -> Self {
        Self::CheckpointFailed {
            message: message.into(),
            recoverable,
            source: error.map(|e| Arc::new(Box::new(e) as BoxError)),
        }
    }

    /// Create a new AWS SDK error.
    pub fn aws_sdk(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::AwsSdk {
            message: error.to_string(),
            source: Some(Arc::new(Box::new(error))),
        }
    }

    /// Check if this error is recoverable (Lambda should retry).
    pub fn is_recoverable(&self) -> bool {
        match self {
            Self::CheckpointFailed { recoverable, .. } => *recoverable,
            Self::StepFailed { .. } => false, // Steps have their own retry logic
            Self::CallbackTimeout { .. } => false,
            Self::CallbackFailed { .. } => false,
            Self::SerializationFailed { .. } => false,
            Self::ReplayValidationFailed { .. } => false,
            Self::InvalidConfiguration { .. } => false,
            Self::ContextValidationError { .. } => false,
            _ => false,
        }
    }

    /// Check if this error should terminate the Lambda execution.
    pub fn should_terminate_lambda(&self) -> bool {
        match self {
            Self::CheckpointFailed { recoverable, .. } => !recoverable,
            Self::ReplayValidationFailed { .. } => true,
            Self::ContextValidationError { .. } => true,
            Self::Internal(_) => true,
            _ => false,
        }
    }
}

/// Represents an error object that can be serialized for checkpoint storage.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ErrorObject {
    /// Error type/name.
    #[serde(alias = "error_type", default = "ErrorObject::default_error_type")]
    pub error_type: String,
    /// Error message.
    #[serde(alias = "error_message", default)]
    pub error_message: String,
    /// Optional stack trace or additional details.
    #[serde(default)]
    pub details: Option<String>,
}

impl ErrorObject {
    fn default_error_type() -> String {
        "Error".to_string()
    }

    /// Create a new error object from a DurableError.
    pub fn from_durable_error(error: &DurableError) -> Self {
        Self {
            error_type: error_type_name(error),
            error_message: error.to_string(),
            details: None,
        }
    }

    /// Create a new error object from any error.
    pub fn from_error(error: &(dyn std::error::Error + Send + Sync)) -> Self {
        Self {
            error_type: "Error".to_string(),
            error_message: error.to_string(),
            details: error.source().map(|s| s.to_string()),
        }
    }
}

fn error_type_name(error: &DurableError) -> String {
    match error {
        DurableError::StepFailed { .. } => "StepFailed",
        DurableError::CallbackTimeout { .. } => "CallbackTimeout",
        DurableError::CallbackFailed { .. } => "CallbackFailed",
        DurableError::InvocationFailed { .. } => "InvocationFailed",
        DurableError::CheckpointFailed { .. } => "CheckpointFailed",
        DurableError::SerializationFailed { .. } => "SerializationFailed",
        DurableError::ReplayValidationFailed { .. } => "ReplayValidationFailed",
        DurableError::ChildContextFailed { .. } => "ChildContextFailed",
        DurableError::BatchOperationFailed { .. } => "BatchOperationFailed",
        DurableError::WaitConditionExceeded { .. } => "WaitConditionExceeded",
        DurableError::InvalidConfiguration { .. } => "InvalidConfiguration",
        DurableError::ContextValidationError { .. } => "ContextValidationError",
        DurableError::Internal(_) => "Internal",
        DurableError::AwsSdk { .. } => "AwsSdk",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;
    use std::fmt;

    #[test]
    fn test_step_failed_error() {
        let error = DurableError::step_failed_msg("my-step", 3, "connection timeout");
        assert!(error.to_string().contains("my-step"));
        assert!(error.to_string().contains("3"));
        assert!(!error.is_recoverable());
    }

    #[test]
    fn test_checkpoint_recoverable() {
        let recoverable =
            DurableError::checkpoint_failed("rate limited", true, None::<std::io::Error>);
        assert!(recoverable.is_recoverable());

        let unrecoverable =
            DurableError::checkpoint_failed("invalid token", false, None::<std::io::Error>);
        assert!(!unrecoverable.is_recoverable());
        assert!(unrecoverable.should_terminate_lambda());
    }

    #[test]
    fn test_error_object_serialization() {
        let error = DurableError::step_failed_msg("test", 1, "test error");
        let obj = ErrorObject::from_durable_error(&error);

        let json = serde_json::to_string(&obj).unwrap();
        assert!(json.contains("StepFailed"));

        let deserialized: ErrorObject = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.error_type, "StepFailed");
    }

    #[test]
    fn test_should_terminate_lambda_for_validation_errors() {
        let ctx_error = DurableError::ContextValidationError {
            message: "invalid context".to_string(),
        };
        assert!(ctx_error.should_terminate_lambda());

        let internal = DurableError::Internal("internal".to_string());
        assert!(internal.should_terminate_lambda());

        let step_error = DurableError::step_failed_msg("step", 1, "boom");
        assert!(!step_error.should_terminate_lambda());
    }

    #[derive(Debug)]
    struct OuterError {
        message: &'static str,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    }

    impl fmt::Display for OuterError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.message)
        }
    }

    impl Error for OuterError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            self.source
                .as_deref()
                .map(|err| err as &(dyn Error + 'static))
        }
    }

    #[test]
    fn test_error_object_from_error_includes_source() {
        let inner = std::io::Error::new(std::io::ErrorKind::Other, "inner");
        let err = OuterError {
            message: "outer",
            source: Some(Box::new(inner)),
        };

        let obj = ErrorObject::from_error(&err);
        assert_eq!(obj.error_type, "Error");
        assert_eq!(obj.error_message, "outer");
        assert_eq!(obj.details.as_deref(), Some("inner"));
    }

    #[test]
    fn test_error_object_defaults_missing_type() {
        let json = r#"{"ErrorMessage":"oops"}"#;
        let obj: ErrorObject = serde_json::from_str(json).unwrap();
        assert_eq!(obj.error_type, "Error");
        assert_eq!(obj.error_message, "oops");
    }

    #[test]
    fn test_aws_sdk_error_wraps_source() {
        let err = std::io::Error::other("sdk failure");
        let wrapped = DurableError::aws_sdk(err);

        match &wrapped {
            DurableError::AwsSdk { message, source } => {
                assert!(message.contains("sdk failure"));
                assert!(source.is_some());
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert!(!wrapped.is_recoverable());
    }

    #[test]
    fn test_serialization_failed_sets_source() {
        let err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        let failure = DurableError::serialization_failed("step", err);

        match &failure {
            DurableError::SerializationFailed {
                operation, source, ..
            } => {
                assert_eq!(operation, "step");
                assert!(source.is_some());
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert!(!failure.is_recoverable());
    }
}
