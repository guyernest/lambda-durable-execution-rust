use serde::{Deserialize, Serialize};

use crate::config::types::{AgentConfig, AgentParameters};
use crate::llm::models::{LLMResponse, UnifiedMessage};

/// Agent handler input.
///
/// Supports two modes:
/// 1. **Registry mode**: `agent_name` + `version` — handler loads config from DynamoDB
/// 2. **Inline mode**: `inline_config` provided — handler skips DynamoDB lookup
///
/// Inline mode is used by pmcp-run's execute-agent function which resolves
/// all config (instructions, model, MCP servers) before invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRequest {
    /// Agent name (DynamoDB PK in registry mode, display name in inline mode).
    pub agent_name: String,
    /// Agent version (DynamoDB SK in registry mode).
    #[serde(default = "default_version")]
    pub version: String,
    /// Initial conversation messages.
    pub messages: Vec<UnifiedMessage>,
    /// Pre-resolved config — when present, skips DynamoDB lookup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_config: Option<InlineAgentConfig>,
    /// Execution tracking ID — when present, the handler writes status back to DynamoDB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    /// DynamoDB table for execution tracking (required if execution_id is set).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executions_table: Option<String>,
}

fn default_version() -> String {
    "latest".to_string()
}

/// Pre-resolved agent configuration passed in the invocation payload.
/// Used by pmcp-run to avoid schema mismatch with the original AgentRegistry table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineAgentConfig {
    /// System prompt / instructions for the LLM.
    pub instructions: String,
    /// LLM provider name ("anthropic" or "openai").
    pub provider: String,
    /// LLM model ID (e.g. "claude-sonnet-4-20250514").
    pub model_id: String,
    /// MCP server endpoint URLs.
    #[serde(default)]
    pub mcp_server_urls: Vec<String>,
    /// Sampling temperature.
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// Max tokens per LLM call.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// Max agent loop iterations.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    /// Secrets Manager path for LLM API keys (e.g. "pmcp/orgs/{orgId}/agents/llm-keys").
    /// Overrides the provider's default secret_path when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_path: Option<String>,
    /// Key name within the secret (e.g. "ANTHROPIC_API_KEY").
    /// Overrides the provider's default secret_key_name when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_key_name: Option<String>,
}

fn default_temperature() -> f32 { AgentParameters::default().temperature }
fn default_max_tokens() -> u32 { AgentParameters::default().max_tokens }
fn default_max_iterations() -> u32 { AgentParameters::default().max_iterations }

impl InlineAgentConfig {
    /// Convert to the internal `AgentConfig` used by the handler.
    pub fn to_agent_config(
        self,
        agent_name: &str,
        version: &str,
    ) -> Result<AgentConfig, crate::config::error::ConfigError> {
        let mut provider_config =
            crate::config::loader::map_provider_config(&self.provider, &self.model_id)?;

        // Override secret path and key name if provided by the caller
        if let Some(path) = self.secret_path {
            provider_config.secret_path = path;
        }
        if let Some(key) = self.secret_key_name {
            provider_config.secret_key_name = key;
        }

        Ok(AgentConfig {
            agent_name: agent_name.to_string(),
            version: version.to_string(),
            system_prompt: self.instructions,
            provider_config,
            mcp_server_urls: self.mcp_server_urls,
            parameters: AgentParameters {
                max_iterations: self.max_iterations,
                temperature: self.temperature,
                max_tokens: self.max_tokens,
                timeout_seconds: AgentParameters::default().timeout_seconds,
            },
        })
    }
}

/// Metadata about the agent execution for observability (OBS-01, OBS-02).
///
/// Tracks token usage across all LLM call iterations, tool invocations,
/// iteration count, and wall-clock elapsed time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetadata {
    /// Number of agent loop iterations executed.
    pub iterations: u32,
    /// Total input tokens across all LLM calls.
    pub total_input_tokens: u32,
    /// Total output tokens across all LLM calls.
    pub total_output_tokens: u32,
    /// Tool names called across all iterations (not deduplicated -- shows full history).
    pub tools_called: Vec<String>,
    /// Wall-clock milliseconds from handler start to response.
    pub elapsed_ms: u64,
}

