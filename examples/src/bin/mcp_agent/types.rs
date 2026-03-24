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
