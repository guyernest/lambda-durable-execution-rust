//! Configuration types for durable operations.

use crate::retry::RetryStrategy;
use crate::types::{BatchResult, Duration, LambdaService, RealLambdaService, Serdes};
use std::marker::PhantomData;
use std::sync::Arc;

type ItemNamer<TIn> = dyn Fn(&TIn, usize) -> String + Send + Sync;
type WaitStrategy<T> = dyn Fn(&T, u32) -> WaitConditionDecision + Send + Sync;

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

/// Configuration for invoke operations.
///
/// Mirrors the JS `InvokeConfig`:
/// - `payload_serdes` is used to serialize the input payload.
/// - `result_serdes` is used to deserialize the invoke result.
/// - `tenant_id` is optional metadata passed to the service.
pub struct InvokeConfig<I, O> {
    /// Optional Serdes for input payload.
    pub payload_serdes: Option<Arc<dyn Serdes<I>>>,
    /// Optional Serdes for result payload.
    pub result_serdes: Option<Arc<dyn Serdes<O>>>,
    /// Optional tenant identifier.
    pub tenant_id: Option<String>,
    /// Phantom data for generic parameters.
    pub(crate) _phantom: PhantomData<(I, O)>,
}

impl<I, O> Default for InvokeConfig<I, O> {
    fn default() -> Self {
        Self {
            payload_serdes: None,
            result_serdes: None,
            tenant_id: None,
            _phantom: PhantomData,
        }
    }
}

impl<I, O> InvokeConfig<I, O> {
    /// Create a new default invoke configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set Serdes for the input payload.
    pub fn with_payload_serdes(mut self, serdes: Arc<dyn Serdes<I>>) -> Self {
        self.payload_serdes = Some(serdes);
        self
    }

    /// Set Serdes for the result payload.
    pub fn with_result_serdes(mut self, serdes: Arc<dyn Serdes<O>>) -> Self {
        self.result_serdes = Some(serdes);
        self
    }

    /// Set the tenant id to pass to the chained invoke.
    pub fn with_tenant_id(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = Some(tenant_id.into());
        self
    }
}

impl<I, O> Clone for InvokeConfig<I, O> {
    fn clone(&self) -> Self {
        Self {
            payload_serdes: self.payload_serdes.clone(),
            result_serdes: self.result_serdes.clone(),
            tenant_id: self.tenant_id.clone(),
            _phantom: PhantomData,
        }
    }
}

impl<I, O> std::fmt::Debug for InvokeConfig<I, O> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InvokeConfig")
            .field("payload_serdes", &self.payload_serdes.is_some())
            .field("result_serdes", &self.result_serdes.is_some())
            .field("tenant_id", &self.tenant_id)
            .finish()
    }
}

/// Configuration for callback operations.
pub struct CallbackConfig<T> {
    /// Timeout for waiting on the callback.
    pub timeout: Option<Duration>,

    /// Heartbeat timeout (callback must send heartbeats more frequently than this).
    pub heartbeat_timeout: Option<Duration>,

    /// Optional retry strategy for the callback submitter step.
    ///
    /// This mirrors the JS `waitForCallback` retryStrategy which is applied
    /// to the submitter step, not the callback wait itself.
    pub retry_strategy: Option<Arc<dyn RetryStrategy>>,

    /// Optional Serdes for callback result payloads (deserialize only).
    pub serdes: Option<Arc<dyn Serdes<T>>>,

    /// Phantom data for the output type.
    pub(crate) _phantom: PhantomData<T>,
}

impl<T> Clone for CallbackConfig<T> {
    fn clone(&self) -> Self {
        Self {
            timeout: self.timeout,
            heartbeat_timeout: self.heartbeat_timeout,
            retry_strategy: self.retry_strategy.clone(),
            serdes: self.serdes.clone(),
            _phantom: PhantomData,
        }
    }
}

impl<T> Default for CallbackConfig<T> {
    fn default() -> Self {
        Self {
            timeout: None,
            heartbeat_timeout: None,
            retry_strategy: None,
            serdes: None,
            _phantom: PhantomData,
        }
    }
}

