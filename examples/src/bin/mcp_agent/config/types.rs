use crate::llm::models::ProviderConfig;
use serde::{Deserialize, Serialize};

/// Full agent configuration loaded from the AgentRegistry DynamoDB table.
///
/// Both `Serialize` and `Deserialize` are required so this type can
/// survive a `ctx.step()` checkpoint round-trip (CONF-04).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Agent name (DynamoDB partition key).
    pub agent_name: String,
    /// Agent version (DynamoDB sort key).
    pub version: String,
    /// System prompt for the LLM.
    pub system_prompt: String,
    /// Resolved provider configuration (endpoint, auth, transformers).
    pub provider_config: ProviderConfig,
    /// MCP server endpoint URLs.
    pub mcp_server_urls: Vec<String>,
    /// Agent execution parameters.
    pub parameters: AgentParameters,
}

/// Tunable parameters for agent execution.
///
/// Derives `Serialize + Deserialize` for checkpoint compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentParameters {
    /// Maximum number of LLM call iterations.
    pub max_iterations: u32,
    /// Sampling temperature (0.0 - 1.0).
    pub temperature: f32,
    /// Maximum tokens to generate per LLM call.
    pub max_tokens: u32,
    /// Overall timeout for the agent execution in seconds.
    pub timeout_seconds: u32,
}

impl Default for AgentParameters {
    fn default() -> Self {
        Self {
            max_iterations: 10,
            temperature: 0.7,
            max_tokens: 4096,
            timeout_seconds: 120,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_parameters_default() {
        let params = AgentParameters::default();
        assert_eq!(params.max_iterations, 10);
        assert!((params.temperature - 0.7).abs() < f32::EPSILON);
        assert_eq!(params.max_tokens, 4096);
        assert_eq!(params.timeout_seconds, 120);
    }

    #[test]
    fn test_agent_parameters_serde_round_trip() {
        let params = AgentParameters {
            max_iterations: 5,
            temperature: 0.3,
            max_tokens: 2048,
            timeout_seconds: 60,
        };
        let json = serde_json::to_string(&params).expect("serialize");
        let deserialized: AgentParameters = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.max_iterations, 5);
        assert!((deserialized.temperature - 0.3).abs() < f32::EPSILON);
        assert_eq!(deserialized.max_tokens, 2048);
        assert_eq!(deserialized.timeout_seconds, 60);
    }
}
