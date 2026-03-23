use super::super::error::LlmError;
use super::super::models::{
    AssistantMessage, ContentBlock, FunctionCall, LLMInvocation, MessageContent, TokenUsage,
    TransformedRequest, TransformedResponse, UnifiedMessage,
};
use super::utils;
use super::MessageTransformer;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use tracing::debug;

/// Transformer for the OpenAI Chat Completions API format.
///
/// Also used for OpenAI-compatible providers (XAI, DeepSeek).
/// Handles the complex message reconstruction required when converting
/// Anthropic-style tool results (content blocks in user messages) to
/// OpenAI-style tool results (separate "tool" role messages).
pub struct OpenAITransformer;

impl MessageTransformer for OpenAITransformer {
    fn transform_request(
        &self,
        invocation: &LLMInvocation,
    ) -> Result<TransformedRequest, LlmError> {
        debug!("Transforming request to OpenAI format");

        let mut body = json!({
            "model": invocation.provider_config.model_id,
            "messages": self.transform_messages(&invocation.messages)?,
        });

        // Add optional parameters
        if let Some(temp) = invocation.temperature {
            body["temperature"] = json!(temp);
        }
        if let Some(max_tokens) = invocation.max_tokens {
            body["max_tokens"] = json!(max_tokens);
        }
        if let Some(top_p) = invocation.top_p {
            body["top_p"] = json!(top_p);
        }
        if let Some(stream) = invocation.stream {
            body["stream"] = json!(stream);
        }

        // Add tools if present
        if let Some(tools) = &invocation.tools {
            let openai_tools: Vec<Value> = tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": utils::clean_tool_schema(&tool.input_schema)
                        }
                    })
                })
                .collect();
            body["tools"] = json!(openai_tools);
        }

        Ok(TransformedRequest {
            body,
            headers: HashMap::new(),
        })
    }

    fn transform_response(&self, response: Value) -> Result<TransformedResponse, LlmError> {
        debug!("Transforming OpenAI response to unified format");

        // Extract choice
        let choice = utils::extract_with_fallback(&response, &["choices.0", "results.0"])
            .ok_or_else(|| LlmError::TransformError("No choices in response".to_string()))?;

        // Extract message
        let message = choice
            .get("message")
            .ok_or_else(|| LlmError::TransformError("No message in choice".to_string()))?;

        // Transform content
        let content = self.transform_response_content(message)?;

        // Extract tool calls if present
        let (tool_calls, function_calls) = self.extract_tool_calls(message)?;

        // Extract usage
        let usage = self.extract_usage(&response);

        // Extract stop reason
        let stop_reason = utils::extract_string(&choice, &["finish_reason", "stop_reason"]);

        Ok(TransformedResponse {
            message: AssistantMessage {
                role: "assistant".to_string(),
                content,
                tool_calls,
            },
            function_calls,
            usage,
            stop_reason,
        })
    }
}

