use crate::retry::RetryStrategy;
use crate::types::Serdes;
use std::marker::PhantomData;
use std::sync::Arc;

/// Execution semantics for step operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StepSemantics {
    /// Execute at least once per retry cycle.
    /// The step may execute multiple times if Lambda is terminated during execution.
    #[default]
    AtLeastOncePerRetry,

    /// Execute at most once per retry cycle.
    /// The step is checkpointed before execution, preventing duplicate execution.
    AtMostOncePerRetry,
}

/// Configuration for step operations.
pub struct StepConfig<T> {
    /// Retry strategy for handling failures.
    pub retry_strategy: Option<Arc<dyn RetryStrategy>>,

    /// Execution semantics (at-most-once vs at-least-once).
    pub semantics: StepSemantics,

    /// Optional Serdes for step result payloads.
    pub serdes: Option<Arc<dyn Serdes<T>>>,

    /// Phantom data for the output type.
    pub(crate) _phantom: PhantomData<T>,
}

impl<T> Default for StepConfig<T> {
    fn default() -> Self {
        Self {
            retry_strategy: None,
            semantics: StepSemantics::default(),
            serdes: None,
            _phantom: PhantomData,
        }
    }
}

impl<T> StepConfig<T> {
    /// Create a new default step configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the retry strategy.
    pub fn with_retry_strategy(mut self, strategy: Arc<dyn RetryStrategy>) -> Self {
        self.retry_strategy = Some(strategy);
        self
    }

    /// Set the execution semantics.
    pub fn with_semantics(mut self, semantics: StepSemantics) -> Self {
        self.semantics = semantics;
        self
    }

    /// Set custom Serdes for this step.
    pub fn with_serdes(mut self, serdes: Arc<dyn Serdes<T>>) -> Self {
        self.serdes = Some(serdes);
        self
    }
}

impl<T> std::fmt::Debug for StepConfig<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StepConfig")
            .field("retry_strategy", &self.retry_strategy.is_some())
            .field("semantics", &self.semantics)
            .field("serdes", &self.serdes.is_some())
            .finish()
    }
}
