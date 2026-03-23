use super::super::error::LlmError;
use super::super::models::{
    AssistantMessage, ContentBlock, FunctionCall, LLMInvocation, MessageContent, TokenUsage,
    TransformedRequest, TransformedResponse, UnifiedMessage,
};
use super::utils;
use super::MessageTransformer;
use serde_json::{json, Value};
use std::collections::HashMap;
use tracing::{debug, warn};

/// Transformer for the Anthropic Messages API format.
pub struct AnthropicTransformer;

impl MessageTransformer for AnthropicTransformer {
    fn transform_request(
        &self,
        invocation: &LLMInvocation,
    ) -> Result<TransformedRequest, LlmError> {
        debug!("Transforming request to Anthropic format");

        // Transform messages
        let messages = self.transform_messages(&invocation.messages)?;

        // Extract system prompt if present
        let system = self.extract_system_prompt(&invocation.messages);

        let mut body = json!({
            "model": invocation.provider_config.model_id,
            "messages": messages,
            "max_tokens": invocation.max_tokens.unwrap_or(4096),
        });

        // Add system prompt if present
        if let Some(system_text) = system {
            body["system"] = json!([{
                "type": "text",
                "text": system_text,
                "cache_control": {"type": "ephemeral"}
            }]);
        }

        // Add optional parameters
        if let Some(temp) = invocation.temperature {
            body["temperature"] = json!(temp);
        }
        if let Some(top_p) = invocation.top_p {
            body["top_p"] = json!(top_p);
        }
        if let Some(stream) = invocation.stream {
            body["stream"] = json!(stream);
        }

        // Add tools if present
        if let Some(tools) = &invocation.tools {
            let anthropic_tools: Vec<Value> = tools
                .iter()
                .enumerate()
                .map(|(i, tool)| {
                    let mut tool_json = json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": utils::clean_tool_schema(&tool.input_schema)
                    });

                    // Add cache control to last tool
                    if i == tools.len() - 1 {
                        tool_json["cache_control"] = json!({"type": "ephemeral"});
                    }

                    tool_json
                })
                .collect();
            body["tools"] = json!(anthropic_tools);
            body["tool_choice"] = json!({"type": "auto"});
        }

        // Add Anthropic-specific headers
        let mut headers = HashMap::new();
        headers.insert("anthropic-version".to_string(), "2023-06-01".to_string());
        headers.insert("content-type".to_string(), "application/json".to_string());

        Ok(TransformedRequest { body, headers })
    }

    fn transform_response(&self, response: Value) -> Result<TransformedResponse, LlmError> {
        debug!("Transforming Anthropic response to unified format");

        // Extract content blocks
        let content_array = response
            .get("content")
            .ok_or_else(|| LlmError::TransformError("No content in response".to_string()))?
            .as_array()
            .ok_or_else(|| LlmError::TransformError("Content is not an array".to_string()))?;

        let mut unified_blocks = Vec::new();
        let mut function_calls = Vec::new();
        let mut tool_use_blocks = Vec::new();

        for block in content_array {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        unified_blocks.push(ContentBlock::Text {
                            text: text.to_string(),
                        });
                    }
                }
                Some("tool_use") => {
                    let id = block
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let input = block.get("input").cloned().unwrap_or(json!({}));

                    // Add to content blocks
                    unified_blocks.push(ContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    });

                    // Also add to function_calls
                    function_calls.push(FunctionCall {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    });

                    // Keep the original tool_use block for provider-specific format
                    tool_use_blocks.push(block.clone());
                }
                _ => {
                    warn!("Unknown content block type: {:?}", block.get("type"));
                }
            }
        }

        // Extract usage
        let usage = self.extract_usage(&response);

        // Extract stop reason
        let stop_reason = response
            .get("stop_reason")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string());

        Ok(TransformedResponse {
            message: AssistantMessage {
                role: "assistant".to_string(),
                content: unified_blocks,
                tool_calls: if !tool_use_blocks.is_empty() {
                    Some(json!(tool_use_blocks))
                } else {
                    None
                },
            },
            function_calls: if !function_calls.is_empty() {
                Some(function_calls)
            } else {
                None
            },
            usage,
            stop_reason,
        })
    }
}

impl AnthropicTransformer {
    fn extract_system_prompt(&self, messages: &[UnifiedMessage]) -> Option<String> {
        // Check if first message is system
        if let Some(first) = messages.first() {
            if first.role == "system" {
                match &first.content {
                    MessageContent::Text { content } => return Some(content.clone()),
                    MessageContent::Blocks { content } => {
                        let texts: Vec<String> = content
                            .iter()
                            .filter_map(|block| {
                                if let ContentBlock::Text { text } = block {
                                    Some(text.clone())
                                } else {
                                    None
                                }
                            })
                            .collect();
                        if !texts.is_empty() {
                            return Some(texts.join("\n"));
                        }
                    }
                }
            }
        }
        None
    }