impl OpenAITransformer {
    /// Transform unified messages to OpenAI format.
    ///
    /// This handles the complex message reconstruction required when the unified
    /// format (Anthropic-style) has tool results as content blocks within user
    /// messages, but OpenAI expects them as separate "tool" role messages with
    /// the preceding assistant message carrying `tool_calls`.
    fn transform_messages(
        &self,
        messages: &[UnifiedMessage],
    ) -> Result<Vec<Value>, LlmError> {
        let mut openai_messages: Vec<Value> = Vec::new();
        let mut i = 0;
        let mut processed_assistant_indices = HashSet::new();

        while i < messages.len() {
            let message = &messages[i];

            // Check if this is a user message with tool results
            if message.role == "user" {
                if let MessageContent::Blocks { content } = &message.content {
                    let tool_results: Vec<_> = content
                        .iter()
                        .filter_map(|block| {
                            if let ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                ..
                            } = block
                            {
                                Some((tool_use_id.clone(), content.clone()))
                            } else {
                                None
                            }
                        })
                        .collect();

                    if !tool_results.is_empty() {
                        // Find the previous assistant message that should have tool_calls
                        for j in (0..i).rev() {
                            if messages[j].role == "assistant" {
                                // Check if we already processed this assistant message
                                if !processed_assistant_indices.contains(&j) {
                                    // Remove the previously added message without tool_calls
                                    // Find and remove it from openai_messages
                                    let messages_to_check = openai_messages.len().min(i);
                                    for k in (0..messages_to_check).rev() {
                                        if openai_messages[k].get("role")
                                            == Some(&json!("assistant"))
                                        {
                                            openai_messages.remove(k);
                                            break;
                                        }
                                    }

                                    // Now add it with tool_calls
                                    let mut assistant_msg =
                                        self.transform_single_message(&messages[j])?;

                                    // Add tool_calls if not present
                                    if assistant_msg.get("tool_calls").is_none() {
                                        let tool_calls = self.reconstruct_tool_calls(
                                            &messages[j],
                                            &tool_results,
                                        )?;
                                        assistant_msg["tool_calls"] = json!(tool_calls);
                                    }

                                    openai_messages.push(assistant_msg);
                                    processed_assistant_indices.insert(j);
                                }
                                break;
                            }
                        }

                        // Add tool response messages
                        for (tool_use_id, content) in tool_results {
                            openai_messages.push(json!({
                                "role": "tool",
                                "tool_call_id": tool_use_id,
                                "content": content
                            }));
                        }

                        i += 1;
                        continue;
                    }
                }
            }

            // Skip if this assistant message was already processed with tool_calls
            if message.role == "assistant" && processed_assistant_indices.contains(&i) {
                i += 1;
                continue;
            }

            openai_messages.push(self.transform_single_message(message)?);
            i += 1;
        }

