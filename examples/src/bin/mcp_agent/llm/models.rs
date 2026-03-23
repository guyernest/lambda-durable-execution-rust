use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

// ===== Request Models =====

/// A provider-agnostic LLM invocation request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMInvocation {
    /// Provider configuration (endpoint, auth, transformer IDs).
    pub provider_config: ProviderConfig,
    /// Conversation messages in unified format.
    pub messages: Vec<UnifiedMessage>,
    /// Tool definitions available to the model.
    #[serde(default)]
    pub tools: Option<Vec<UnifiedTool>>,
    /// Sampling temperature (0.0 - 1.0).
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Maximum tokens to generate.
    #[serde(default)]
    pub max_tokens: Option<i32>,
    /// Top-p / nucleus sampling parameter.
    #[serde(default)]
    pub top_p: Option<f32>,
    /// Whether to stream the response (not used in PoC).
    #[serde(default)]
    pub stream: Option<bool>,
}

/// Provider-specific configuration matching the AgentRegistry schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Provider identifier (e.g. "anthropic", "openai").
    pub provider_id: String,
    /// Model identifier (e.g. "claude-sonnet-4-20250514").
    pub model_id: String,
    /// API endpoint URL.
    pub endpoint: String,
    /// Name of the HTTP header for authentication.
    pub auth_header_name: String,
    /// Optional prefix for the auth header value (e.g. "Bearer").
    #[serde(default)]
    pub auth_header_prefix: Option<String>,
    /// AWS Secrets Manager secret path for the API key.
    pub secret_path: String,
    /// Key name within the secret JSON.
    pub secret_key_name: String,
    /// Request transformer identifier (e.g. "anthropic_v1").
    pub request_transformer: String,
    /// Response transformer identifier (e.g. "anthropic_v1").
    pub response_transformer: String,
    /// Request timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    /// Optional additional HTTP headers.
    #[serde(default)]
    pub custom_headers: Option<HashMap<String, String>>,
}

fn default_timeout() -> u64 {
    30
}

/// A message in the unified conversation format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedMessage {
    /// Role: "user", "assistant", or "system".
    pub role: String,
    /// Message content (text or structured blocks).
    #[serde(flatten)]
    pub content: MessageContent,
}

/// Message content: either plain text or structured content blocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// Simple text content.
    Text {
        /// The text content.
        content: String,
    },
    /// Structured content blocks (tool_use, tool_result, etc.).
    Blocks {
        /// The content blocks.
        content: Vec<ContentBlock>,
    },
}

/// A content block within a message (tagged by type).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    /// Text content block.
    #[serde(rename = "text")]
    Text {
        /// The text content.
        text: String,
    },

    /// Tool use request from the model.
    #[serde(rename = "tool_use")]
    ToolUse {
        /// Unique tool use ID.
        id: String,
        /// Tool name.
        name: String,
        /// Tool input parameters.
        input: Value,
    },

    /// Tool result returned to the model.
    #[serde(rename = "tool_result")]
    ToolResult {
        /// ID of the tool_use this result corresponds to.
        tool_use_id: String,
        /// The result content.
        content: String,
        /// Whether this result represents an error (for MCP error propagation).
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },

    /// Image content block.
    #[serde(rename = "image")]
    Image {
        /// Image source data.
        source: ImageSource,
    },
}

/// Image source for inline image content blocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageSource {
    /// Source type (e.g. "base64").
    #[serde(rename = "type")]
    pub source_type: String,
    /// MIME type (e.g. "image/png").
    pub media_type: String,
    /// Base64-encoded image data.
    pub data: String,
}

/// A tool definition in the unified format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedTool {
    /// Tool name.
    pub name: String,
    /// Tool description.
    pub description: String,
    /// JSON Schema for the tool's input parameters.
    pub input_schema: Value,
}

// ===== Response Models =====

/// The unified LLM response containing the assistant message, extracted
/// function calls, and provider metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMResponse {
    /// The assistant's response message.
    pub message: AssistantMessage,
    /// Extracted function/tool calls (unified across providers).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_calls: Option<Vec<FunctionCall>>,
    /// Response metadata (model, latency, tokens, stop reason).
    pub metadata: ResponseMetadata,
}

/// The assistant's message with content blocks and optional raw tool_calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    /// Always "assistant".
    pub role: String,
    /// Content blocks in the response.
    pub content: Vec<ContentBlock>,
    /// Provider-specific raw tool_calls (kept for debugging).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Value>,
}

