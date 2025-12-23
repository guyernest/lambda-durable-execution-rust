use crate::types::{Duration, Serdes};
use std::sync::Arc;

use super::WaitStrategy;

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
