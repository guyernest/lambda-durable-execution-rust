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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct TestLogger {
        levels: Mutex<Vec<DurableLogLevel>>,
    }

    impl TestLogger {
        fn new() -> Self {
            Self {
                levels: Mutex::new(Vec::new()),
            }
        }

        fn levels(&self) -> Vec<DurableLogLevel> {
            self.levels.lock().expect("levels mutex").clone()
        }
    }

    impl DurableLogger for TestLogger {
        fn log(
            &self,
            level: DurableLogLevel,
            _data: &DurableLogData,
            _message: &str,
            _fields: Option<&[(&'static str, String)]>,
        ) {
            self.levels.lock().expect("levels mutex").push(level);
        }
    }

    #[test]
    fn test_logger_helpers_forward_levels() {
        let logger = TestLogger::new();
        let data = DurableLogData {
            durable_execution_arn: "arn:aws:lambda:us-east-1:123:function:durable".to_string(),
            operation_id: Some("op-1".to_string()),
            step_name: Some("step".to_string()),
            attempt: Some(1),
        };

        logger.debug(&data, "debug");
        logger.info(&data, "info");
        logger.warn(&data, "warn");
        logger.error(&data, "error");

        assert_eq!(
            logger.levels(),
            vec![
                DurableLogLevel::Debug,
                DurableLogLevel::Info,
                DurableLogLevel::Warn,
                DurableLogLevel::Error
            ]
        );
    }

    #[test]
    fn test_tracing_logger_log_levels() {
        let logger = TracingLogger;
        let data = DurableLogData {
            durable_execution_arn: "arn:aws:lambda:us-east-1:123:function:durable".to_string(),
            operation_id: Some("op-1".to_string()),
            step_name: Some("step".to_string()),
            attempt: Some(2),
        };

        logger.log(DurableLogLevel::Debug, &data, "debug", None);
        logger.log(
            DurableLogLevel::Info,
            &data,
            "info",
            Some(&[("key", "value".to_string())]),
        );
        logger.log(DurableLogLevel::Warn, &data, "warn", None);
        logger.log(DurableLogLevel::Error, &data, "error", None);
    }
}
