use crate::retry::RetryStrategy;
use crate::types::{Duration, Serdes};
use std::marker::PhantomData;
use std::sync::Arc;

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
