//! Durable logging support.
//!
//! Provides a small logger trait that can be customized by users. The default
//! implementation routes to `tracing` and can suppress logs during replay.

use tracing::{debug, error, info, warn};

/// Log level for durable logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableLogLevel {
    /// Debug-level log.
    Debug,
    /// Info-level log.
    Info,
    /// Warning-level log.
    Warn,
    /// Error-level log.
    Error,
}

/// Metadata passed to loggers for durable operations.
#[derive(Debug, Clone)]
pub struct DurableLogData {
    /// ARN of the durable execution.
    pub durable_execution_arn: String,
    /// Hashed operation id, if available.
    pub operation_id: Option<String>,
    /// Optional customer-provided name.
    pub step_name: Option<String>,
    /// Attempt number, if known.
    pub attempt: Option<u32>,
}

/// Trait for custom durable loggers.
pub trait DurableLogger: Send + Sync {
    /// Generic logging entrypoint.
    fn log(
        &self,
        level: DurableLogLevel,
        data: &DurableLogData,
        message: &str,
        fields: Option<&[(&'static str, String)]>,
    );

    /// Log a debug message.
    fn debug(&self, data: &DurableLogData, message: &str) {
        self.log(DurableLogLevel::Debug, data, message, None);
    }
    /// Log an info message.
    fn info(&self, data: &DurableLogData, message: &str) {
        self.log(DurableLogLevel::Info, data, message, None);
    }
    /// Log a warning message.
    fn warn(&self, data: &DurableLogData, message: &str) {
        self.log(DurableLogLevel::Warn, data, message, None);
    }
    /// Log an error message.
    fn error(&self, data: &DurableLogData, message: &str) {
        self.log(DurableLogLevel::Error, data, message, None);
    }
}

/// Default logger that emits via `tracing`.
#[derive(Debug, Clone, Copy, Default)]
pub struct TracingLogger;

impl DurableLogger for TracingLogger {
    fn log(
        &self,
        level: DurableLogLevel,
        data: &DurableLogData,
        message: &str,
        fields: Option<&[(&'static str, String)]>,
    ) {
        match level {
            DurableLogLevel::Debug => debug!(
                durable_execution_arn = %data.durable_execution_arn,
                step_name = data.step_name.as_deref(),
                operation_id = data.operation_id.as_deref(),
                attempt = data.attempt,
                extra = ?fields,
                "{}",
                message
            ),
            DurableLogLevel::Info => info!(
                durable_execution_arn = %data.durable_execution_arn,
                step_name = data.step_name.as_deref(),
                operation_id = data.operation_id.as_deref(),
                attempt = data.attempt,
                extra = ?fields,
                "{}",
                message
            ),
            DurableLogLevel::Warn => warn!(
                durable_execution_arn = %data.durable_execution_arn,
                step_name = data.step_name.as_deref(),
                operation_id = data.operation_id.as_deref(),
                attempt = data.attempt,
                extra = ?fields,
                "{}",
                message
            ),
            DurableLogLevel::Error => error!(
                durable_execution_arn = %data.durable_execution_arn,
                step_name = data.step_name.as_deref(),
                operation_id = data.operation_id.as_deref(),
                attempt = data.attempt,
                extra = ?fields,
                "{}",
                message
            ),
        }
    }
}
