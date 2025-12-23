//! Limited context available inside step functions.

use crate::context::ExecutionMode;
use crate::types::{DurableLogData, DurableLogLevel, DurableLogger, TracingLogger};
use std::sync::Arc;

/// A limited context available inside step functions.
///
/// The StepContext provides logging capabilities but does not allow
/// calling other durable operations. To group multiple durable operations,
/// use `run_in_child_context` instead.
///
/// # Example
///
/// ```rust,no_run
/// # use lambda_durable_execution_rust::prelude::*;
/// async fn do_something() -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
///     Ok("ok".to_string())
/// }
///
/// async fn example(ctx: DurableContextHandle) -> DurableResult<String> {
///     ctx.step(
///         Some("my-step"),
///         |step_ctx| async move {
///             step_ctx.info("Starting operation");
///             let result = do_something().await?;
///             step_ctx.info("Operation completed");
///             Ok(result)
///         },
///         None,
///     )
///     .await
/// }
/// ```
#[derive(Clone)]
pub struct StepContext {
    /// Name of the current step.
    step_name: Option<String>,
    /// Operation ID of the current step.
    operation_id: String,
    /// ARN of the durable execution.
    durable_execution_arn: String,
    /// Logger for durable operations.
    logger: Arc<dyn DurableLogger>,
    /// Whether to suppress logs during replay.
    mode_aware_logging: bool,
    /// Current execution mode.
    mode: ExecutionMode,
    /// Attempt number, if known.
    attempt: Option<u32>,
}

impl std::fmt::Debug for StepContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StepContext")
            .field("step_name", &self.step_name)
            .field("operation_id", &self.operation_id)
            .field("durable_execution_arn", &self.durable_execution_arn)
            .field("mode_aware_logging", &self.mode_aware_logging)
            .field("mode", &self.mode)
            .field("attempt", &self.attempt)
            .finish()
    }
}

impl StepContext {
    /// Create a new step context.
    ///
    /// This constructor is primarily for tests; durable operations will use
    /// the internal constructor that carries logger configuration.
    pub fn new(step_name: Option<String>, operation_id: String) -> Self {
        Self::new_with_logger(
            step_name,
            operation_id,
            "unknown".to_string(),
            Arc::new(TracingLogger),
            ExecutionMode::Execution,
            false,
            None,
        )
    }

    pub(crate) fn new_with_logger(
        step_name: Option<String>,
        operation_id: String,
        durable_execution_arn: String,
        logger: Arc<dyn DurableLogger>,
        mode: ExecutionMode,
        mode_aware_logging: bool,
        attempt: Option<u32>,
    ) -> Self {
        Self {
            step_name,
            operation_id,
            durable_execution_arn,
            logger,
            mode_aware_logging,
            mode,
            attempt,
        }
    }

    fn log_data(&self) -> DurableLogData {
        DurableLogData {
            durable_execution_arn: self.durable_execution_arn.clone(),
            operation_id: Some(self.operation_id.clone()),
            step_name: self.step_name.clone(),
            attempt: self.attempt,
        }
    }

    /// Get the name of the current step.
    pub fn step_name(&self) -> Option<&str> {
        self.step_name.as_deref()
    }

    /// Get the operation ID of the current step.
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    /// Get the current attempt number for this step, if available.
    ///
    /// Attempt numbers are 1-based and only set when the step implementation
    /// has retry semantics enabled.
    pub fn attempt(&self) -> Option<u32> {
        self.attempt
    }

    /// Log a debug message.
    pub fn debug(&self, message: &str) {
        if self.mode_aware_logging && self.mode == ExecutionMode::Replay {
            return;
        }
        self.logger
            .log(DurableLogLevel::Debug, &self.log_data(), message, None);
    }

    /// Log an info message.
    pub fn info(&self, message: &str) {
        if self.mode_aware_logging && self.mode == ExecutionMode::Replay {
            return;
        }
        self.logger
            .log(DurableLogLevel::Info, &self.log_data(), message, None);
    }

    /// Log a warning message.
    pub fn warn(&self, message: &str) {
        if self.mode_aware_logging && self.mode == ExecutionMode::Replay {
            return;
        }
        self.logger
            .log(DurableLogLevel::Warn, &self.log_data(), message, None);
    }

    /// Log an error message.
    pub fn error(&self, message: &str) {
        if self.mode_aware_logging && self.mode == ExecutionMode::Replay {
            return;
        }
        self.logger
            .log(DurableLogLevel::Error, &self.log_data(), message, None);
    }

