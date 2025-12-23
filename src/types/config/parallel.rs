use crate::types::{BatchResult, Serdes};
use std::marker::PhantomData;
use std::sync::Arc;

use super::CompletionConfig;

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