/// A unified function call extracted from the model response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    /// Unique call ID.
    pub id: String,
    /// Function/tool name.
    pub name: String,
    /// Function input as JSON.
    pub input: Value,
}

/// Metadata about the LLM response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseMetadata {
    /// Model that generated the response.
    pub model_id: String,
    /// Provider that served the request.
    pub provider_id: String,
    /// Round-trip latency in milliseconds.
    pub latency_ms: u64,
    /// Token usage statistics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_used: Option<TokenUsage>,
    /// Why the model stopped generating.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
}

/// Token usage breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Tokens in the prompt/input.
    pub input_tokens: u32,
    /// Tokens in the completion/output.
    pub output_tokens: u32,
    /// Total tokens used.
    pub total_tokens: u32,
}

// ===== Internal Models =====

/// A transformed HTTP request ready to send to a provider.
#[derive(Debug, Clone)]
pub struct TransformedRequest {
    /// The JSON request body.
    pub body: Value,
    /// Additional HTTP headers.
    pub headers: HashMap<String, String>,
}

/// A transformed provider response in unified format.
#[derive(Debug, Clone)]
pub struct TransformedResponse {
    /// The assistant message.
    pub message: AssistantMessage,
    /// Extracted function calls.
    pub function_calls: Option<Vec<FunctionCall>>,
    /// Token usage.
    pub usage: Option<TokenUsage>,
    /// Stop reason.
    pub stop_reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_llm_response_serde_round_trip() {
        let response = LLMResponse {
            message: AssistantMessage {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Hello!".to_string(),
                }],
                tool_calls: None,
            },
            function_calls: Some(vec![FunctionCall {
                id: "call_1".to_string(),
                name: "get_weather".to_string(),
                input: json!({"city": "London"}),
            }]),
            metadata: ResponseMetadata {
                model_id: "claude-sonnet-4-20250514".to_string(),
                provider_id: "anthropic".to_string(),
                latency_ms: 1234,
                tokens_used: Some(TokenUsage {
                    input_tokens: 100,
                    output_tokens: 50,
                    total_tokens: 150,
                }),
                stop_reason: Some("end_turn".to_string()),
            },
        };

        let serialized = serde_json::to_string(&response).expect("serialize");
        let deserialized: LLMResponse = serde_json::from_str(&serialized).expect("deserialize");

        assert_eq!(deserialized.message.role, "assistant");
        assert_eq!(deserialized.metadata.model_id, "claude-sonnet-4-20250514");
        assert_eq!(deserialized.metadata.latency_ms, 1234);
        assert_eq!(
            deserialized
                .metadata
                .tokens_used
                .as_ref()
                .unwrap()
                .input_tokens,
            100
        );
        assert_eq!(deserialized.function_calls.as_ref().unwrap().len(), 1);
        assert_eq!(
            deserialized.function_calls.as_ref().unwrap()[0].name,
            "get_weather"
        );
    }

    #[test]
    fn test_content_block_tool_use_serialization() {
        let block = ContentBlock::ToolUse {
            id: "tu_1".to_string(),
            name: "search".to_string(),
            input: json!({"query": "test"}),
        };

        let serialized = serde_json::to_value(&block).expect("serialize");
        assert_eq!(serialized["type"], "tool_use");
        assert_eq!(serialized["id"], "tu_1");
        assert_eq!(serialized["name"], "search");
    }

    #[test]
    fn test_content_block_tool_result_serialization() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "tu_1".to_string(),
            content: "result data".to_string(),
            is_error: Some(true),
        };

        let serialized = serde_json::to_value(&block).expect("serialize");
        assert_eq!(serialized["type"], "tool_result");
        assert_eq!(serialized["tool_use_id"], "tu_1");
        assert_eq!(serialized["content"], "result data");
        assert_eq!(serialized["is_error"], true);
    }

    #[test]
    fn test_content_block_tool_result_no_error_field() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "tu_2".to_string(),
            content: "ok".to_string(),
            is_error: None,
        };

        let serialized = serde_json::to_value(&block).expect("serialize");
        assert_eq!(serialized["type"], "tool_result");
        // is_error should be absent when None (skip_serializing_if)
        assert!(serialized.get("is_error").is_none());
    }
}
