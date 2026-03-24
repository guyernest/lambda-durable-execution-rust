use serde::{Deserialize, Serialize};

use crate::llm::models::{LLMResponse, UnifiedMessage};

/// Agent handler input (per D-01).
///
/// The caller specifies which agent to run and the initial conversation.
/// The handler loads configuration from AgentRegistry using `agent_name`/`version`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRequest {
    /// Agent name (maps to DynamoDB partition key).
    pub agent_name: String,
    /// Agent version (maps to DynamoDB sort key).
    pub version: String,
    /// Initial conversation messages.
    pub messages: Vec<UnifiedMessage>,
}

/// Agent handler output (per D-02 -- matches Step Functions LLMResponse shape).
///
/// Uses `#[serde(flatten)]` so the JSON shape is identical to `LLMResponse`,
/// providing a drop-in replacement for the Step Functions agent response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    /// The final LLM response, flattened into the top-level JSON object.
    #[serde(flatten)]
    pub response: LLMResponse,
}

/// Result of a single agent loop iteration (checkpointed by `run_in_child_context`).
///
/// Must be `Serialize + Deserialize` for checkpoint round-trip (Pitfall 3 from research).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IterationResult {
    /// The raw LLM response for this iteration.
    pub llm_response: LLMResponse,
    /// The assistant message to append to history.
    pub assistant_message: UnifiedMessage,
    /// Tool results message to append to history (None if no tool calls).
    pub tool_results_message: Option<UnifiedMessage>,
    /// Whether this is the final iteration (end_turn or no tool calls).
    pub is_final: bool,
}

/// Result of a single MCP tool call (checkpointed by `ctx.map()`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    /// ID of the tool_use this result corresponds to.
    pub tool_use_id: String,
    /// The result content text.
    pub content: String,
    /// Whether this result represents an error (from MCP `is_error`).
    pub is_error: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::models::{
        AssistantMessage, ContentBlock, FunctionCall, ResponseMetadata, TokenUsage,
    };
    use serde_json::json;

    fn make_test_llm_response(stop_reason: &str, with_tool_calls: bool) -> LLMResponse {
        let mut content = vec![ContentBlock::Text {
            text: "Hello!".to_string(),
        }];
        let function_calls = if with_tool_calls {
            content.push(ContentBlock::ToolUse {
                id: "tu_1".to_string(),
                name: "calc__multiply".to_string(),
                input: json!({"a": 2, "b": 3}),
            });
            Some(vec![FunctionCall {
                id: "tu_1".to_string(),
                name: "calc__multiply".to_string(),
                input: json!({"a": 2, "b": 3}),
            }])
        } else {
            None
        };

        LLMResponse {
            message: AssistantMessage {
                role: "assistant".to_string(),
                content,
                tool_calls: None,
            },
            function_calls,
            metadata: ResponseMetadata {
                model_id: "claude-sonnet-4-20250514".to_string(),
                provider_id: "anthropic".to_string(),
                latency_ms: 500,
                tokens_used: Some(TokenUsage {
                    input_tokens: 100,
                    output_tokens: 50,
                    total_tokens: 150,
                }),
                stop_reason: Some(stop_reason.to_string()),
            },
        }
    }

    #[test]
    fn test_agent_request_serde_round_trip() {
        let request = AgentRequest {
            agent_name: "test-agent".to_string(),
            version: "v1".to_string(),
            messages: vec![UnifiedMessage {
                role: "user".to_string(),
                content: crate::llm::models::MessageContent::Text {
                    content: "Hello world".to_string(),
                },
            }],
        };

        let serialized = serde_json::to_value(&request).expect("serialize");

        // Verify JSON keys
        assert!(serialized.get("agent_name").is_some());
        assert!(serialized.get("version").is_some());
        assert!(serialized.get("messages").is_some());

        let json_str = serde_json::to_string(&request).expect("serialize to string");
        let deserialized: AgentRequest =
            serde_json::from_str(&json_str).expect("deserialize");

        assert_eq!(deserialized.agent_name, "test-agent");
        assert_eq!(deserialized.version, "v1");
        assert_eq!(deserialized.messages.len(), 1);
        assert_eq!(deserialized.messages[0].role, "user");
    }

    #[test]
    fn test_agent_response_flatten_matches_llm_response() {
        let llm_response = make_test_llm_response("end_turn", false);
        let agent_response = AgentResponse {
            response: llm_response,
        };

        let json = serde_json::to_value(&agent_response).expect("serialize");

        // Flatten should produce top-level "message" and "metadata" keys,
        // NOT nested under "response" (validates D-02)
        assert!(
            json.get("message").is_some(),
            "Expected top-level 'message' key"
        );
        assert!(
            json.get("metadata").is_some(),
            "Expected top-level 'metadata' key"
        );
        assert!(
            json.get("response").is_none(),
            "Should NOT have 'response' wrapper"
        );

        // Validate stop_reason is accessible at the expected path
        assert_eq!(json["metadata"]["stop_reason"], "end_turn");
        assert_eq!(json["metadata"]["model_id"], "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_iteration_result_serde_round_trip() {
        let llm_response = make_test_llm_response("tool_use", true);
        let assistant_message = UnifiedMessage {
            role: "assistant".to_string(),
            content: crate::llm::models::MessageContent::Blocks {
                content: llm_response.message.content.clone(),
            },
        };

        let iteration = IterationResult {
            llm_response,
            assistant_message,
            tool_results_message: None,
            is_final: true,
        };

        let json_str = serde_json::to_string(&iteration).expect("serialize");
        let deserialized: IterationResult =
            serde_json::from_str(&json_str).expect("deserialize");

        assert!(deserialized.is_final);
        assert!(deserialized.tool_results_message.is_none());
        // Verify function_calls survived round-trip
        let fc = deserialized
            .llm_response
            .function_calls
            .as_ref()
            .expect("function_calls should be present");
        assert_eq!(fc.len(), 1);
        assert_eq!(fc[0].name, "calc__multiply");
        assert_eq!(fc[0].id, "tu_1");
    }

    #[test]
    fn test_tool_call_result_serde_round_trip() {
        let result = ToolCallResult {
            tool_use_id: "tu_99".to_string(),
            content: "Error: division by zero".to_string(),
            is_error: true,
        };

        let json_str = serde_json::to_string(&result).expect("serialize");
        let deserialized: ToolCallResult =
            serde_json::from_str(&json_str).expect("deserialize");

        assert!(deserialized.is_error);
        assert_eq!(deserialized.tool_use_id, "tu_99");
        assert_eq!(deserialized.content, "Error: division by zero");
    }

    #[test]
    fn test_tool_call_result_success() {
        let result = ToolCallResult {
            tool_use_id: "tu_1".to_string(),
            content: "42".to_string(),
            is_error: false,
        };

        assert!(!result.is_error);
        assert_eq!(result.tool_use_id, "tu_1");
        assert_eq!(result.content, "42");
    }
}
