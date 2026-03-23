use super::super::error::LlmError;
use super::super::models::{LLMInvocation, TransformedRequest, TransformedResponse};
use super::MessageTransformer;
use serde_json::Value;

/// Transformer for the OpenAI Chat Completions API format.
///
/// Also used for OpenAI-compatible providers (XAI, DeepSeek).
pub struct OpenAITransformer;

impl MessageTransformer for OpenAITransformer {
    fn transform_request(
        &self,
        _invocation: &LLMInvocation,
    ) -> Result<TransformedRequest, LlmError> {
        // Full implementation in Task 2
        Err(LlmError::TransformError("Not yet implemented".to_string()))
    }

    fn transform_response(&self, _response: Value) -> Result<TransformedResponse, LlmError> {
        // Full implementation in Task 2
        Err(LlmError::TransformError("Not yet implemented".to_string()))
    }
}