/// Agent handler output (per D-02 -- matches Step Functions LLMResponse shape).
///
/// Uses `#[serde(flatten)]` so the JSON shape is identical to `LLMResponse`,
/// providing a drop-in replacement for the Step Functions agent response.
/// The optional `agent_metadata` field is omitted from JSON when `None`
/// to preserve backward compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    /// The final LLM response, flattened into the top-level JSON object.
    #[serde(flatten)]
    pub response: LLMResponse,
    /// Agent execution metadata (token usage, iterations, elapsed time).
    /// Omitted from serialized JSON when None for backward compatibility.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_metadata: Option<AgentMetadata>,
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
            inline_config: None,
            execution_id: None,
            executions_table: None,
        };

        let serialized = serde_json::to_value(&request).expect("serialize");

        // Verify JSON keys
        assert!(serialized.get("agent_name").is_some());
        assert!(serialized.get("version").is_some());
        assert!(serialized.get("messages").is_some());

        let json_str = serde_json::to_string(&request).expect("serialize to string");
        let deserialized: AgentRequest = serde_json::from_str(&json_str).expect("deserialize");

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
            agent_metadata: None,
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
        let deserialized: IterationResult = serde_json::from_str(&json_str).expect("deserialize");

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
        let deserialized: ToolCallResult = serde_json::from_str(&json_str).expect("deserialize");

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

    #[test]
    fn test_agent_metadata_serde_round_trip() {
        let metadata = AgentMetadata {
            iterations: 3,
            total_input_tokens: 1500,
            total_output_tokens: 750,
            tools_called: vec![
                "calc__add".to_string(),
                "search__query".to_string(),
                "calc__add".to_string(),
            ],
            elapsed_ms: 4200,
        };

        let json_str = serde_json::to_string(&metadata).expect("serialize");
        let deserialized: AgentMetadata = serde_json::from_str(&json_str).expect("deserialize");

        assert_eq!(deserialized.iterations, 3);
        assert_eq!(deserialized.total_input_tokens, 1500);
        assert_eq!(deserialized.total_output_tokens, 750);
        assert_eq!(deserialized.tools_called.len(), 3);
        assert_eq!(deserialized.tools_called[0], "calc__add");
        assert_eq!(deserialized.tools_called[1], "search__query");
        assert_eq!(deserialized.tools_called[2], "calc__add");
        assert_eq!(deserialized.elapsed_ms, 4200);
    }

    #[test]
    fn test_agent_response_with_metadata() {
        let llm_response = make_test_llm_response("end_turn", false);
        let agent_response = AgentResponse {
            response: llm_response,
            agent_metadata: Some(AgentMetadata {
                iterations: 2,
                total_input_tokens: 500,
                total_output_tokens: 200,
                tools_called: vec!["calc__multiply".to_string()],
                elapsed_ms: 3000,
            }),
        };

        let json = serde_json::to_value(&agent_response).expect("serialize");

        // agent_metadata should be present at top level alongside flattened LLM fields
        assert!(
            json.get("agent_metadata").is_some(),
            "Expected 'agent_metadata' key in JSON"
        );
        assert!(
            json.get("message").is_some(),
            "Expected flattened 'message' key"
        );
        assert!(
            json.get("metadata").is_some(),
            "Expected flattened 'metadata' key"
        );

        // Verify agent_metadata fields
        let am = &json["agent_metadata"];
        assert_eq!(am["iterations"], 2);
        assert_eq!(am["total_input_tokens"], 500);
        assert_eq!(am["total_output_tokens"], 200);
        assert_eq!(am["tools_called"][0], "calc__multiply");
        assert_eq!(am["elapsed_ms"], 3000);
    }

    #[test]
    fn test_agent_response_without_metadata() {
        let llm_response = make_test_llm_response("end_turn", false);
        let agent_response = AgentResponse {
            response: llm_response,
            agent_metadata: None,
        };

        let json = serde_json::to_value(&agent_response).expect("serialize");

        // agent_metadata should NOT be present (skip_serializing_if works)
        assert!(
            json.get("agent_metadata").is_none(),
            "agent_metadata should be absent when None"
        );

        // Flattened LLM fields should still be present
        assert!(json.get("message").is_some());
        assert!(json.get("metadata").is_some());
    }
}