impl<T> CallbackConfig<T> {
    /// Create a new default callback configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Set the heartbeat timeout.
    pub fn with_heartbeat_timeout(mut self, timeout: Duration) -> Self {
        self.heartbeat_timeout = Some(timeout);
        self
    }

    /// Set the retry strategy for the submitter step.
    pub fn with_retry_strategy(mut self, strategy: Arc<dyn RetryStrategy>) -> Self {
        self.retry_strategy = Some(strategy);
        self
    }

    /// Set custom Serdes for this callback.
    pub fn with_serdes(mut self, serdes: Arc<dyn Serdes<T>>) -> Self {
        self.serdes = Some(serdes);
        self
    }
}

impl<T> std::fmt::Debug for CallbackConfig<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallbackConfig")
            .field("timeout", &self.timeout)
            .field("heartbeat_timeout", &self.heartbeat_timeout)
            .field("retry_strategy", &self.retry_strategy.is_some())
            .field("serdes", &self.serdes.is_some())
            .finish()
    }
}

/// Configuration for child context operations.
#[derive(Clone)]
pub struct ChildContextConfig<T> {
    /// Subtype identifier for the child context.
    pub sub_type: Option<String>,

    /// Optional Serdes for child context result payloads.
    pub serdes: Option<Arc<dyn Serdes<T>>>,

    /// Phantom data for the output type.
    pub(crate) _phantom: PhantomData<T>,
}

impl<T> Default for ChildContextConfig<T> {
    fn default() -> Self {
        Self {
            sub_type: None,
            serdes: None,
            _phantom: PhantomData,
        }
    }
}

impl<T> ChildContextConfig<T> {
    /// Create a new default child context configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the subtype.
    pub fn with_sub_type(mut self, sub_type: impl Into<String>) -> Self {
        self.sub_type = Some(sub_type.into());
        self
    }

    /// Set custom Serdes for this child context.
    pub fn with_serdes(mut self, serdes: Arc<dyn Serdes<T>>) -> Self {
        self.serdes = Some(serdes);
        self
    }
}

impl<T> std::fmt::Debug for ChildContextConfig<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChildContextConfig")
            .field("sub_type", &self.sub_type)
            .field("serdes", &self.serdes.is_some())
            .finish()
    }
}

/// Completion requirements for batch operations.
#[derive(Debug, Clone, Default)]
pub struct CompletionConfig {
    /// Minimum number of successful operations required.
    /// If set, the batch will complete early once this many succeed.
    pub min_successful: Option<usize>,

    /// Maximum number of tolerated failures before the batch fails.
    pub tolerated_failure_count: Option<usize>,

    /// Maximum percentage of tolerated failures (0-100) before the batch fails.
    pub tolerated_failure_percentage: Option<f64>,
}

impl CompletionConfig {
    /// Create a new default completion configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the minimum number of successful operations.
    pub fn with_min_successful(mut self, count: usize) -> Self {
        self.min_successful = Some(count);
        self
    }

    /// Set the tolerated failure count.
    pub fn with_tolerated_failures(mut self, count: usize) -> Self {
        self.tolerated_failure_count = Some(count);
        self
    }

    /// Set the tolerated failure percentage (0-100).
    pub fn with_tolerated_failure_percentage(mut self, percentage: f64) -> Self {
        self.tolerated_failure_percentage = Some(percentage);
        self
    }
}

/// Configuration for parallel operations.
#[derive(Clone)]
pub struct ParallelConfig<T> {
    /// Maximum number of concurrent operations.
    pub max_concurrency: Option<usize>,

    /// Optional Serdes for the entire batch result (`BatchResult<T>`).
    ///
    /// When provided, the SDK will serialize the final `BatchResult` using this
    /// Serdes for checkpointing and replay.
    pub serdes: Option<Arc<dyn Serdes<BatchResult<T>>>>,

    /// Optional Serdes for each branch result.
    pub item_serdes: Option<Arc<dyn Serdes<T>>>,

    /// Completion requirements.
    pub completion_config: CompletionConfig,

    /// Phantom data for the output type.
    pub(crate) _phantom: PhantomData<T>,
}

/// A named branch for parallel execution.
///
/// Mirrors JS `NamedParallelBranch<TResult>`.
pub struct NamedParallelBranch<F> {
    /// Optional customer-provided branch name.
    pub name: Option<String>,
    /// Branch function.
    pub func: F,
}