    fn transform_messages(&self, messages: &[UnifiedMessage]) -> Result<Vec<Value>, LlmError> {
        let mut anthropic_messages = Vec::new();

        for message in messages {
            // Skip system messages (handled separately)
            if message.role == "system" {
                continue;
            }

            let content = match &message.content {
                MessageContent::Text { content } => {
                    vec![json!({
                        "type": "text",
                        "text": content
                    })]
                }
                MessageContent::Blocks { content } => self.transform_content_blocks(content)?,
            };

            // Only add non-empty messages
            if !content.is_empty() {
                anthropic_messages.push(json!({
                    "role": message.role,
                    "content": content
                }));
            }
        }

        Ok(anthropic_messages)
    }

    fn transform_content_blocks(&self, blocks: &[ContentBlock]) -> Result<Vec<Value>, LlmError> {
        let mut anthropic_blocks = Vec::new();

        for block in blocks {
            match block {
                ContentBlock::Text { text } => {
                    anthropic_blocks.push(json!({
                        "type": "text",
                        "text": text
                    }));
                }
                ContentBlock::ToolUse { id, name, input } => {
                    anthropic_blocks.push(json!({
                        "type": "tool_use",
                        "id": id,
                        "name": name,
                        "input": input
                    }));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    let mut result_json = json!({
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": content
                    });
                    // Include is_error field when present and true
                    if let Some(true) = is_error {
                        result_json["is_error"] = json!(true);
                    }
                    anthropic_blocks.push(result_json);
                }
                ContentBlock::Image { source } => {
                    anthropic_blocks.push(json!({
                        "type": "image",
                        "source": {
                            "type": source.source_type,
                            "media_type": source.media_type,
                            "data": source.data
                        }
                    }));
                }
            }
        }

        Ok(anthropic_blocks)
    }

    fn extract_usage(&self, response: &Value) -> Option<TokenUsage> {
        let usage = response.get("usage")?;

        let input_tokens = usage
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let output_tokens = usage
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        Some(TokenUsage {
            input_tokens,
            output_tokens,
            total_tokens: input_tokens + output_tokens,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::models::{ProviderConfig, UnifiedTool};
    use super::*;

    fn make_provider_config() -> ProviderConfig {
        ProviderConfig {
            provider_id: "anthropic".to_string(),
            model_id: "claude-sonnet-4-20250514".to_string(),
            endpoint: "https://api.anthropic.com/v1/messages".to_string(),
            auth_header_name: "x-api-key".to_string(),
            auth_header_prefix: None,
            secret_path: "test/secret".to_string(),
            secret_key_name: "api_key".to_string(),
            request_transformer: "anthropic_v1".to_string(),
            response_transformer: "anthropic_v1".to_string(),
            timeout: 30,
            custom_headers: None,
        }
    }

    fn make_invocation(messages: Vec<UnifiedMessage>) -> LLMInvocation {
        LLMInvocation {
            provider_config: make_provider_config(),
            messages,
            tools: None,
            temperature: None,
            max_tokens: Some(1024),
            top_p: None,
            stream: None,
        }
    }

    #[test]
    fn test_transform_request_basic() {
        let transformer = AnthropicTransformer;
        let invocation = make_invocation(vec![UnifiedMessage {
            role: "user".to_string(),
            content: MessageContent::Text {
                content: "Hello".to_string(),
            },
        }]);

        let result = transformer.transform_request(&invocation).unwrap();

        assert_eq!(result.body["model"], "claude-sonnet-4-20250514");
        assert!(result.body["messages"].is_array());
        assert_eq!(result.body["max_tokens"], 1024);
    }

    #[test]
    fn test_transform_request_with_system_prompt() {
        let transformer = AnthropicTransformer;
        let invocation = make_invocation(vec![
            UnifiedMessage {
                role: "system".to_string(),
                content: MessageContent::Text {
                    content: "You are a helpful assistant.".to_string(),
                },
            },
            UnifiedMessage {
                role: "user".to_string(),
                content: MessageContent::Text {
                    content: "Hello".to_string(),
                },
            },
        ]);

        let result = transformer.transform_request(&invocation).unwrap();

        // System prompt should be extracted to top-level "system" field
        assert!(result.body.get("system").is_some());
        let system_blocks = result.body["system"].as_array().unwrap();
        assert_eq!(system_blocks[0]["text"], "You are a helpful assistant.");
        assert_eq!(system_blocks[0]["type"], "text");

        // Messages should not contain the system message
        let messages = result.body["messages"].as_array().unwrap();
        for msg in messages {
            assert_ne!(msg["role"], "system");
        }
    }

    #[test]
    fn test_transform_request_with_tools() {
        let transformer = AnthropicTransformer;
        let mut invocation = make_invocation(vec![UnifiedMessage {
            role: "user".to_string(),
            content: MessageContent::Text {
                content: "What is the weather?".to_string(),
            },
        }]);
        invocation.tools = Some(vec![UnifiedTool {
            name: "get_weather".to_string(),
            description: "Get weather for a city".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "city": {"type": "string"}
                },
                "required": ["city"]
            }),
        }]);

        let result = transformer.transform_request(&invocation).unwrap();

        let tools = result.body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "get_weather");
        assert_eq!(tools[0]["description"], "Get weather for a city");
        assert!(tools[0]["input_schema"].is_object());