    /// Log a debug message with additional fields.
    pub fn debug_with<F>(&self, message: &str, fields: F)
    where
        F: FnOnce() -> Vec<(&'static str, String)>,
    {
        if self.mode_aware_logging && self.mode == ExecutionMode::Replay {
            return;
        }
        let extra = fields();
        self.logger.log(
            DurableLogLevel::Debug,
            &self.log_data(),
            message,
            Some(&extra),
        );
    }

    /// Log an info message with additional fields.
    pub fn info_with<F>(&self, message: &str, fields: F)
    where
        F: FnOnce() -> Vec<(&'static str, String)>,
    {
        if self.mode_aware_logging && self.mode == ExecutionMode::Replay {
            return;
        }
        let extra = fields();
        self.logger.log(
            DurableLogLevel::Info,
            &self.log_data(),
            message,
            Some(&extra),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_step_context_creation() {
        let ctx = StepContext::new(Some("my-step".to_string()), "op-123".to_string());
        assert_eq!(ctx.step_name(), Some("my-step"));
        assert_eq!(ctx.operation_id(), "op-123");
    }

    #[test]
    fn test_step_context_without_name() {
        let ctx = StepContext::new(None, "op-456".to_string());
        assert_eq!(ctx.step_name(), None);
    }

    struct CountingLogger {
        count: Arc<Mutex<usize>>,
    }

    impl DurableLogger for CountingLogger {
        fn log(
            &self,
            _level: DurableLogLevel,
            _data: &DurableLogData,
            _message: &str,
            _fields: Option<&[(&'static str, String)]>,
        ) {
            *self.count.lock().unwrap() += 1;
        }
    }

    #[test]
    fn test_mode_aware_suppresses_replay_logs() {
        let count = Arc::new(Mutex::new(0));
        let logger = Arc::new(CountingLogger {
            count: Arc::clone(&count),
        });

        let ctx = StepContext::new_with_logger(
            Some("step".to_string()),
            "op-1".to_string(),
            "arn:test".to_string(),
            logger,
            ExecutionMode::Replay,
            true,
            None,
        );

        ctx.info("hello");
        assert_eq!(*count.lock().unwrap(), 0);
    }

    #[test]
    fn test_mode_aware_allows_execution_logs() {
        let count = Arc::new(Mutex::new(0));
        let logger = Arc::new(CountingLogger {
            count: Arc::clone(&count),
        });

        let ctx = StepContext::new_with_logger(
            Some("step".to_string()),
            "op-1".to_string(),
            "arn:test".to_string(),
            logger,
            ExecutionMode::Execution,
            true,
            None,
        );

        ctx.info("hello");
        assert_eq!(*count.lock().unwrap(), 1);
    }

    #[derive(Debug, Clone)]
    struct LogEntry {
        level: DurableLogLevel,
        data: DurableLogData,
        message: String,
        fields: Option<Vec<(String, String)>>,
    }

    #[derive(Default)]
    struct RecordingLogger {
        entries: Arc<Mutex<Vec<LogEntry>>>,
    }

    impl DurableLogger for RecordingLogger {
        fn log(
            &self,
            level: DurableLogLevel,
            data: &DurableLogData,
            message: &str,
            fields: Option<&[(&'static str, String)]>,
        ) {
            let fields = fields.map(|items| {
                items
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), value.clone()))
                    .collect()
            });
            self.entries.lock().unwrap().push(LogEntry {
                level,
                data: data.clone(),
                message: message.to_string(),
                fields,
            });
        }
    }

    #[test]
    fn test_log_methods_capture_metadata() {
        let entries = Arc::new(Mutex::new(Vec::new()));
        let logger = Arc::new(RecordingLogger {
            entries: Arc::clone(&entries),
        });

        let ctx = StepContext::new_with_logger(
            Some("step".to_string()),
            "op-1".to_string(),
            "arn:test".to_string(),
            logger,
            ExecutionMode::Execution,
            false,
            Some(2),
        );

        ctx.debug("debug");
        ctx.warn("warn");
        ctx.error("error");
        ctx.debug_with("debug-fields", || vec![("k", "v".to_string())]);
        ctx.info_with("info-fields", || vec![("a", "b".to_string())]);

        let logs = entries.lock().unwrap();
        assert_eq!(logs.len(), 5);
        assert_eq!(logs[0].level, DurableLogLevel::Debug);
        assert_eq!(logs[0].data.operation_id.as_deref(), Some("op-1"));
        assert_eq!(logs[0].data.step_name.as_deref(), Some("step"));
        assert_eq!(logs[0].data.attempt, Some(2));
        assert_eq!(logs[3].fields.as_ref().unwrap()[0].0, "k");
        assert_eq!(logs[3].fields.as_ref().unwrap()[0].1, "v");
        assert_eq!(logs[4].message, "info-fields");
    }
}
