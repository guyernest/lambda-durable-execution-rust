use crate::types::{BatchResult, Serdes};
use std::marker::PhantomData;
use std::sync::Arc;

use super::{CompletionConfig, ItemNamer};

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