        // Last tool should have cache_control
        assert_eq!(tools[0]["cache_control"]["type"], "ephemeral");

        // tool_choice should be set to auto
        assert_eq!(result.body["tool_choice"]["type"], "auto");
    }

    #[test]
    fn test_transform_request_headers() {
        let transformer = AnthropicTransformer;
        let invocation = make_invocation(vec![UnifiedMessage {
            role: "user".to_string(),
            content: MessageContent::Text {
                content: "Hello".to_string(),
            },
        }]);

        let result = transformer.transform_request(&invocation).unwrap();

        assert_eq!(
            result.headers.get("anthropic-version").unwrap(),
            "2023-06-01"
        );
        assert_eq!(
            result.headers.get("content-type").unwrap(),
            "application/json"
        );
    }

    #[test]
    fn test_transform_response_text() {
        let transformer = AnthropicTransformer;
        let response = json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "content": [
                {
                    "type": "text",
                    "text": "Hello! How can I help you?"
                }
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 20
            }
        });

        let result = transformer.transform_response(response).unwrap();

        assert_eq!(result.message.role, "assistant");
        assert_eq!(result.message.content.len(), 1);
        match &result.message.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "Hello! How can I help you?"),
            _ => panic!("Expected Text block"),
        }
        assert!(result.function_calls.is_none());
        assert!(result.message.tool_calls.is_none());
    }

    #[test]
    fn test_transform_response_tool_use() {
        let transformer = AnthropicTransformer;
        let response = json!({
            "id": "msg_456",
            "type": "message",
            "role": "assistant",
            "content": [
                {
                    "type": "text",
                    "text": "Let me check the weather."
                },
                {
                    "type": "tool_use",
                    "id": "toolu_123",
                    "name": "get_weather",
                    "input": {"city": "London"}
                }
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 50,
                "output_tokens": 30
            }
        });

        let result = transformer.transform_response(response).unwrap();

        // Should have 2 content blocks
        assert_eq!(result.message.content.len(), 2);
        match &result.message.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "Let me check the weather."),
            _ => panic!("Expected Text block"),
        }
        match &result.message.content[1] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "toolu_123");
                assert_eq!(name, "get_weather");
                assert_eq!(input["city"], "London");
            }
            _ => panic!("Expected ToolUse block"),
        }

        // Function calls should be extracted
        let calls = result.function_calls.unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "toolu_123");
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[0].input["city"], "London");

        // tool_calls should be set for provider-specific format
        assert!(result.message.tool_calls.is_some());

        // Stop reason should be "tool_use"
        assert_eq!(result.stop_reason.unwrap(), "tool_use");
    }

    #[test]
    fn test_transform_response_usage() {
        let transformer = AnthropicTransformer;
        let response = json!({
            "id": "msg_789",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Hello"}
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50
            }
        });

        let result = transformer.transform_response(response).unwrap();

        let usage = result.usage.unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.total_tokens, 150);
    }

    #[test]
    fn test_transform_response_stop_reason_end_turn() {
        let transformer = AnthropicTransformer;
        let response = json!({
            "content": [{"type": "text", "text": "done"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 3}
        });

        let result = transformer.transform_response(response).unwrap();
        assert_eq!(result.stop_reason.as_deref(), Some("end_turn"));
    }

    #[test]
    fn test_transform_response_stop_reason_tool_use() {
        let transformer = AnthropicTransformer;
        let response = json!({
            "content": [{
                "type": "tool_use",
                "id": "toolu_1",
                "name": "search",
                "input": {"q": "test"}
            }],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });

        let result = transformer.transform_response(response).unwrap();
        assert_eq!(result.stop_reason.as_deref(), Some("tool_use"));
    }

    #[test]
    fn test_transform_content_blocks_with_tool_result_is_error() {
        let transformer = AnthropicTransformer;
        let blocks = vec![ContentBlock::ToolResult {
            tool_use_id: "toolu_1".to_string(),
            content: "Error: not found".to_string(),
            is_error: Some(true),
        }];

        let result = transformer.transform_content_blocks(&blocks).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["is_error"], true);
        assert_eq!(result[0]["type"], "tool_result");
    }

    #[test]
    fn test_transform_content_blocks_with_tool_result_no_error() {
        let transformer = AnthropicTransformer;
        let blocks = vec![ContentBlock::ToolResult {
            tool_use_id: "toolu_2".to_string(),
            content: "Success".to_string(),
            is_error: None,
        }];

        let result = transformer.transform_content_blocks(&blocks).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].get("is_error").is_none());
    }
}