        Ok(openai_messages)
    }

    fn transform_single_message(
        &self,
        message: &UnifiedMessage,
    ) -> Result<Value, LlmError> {
        let mut msg = json!({
            "role": message.role,
        });

        match &message.content {
            MessageContent::Text { content } => {
                msg["content"] = json!(content);
            }
            MessageContent::Blocks { content } => {
                let text_parts: Vec<String> = content
                    .iter()
                    .filter_map(|block| {
                        if let ContentBlock::Text { text } = block {
                            Some(text.clone())
                        } else {
                            None
                        }
                    })
                    .collect();

                // Set content - OpenAI requires this field to be a string (even if empty)
                if !text_parts.is_empty() {
                    msg["content"] = json!(text_parts.join("\n"));
                } else {
                    // OpenAI requires content field to be an empty string when there's no text
                    msg["content"] = json!("");
                }

                // Handle tool uses
                let tool_uses: Vec<Value> = content
                    .iter()
                    .filter_map(|block| {
                        if let ContentBlock::ToolUse { id, name, input } = block {
                            Some(json!({
                                "id": id,
                                "type": "function",
                                "function": {
                                    "name": name,
                                    "arguments": input.to_string()
                                }
                            }))
                        } else {
                            None
                        }
                    })
                    .collect();

                if !tool_uses.is_empty() {
                    msg["tool_calls"] = json!(tool_uses);
                }
            }
        }

        Ok(msg)
    }

    fn reconstruct_tool_calls(
        &self,
        message: &UnifiedMessage,
        tool_results: &[(String, String)],
    ) -> Result<Vec<Value>, LlmError> {
        if let MessageContent::Blocks { content } = &message.content {
            let tool_calls: Vec<Value> = content
                .iter()
                .filter_map(|block| {
                    if let ContentBlock::ToolUse { id, name, input } = block {
                        Some(json!({
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": input.to_string()
                            }
                        }))
                    } else {
                        None
                    }
                })
                .collect();

            if !tool_calls.is_empty() {
                return Ok(tool_calls);
            }
        }

        // If we can't find tool uses in the message, create from results
        Ok(tool_results
            .iter()
            .map(|(id, _)| {
                json!({
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": "unknown",
                        "arguments": "{}"
                    }
                })
            })
            .collect())
    }

    fn transform_response_content(
        &self,
        message: &Value,
    ) -> Result<Vec<ContentBlock>, LlmError> {
        let mut blocks = Vec::new();

        // Extract text content
        if let Some(content) = message.get("content") {
            if let Some(text) = content.as_str() {
                if !text.is_empty() {
                    blocks.push(ContentBlock::Text {
                        text: text.to_string(),
                    });
                }
            }
        }

        Ok(blocks)
    }

    fn extract_tool_calls(
        &self,
        message: &Value,
    ) -> Result<(Option<Value>, Option<Vec<FunctionCall>>), LlmError> {
        if let Some(tool_calls) = message.get("tool_calls") {
            if let Some(calls_array) = tool_calls.as_array() {
                let function_calls: Vec<FunctionCall> = calls_array
                    .iter()
                    .filter_map(|call| {
                        let id = call.get("id")?.as_str()?;
                        let function = call.get("function")?;
                        let name = function.get("name")?.as_str()?;
                        let args_str = function.get("arguments")?.as_str()?;

                        let input = serde_json::from_str(args_str).unwrap_or(json!({}));

                        Some(FunctionCall {
                            id: id.to_string(),
                            name: name.to_string(),
                            input,
                        })
                    })
                    .collect();

                if !function_calls.is_empty() {
                    return Ok((Some(tool_calls.clone()), Some(function_calls)));
                }
            }
        }

        Ok((None, None))
    }

    fn extract_usage(&self, response: &Value) -> Option<TokenUsage> {
        let usage = response.get("usage")?;

        let input_tokens =
            utils::extract_u32(usage, &["prompt_tokens", "input_tokens"], 0);
        let output_tokens =
            utils::extract_u32(usage, &["completion_tokens", "output_tokens"], 0);

        Some(TokenUsage {
            input_tokens,
            output_tokens,
            total_tokens: input_tokens + output_tokens,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::super::models::{ProviderConfig, UnifiedTool};

    fn make_provider_config() -> ProviderConfig {
        ProviderConfig {
            provider_id: "openai".to_string(),
            model_id: "gpt-4o".to_string(),
            endpoint: "https://api.openai.com/v1/chat/completions".to_string(),
            auth_header_name: "Authorization".to_string(),
            auth_header_prefix: Some("Bearer".to_string()),
            secret_path: "test/secret".to_string(),
            secret_key_name: "api_key".to_string(),
            request_transformer: "openai_v1".to_string(),
            response_transformer: "openai_v1".to_string(),
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
        let transformer = OpenAITransformer;
        let invocation = make_invocation(vec![UnifiedMessage {
            role: "user".to_string(),
            content: MessageContent::Text {
                content: "Hello".to_string(),
            },
        }]);

        let result = transformer.transform_request(&invocation).unwrap();

        assert_eq!(result.body["model"], "gpt-4o");
        assert!(result.body["messages"].is_array());
        assert_eq!(result.body["max_tokens"], 1024);
        // OpenAI transformer should not add custom headers
        assert!(result.headers.is_empty());
    }

    #[test]
    fn test_transform_request_with_tools() {
        let transformer = OpenAITransformer;
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
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["function"]["name"], "get_weather");
        assert_eq!(tools[0]["function"]["description"], "Get weather for a city");
        assert!(tools[0]["function"]["parameters"].is_object());
    }

    #[test]
    fn test_transform_response_text() {
        let transformer = OpenAITransformer;
        let response = json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello! How can I help you?"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 20,
                "total_tokens": 30
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
        assert_eq!(result.stop_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn test_transform_response_tool_calls() {
        let transformer = OpenAITransformer;
        let response = json!({
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc123",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"city\":\"London\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 50,
                "completion_tokens": 30,
                "total_tokens": 80
            }
        });

        let result = transformer.transform_response(response).unwrap();

        // Function calls should be extracted and parsed
        let calls = result.function_calls.unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_abc123");
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[0].input["city"], "London");

        // tool_calls should be preserved as raw Value
        assert!(result.message.tool_calls.is_some());

        // Stop reason should be "tool_calls"
        assert_eq!(result.stop_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn test_transform_response_tool_calls_invalid_json() {
        let transformer = OpenAITransformer;
        let response = json!({
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_bad",
                        "type": "function",
                        "function": {
                            "name": "some_tool",
                            "arguments": "not valid json"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        });

        let result = transformer.transform_response(response).unwrap();

        // Should gracefully fall back to empty object for invalid JSON arguments
        let calls = result.function_calls.unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "some_tool");
        assert_eq!(calls[0].input, json!({}));
    }

    #[test]
    fn test_message_reconstruction_with_tool_results() {
        let transformer = OpenAITransformer;

        // Simulate a conversation:
        // 1. User asks something
        // 2. Assistant responds with tool_use (as content blocks)
        // 3. User provides tool_result (as content blocks in user message)
        let messages = vec![
            UnifiedMessage {
                role: "user".to_string(),
                content: MessageContent::Text {
                    content: "What is the weather?".to_string(),
                },
            },
            UnifiedMessage {
                role: "assistant".to_string(),
                content: MessageContent::Blocks {
                    content: vec![
                        ContentBlock::Text {
                            text: "Let me check.".to_string(),
                        },
                        ContentBlock::ToolUse {
                            id: "call_1".to_string(),
                            name: "get_weather".to_string(),
                            input: json!({"city": "London"}),
                        },
                    ],
                },
            },
            UnifiedMessage {
                role: "user".to_string(),
                content: MessageContent::Blocks {
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: "call_1".to_string(),
                        content: "Sunny, 22C".to_string(),
                        is_error: None,
                    }],
                },
            },
        ];

        let openai_messages = transformer.transform_messages(&messages).unwrap();

        // Should have:
        // 1. User message ("What is the weather?")
        // 2. Assistant message with tool_calls
        // 3. Tool role message with result
        assert_eq!(openai_messages.len(), 3);

        // First: user message
        assert_eq!(openai_messages[0]["role"], "user");
        assert_eq!(openai_messages[0]["content"], "What is the weather?");

        // Second: assistant message should have tool_calls reconstructed
        assert_eq!(openai_messages[1]["role"], "assistant");
        let tool_calls = openai_messages[1]["tool_calls"].as_array().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["id"], "call_1");
        assert_eq!(tool_calls[0]["function"]["name"], "get_weather");

        // Third: tool role message
        assert_eq!(openai_messages[2]["role"], "tool");
        assert_eq!(openai_messages[2]["tool_call_id"], "call_1");
        assert_eq!(openai_messages[2]["content"], "Sunny, 22C");
    }

    #[test]
    fn test_extract_usage() {
        let transformer = OpenAITransformer;
        let response = json!({
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "test"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 50,
                "total_tokens": 150
            }
        });

        let result = transformer.transform_response(response).unwrap();
        let usage = result.usage.unwrap();

        // OpenAI's prompt_tokens maps to input_tokens
        assert_eq!(usage.input_tokens, 100);
        // OpenAI's completion_tokens maps to output_tokens
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.total_tokens, 150);
    }

    #[test]
    fn test_transform_request_with_system_message() {
        let transformer = OpenAITransformer;
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

        // OpenAI keeps system messages inline (unlike Anthropic which extracts them)
        let messages = result.body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "You are a helpful assistant.");
        assert_eq!(messages[1]["role"], "user");
    }

    #[test]
    fn test_finish_reason_tool_calls() {
        let transformer = OpenAITransformer;
        let response = json!({
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "test",
                            "arguments": "{}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}
        });

        let result = transformer.transform_response(response).unwrap();
        assert_eq!(result.stop_reason.as_deref(), Some("tool_calls"));
    }
}