impl<F> NamedParallelBranch<F> {
    /// Create an unnamed branch.
    pub fn new(func: F) -> Self {
        Self { name: None, func }
    }

    /// Set a name for this branch.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
}

impl<T> Default for ParallelConfig<T> {
    fn default() -> Self {
        Self {
            max_concurrency: None,
            serdes: None,
            item_serdes: None,
            completion_config: CompletionConfig::default(),
            _phantom: PhantomData,
        }
    }
}

impl<T> ParallelConfig<T> {
    /// Create a new default parallel configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum concurrency.
    pub fn with_max_concurrency(mut self, max: usize) -> Self {
        self.max_concurrency = Some(max);
        self
    }

    /// Set the completion configuration.
    pub fn with_completion_config(mut self, config: CompletionConfig) -> Self {
        self.completion_config = config;
        self
    }

    /// Set Serdes for the entire batch result (`BatchResult<T>`).
    pub fn with_serdes(mut self, serdes: Arc<dyn Serdes<BatchResult<T>>>) -> Self {
        self.serdes = Some(serdes);
        self
    }

    /// Set Serdes for each branch result.
    pub fn with_item_serdes(mut self, serdes: Arc<dyn Serdes<T>>) -> Self {
        self.item_serdes = Some(serdes);
        self
    }
}

impl<T> std::fmt::Debug for ParallelConfig<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParallelConfig")
            .field("max_concurrency", &self.max_concurrency)
            .field("serdes", &self.serdes.is_some())
            .field("item_serdes", &self.item_serdes.is_some())
            .field("completion_config", &self.completion_config)
            .finish()
    }
}

/// Configuration for map operations.
///
/// Mirrors JS `MapConfig<TItem, TResult>`, including optional per-item naming.
pub struct MapConfig<TIn, TOut> {
    /// Maximum number of concurrent operations.
    pub max_concurrency: Option<usize>,

    /// Optional function to generate custom names for map items.
    pub item_namer: Option<Arc<ItemNamer<TIn>>>,

    /// Optional Serdes for the entire batch result (`BatchResult<TOut>`).
    ///
    /// When provided, the SDK will serialize the final `BatchResult` using this
    /// Serdes for checkpointing and replay.
    pub serdes: Option<Arc<dyn Serdes<BatchResult<TOut>>>>,

    /// Optional Serdes for each mapped item result.
    pub item_serdes: Option<Arc<dyn Serdes<TOut>>>,

    /// Completion requirements.
    pub completion_config: CompletionConfig,

    /// Phantom data for the input/output types.
    pub(crate) _phantom: PhantomData<(TIn, TOut)>,
}

impl<TIn, TOut> Default for MapConfig<TIn, TOut> {
    fn default() -> Self {
        Self {
            max_concurrency: None,
            item_namer: None,
            serdes: None,
            item_serdes: None,
            completion_config: CompletionConfig::default(),
            _phantom: PhantomData,
        }
    }
}

impl<TIn, TOut> MapConfig<TIn, TOut> {
    /// Create a new default map configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum concurrency.
    pub fn with_max_concurrency(mut self, max: usize) -> Self {
        self.max_concurrency = Some(max);
        self
    }

    /// Set a custom item namer.
    pub fn with_item_namer(mut self, namer: Arc<ItemNamer<TIn>>) -> Self {
        self.item_namer = Some(namer);
        self
    }

    /// Set Serdes for the entire batch result (`BatchResult<TOut>`).
    pub fn with_serdes(mut self, serdes: Arc<dyn Serdes<BatchResult<TOut>>>) -> Self {
        self.serdes = Some(serdes);
        self
    }

    /// Set the completion configuration.
    pub fn with_completion_config(mut self, config: CompletionConfig) -> Self {
        self.completion_config = config;
        self
    }

    /// Set Serdes for each mapped item result.
    pub fn with_item_serdes(mut self, serdes: Arc<dyn Serdes<TOut>>) -> Self {
        self.item_serdes = Some(serdes);
        self
    }
}

