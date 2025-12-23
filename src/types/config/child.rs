use crate::types::Serdes;
use std::marker::PhantomData;
use std::sync::Arc;

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
