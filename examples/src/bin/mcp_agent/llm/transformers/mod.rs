use super::error::LlmError;
use super::models::{LLMInvocation, TransformedRequest, TransformedResponse};
use serde_json::Value;
use std::collections::HashMap;

pub mod anthropic;
pub mod openai;
pub mod utils;

pub use anthropic::AnthropicTransformer;
pub use openai::OpenAITransformer;

/// Synchronous transformer trait for converting between unified and provider-specific formats.
///
/// Each provider (Anthropic, OpenAI) implements this trait to handle its wire format.
/// Methods are synchronous because they perform only JSON transformation, no I/O.
pub trait MessageTransformer: Send + Sync {
    /// Transform a unified LLM invocation to a provider-specific request body and headers.
    fn transform_request(
        &self,
        invocation: &LLMInvocation,
    ) -> Result<TransformedRequest, LlmError>;

    /// Transform a provider-specific JSON response to the unified response format.
    fn transform_response(&self, response: Value) -> Result<TransformedResponse, LlmError>;

    /// Get provider-specific headers (e.g. anthropic-version).
    fn get_headers(&self) -> HashMap<String, String> {
        HashMap::new()
    }
}

/// Registry mapping transformer IDs to their implementations.
///
/// Transformer IDs match the `request_transformer` / `response_transformer`
/// fields in [`ProviderConfig`](super::models::ProviderConfig).
pub struct TransformerRegistry {
    transformers: HashMap<String, Box<dyn MessageTransformer>>,
}

impl TransformerRegistry {
    /// Create a new registry with all built-in transformers registered.
    ///
    /// Registered transformers:
    /// - `anthropic_v1` — Anthropic Messages API
    /// - `openai_v1` — OpenAI Chat Completions API (also used for XAI, DeepSeek)
    pub fn new() -> Self {
        let mut transformers: HashMap<String, Box<dyn MessageTransformer>> = HashMap::new();

        // Register Anthropic transformer
        transformers.insert(
            "anthropic_v1".to_string(),
            Box::new(AnthropicTransformer),
        );

        // Register OpenAI transformer (also used for XAI, DeepSeek)
        transformers.insert(
            "openai_v1".to_string(),
            Box::new(OpenAITransformer),
        );

        Self { transformers }
    }

    /// Look up a transformer by ID, returning `TransformerNotFound` if unknown.
    pub fn get(&self, name: &str) -> Result<&dyn MessageTransformer, LlmError> {
        self.transformers
            .get(name)
            .map(|t| t.as_ref())
            .ok_or_else(|| LlmError::TransformerNotFound(name.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_get_anthropic_v1() {
        let registry = TransformerRegistry::new();
        assert!(registry.get("anthropic_v1").is_ok());
    }

    #[test]
    fn test_registry_get_openai_v1() {
        let registry = TransformerRegistry::new();
        assert!(registry.get("openai_v1").is_ok());
    }

    #[test]
    fn test_registry_get_nonexistent_returns_error() {
        let registry = TransformerRegistry::new();
        let result = registry.get("nonexistent");
        assert!(result.is_err());
        match result {
            Err(LlmError::TransformerNotFound(name)) => assert_eq!(name, "nonexistent"),
            Err(other) => panic!("Expected TransformerNotFound, got: {other:?}"),
            Ok(_) => panic!("Expected error, got Ok"),
        }
    }
}