impl<TIn, TOut> Clone for MapConfig<TIn, TOut> {
    fn clone(&self) -> Self {
        Self {
            max_concurrency: self.max_concurrency,
            item_namer: self.item_namer.clone(),
            serdes: self.serdes.clone(),
            item_serdes: self.item_serdes.clone(),
            completion_config: self.completion_config.clone(),
            _phantom: PhantomData,
        }
    }
}

impl<TIn, TOut> std::fmt::Debug for MapConfig<TIn, TOut> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapConfig")
            .field("max_concurrency", &self.max_concurrency)
            .field("item_namer", &self.item_namer.is_some())
            .field("serdes", &self.serdes.is_some())
            .field("item_serdes", &self.item_serdes.is_some())
            .field("completion_config", &self.completion_config)
            .finish()
    }
}

/// Decision returned by a wait-for-condition strategy.
#[derive(Debug, Clone)]
pub enum WaitConditionDecision {
    /// Continue waiting and retry after the given delay.
    Continue {
        /// Delay before the next attempt.
        delay: Duration,
    },
    /// Stop waiting and succeed with the current state.
    Stop,
}

/// Configuration for wait-for-condition operations.
#[derive(Clone)]
pub struct WaitConditionConfig<T> {
    /// Initial state for the condition check.
    pub initial_state: T,

    /// Strategy that decides whether to continue and how long to wait.
    pub wait_strategy: Arc<WaitStrategy<T>>,

    /// Optional Serdes for state payloads.
    pub serdes: Option<Arc<dyn Serdes<T>>>,

    /// Optional maximum number of attempts before failing.
    pub max_attempts: Option<u32>,
}

impl<T> WaitConditionConfig<T> {
    /// Create a new wait condition configuration.
    pub fn new(initial_state: T, wait_strategy: Arc<WaitStrategy<T>>) -> Self {
        Self {
            initial_state,
            wait_strategy,
            serdes: None,
            max_attempts: None,
        }
    }

    /// Set the maximum number of attempts.
    pub fn with_max_attempts(mut self, max: u32) -> Self {
        self.max_attempts = Some(max);
        self
    }

    /// Set custom Serdes for state payloads.
    pub fn with_serdes(mut self, serdes: Arc<dyn Serdes<T>>) -> Self {
        self.serdes = Some(serdes);
        self
    }
}

impl<T> std::fmt::Debug for WaitConditionConfig<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaitConditionConfig")
            .field("max_attempts", &self.max_attempts)
            .finish_non_exhaustive()
    }
}

/// Configuration for durable execution.
#[derive(Debug, Clone, Default)]
pub struct DurableExecutionConfig {
    /// Custom AWS Lambda service to use.
    pub lambda_service: Option<Arc<dyn LambdaService>>,
}

impl DurableExecutionConfig {
    /// Create a new default configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a custom Lambda client.
    pub fn with_lambda_client(mut self, client: aws_sdk_lambda::Client) -> Self {
        self.lambda_service = Some(Arc::new(RealLambdaService::new(Arc::new(client))));
        self
    }

    /// Set a custom Lambda service.
    pub fn with_lambda_service(mut self, service: Arc<dyn LambdaService>) -> Self {
        self.lambda_service = Some(service);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_step_config_builder() {
        let config: StepConfig<String> =
            StepConfig::new().with_semantics(StepSemantics::AtLeastOncePerRetry);

        assert_eq!(config.semantics, StepSemantics::AtLeastOncePerRetry);
        assert!(config.retry_strategy.is_none());
    }

    #[test]
    fn test_callback_config_builder() {
        let config: CallbackConfig<String> = CallbackConfig::new()
            .with_timeout(Duration::minutes(5))
            .with_heartbeat_timeout(Duration::seconds(30));

        assert_eq!(config.timeout.unwrap().to_seconds(), 300);
        assert_eq!(config.heartbeat_timeout.unwrap().to_seconds(), 30);
    }

    #[test]
    fn test_parallel_config_builder() {
        let config: ParallelConfig<String> = ParallelConfig::new()
            .with_max_concurrency(5)
            .with_completion_config(
                CompletionConfig::new()
                    .with_min_successful(3)
                    .with_tolerated_failures(2),
            );

        assert_eq!(config.max_concurrency, Some(5));
        assert_eq!(config.completion_config.min_successful, Some(3));
        assert_eq!(config.completion_config.tolerated_failure_count, Some(2));
    }
}
