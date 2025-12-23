use crate::types::Serdes;
use std::marker::PhantomData;
use std::sync::Arc;

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
