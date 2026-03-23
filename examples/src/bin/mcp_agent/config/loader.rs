use aws_sdk_dynamodb::types::AttributeValue;
use serde::de::DeserializeOwned;
use std::collections::HashMap;

use super::error::ConfigError;
use super::types::{AgentConfig, AgentParameters};
use crate::llm::models::ProviderConfig;

/// Loads agent configuration from the AgentRegistry DynamoDB table.
///
/// Creates a DynamoDB client using the default AWS config and performs
/// a `get_item` using `agent_name` (PK) and `version` (SK).
///
/// The table name is read from the `AGENT_REGISTRY_TABLE` environment
/// variable, falling back to `"AgentRegistry"` if not set.
pub async fn load_agent_config(
    table_name: &str,
    agent_name: &str,
    version: &str,
) -> Result<AgentConfig, ConfigError> {
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let client = aws_sdk_dynamodb::Client::new(&config);

    let result = client
        .get_item()
        .table_name(table_name)
        .key("agent_name", AttributeValue::S(agent_name.to_string()))
        .key("version", AttributeValue::S(version.to_string()))
        .send()
        .await
        .map_err(|e| ConfigError::DynamoDbError(e.to_string()))?;

    let item = result.item().ok_or_else(|| ConfigError::AgentNotFound {
        agent_name: agent_name.to_string(),
        version: version.to_string(),
    })?;

    parse_agent_config(item)
}

/// Parses an `AgentConfig` from a DynamoDB item attribute map.
///
/// Required string fields: `agent_name`, `version`, `system_prompt`,
/// `llm_provider`, `llm_model`.
///
/// Optional JSON fields: `parameters` (defaults to `AgentParameters::default()`),
/// `mcp_servers` (defaults to empty vec).
pub fn parse_agent_config(
    item: &HashMap<String, AttributeValue>,
) -> Result<AgentConfig, ConfigError> {
    let agent_name = get_string(item, "agent_name")?;
    let version = get_string(item, "version")?;
    let system_prompt = get_string(item, "system_prompt")?;
    let llm_provider = get_string(item, "llm_provider")?;
    let llm_model = get_string(item, "llm_model")?;

    let parameters: AgentParameters =
        get_optional_json_string_as(item, "parameters", AgentParameters::default());
    let mcp_server_urls: Vec<String> = get_optional_json_string_as(item, "mcp_servers", Vec::new());

    let provider_config = map_provider_config(&llm_provider, &llm_model)?;

    Ok(AgentConfig {
        agent_name,
        version,
        system_prompt,
        provider_config,
        mcp_server_urls,
        parameters,
    })
}

/// Maps `llm_provider` and `llm_model` strings to a full `ProviderConfig`.
///
/// Supported providers:
/// - `"claude"` / `"anthropic"` -- Anthropic Messages API
/// - `"openai"` -- OpenAI Chat Completions API
pub fn map_provider_config(
    llm_provider: &str,
    llm_model: &str,
) -> Result<ProviderConfig, ConfigError> {
    match llm_provider {
        "claude" | "anthropic" => {
            let mut custom_headers = HashMap::new();
            custom_headers.insert("anthropic-version".to_string(), "2023-06-01".to_string());

            Ok(ProviderConfig {
                provider_id: "anthropic".to_string(),
                model_id: llm_model.to_string(),
                endpoint: "https://api.anthropic.com/v1/messages".to_string(),
                auth_header_name: "x-api-key".to_string(),
                auth_header_prefix: None,
                secret_path: "prod/anthropic/api-key".to_string(),
                secret_key_name: "api_key".to_string(),
                request_transformer: "anthropic_v1".to_string(),
                response_transformer: "anthropic_v1".to_string(),
                timeout: 120,
                custom_headers: Some(custom_headers),
            })
        }
        "openai" => Ok(ProviderConfig {
            provider_id: "openai".to_string(),
            model_id: llm_model.to_string(),
            endpoint: "https://api.openai.com/v1/chat/completions".to_string(),
            auth_header_name: "Authorization".to_string(),
            auth_header_prefix: Some("Bearer ".to_string()),
            secret_path: "prod/openai/api-key".to_string(),
            secret_key_name: "api_key".to_string(),
            request_transformer: "openai_v1".to_string(),
            response_transformer: "openai_v1".to_string(),
            timeout: 120,
            custom_headers: None,
        }),
        _ => Err(ConfigError::UnsupportedProvider(llm_provider.to_string())),
    }
}

// ===== Helper functions =====

/// Extracts a required string (S) attribute from a DynamoDB item.
fn get_string(item: &HashMap<String, AttributeValue>, key: &str) -> Result<String, ConfigError> {
    item.get(key)
        .and_then(|v| v.as_s().ok())
        .map(|s| s.to_string())
        .ok_or_else(|| ConfigError::MissingField(key.to_string()))
}

/// Extracts a required JSON-encoded string (S) attribute and deserializes it.
#[allow(dead_code)]
fn get_json_string_as<T: DeserializeOwned>(
    item: &HashMap<String, AttributeValue>,
    key: &str,
) -> Result<T, ConfigError> {
    let raw = get_string(item, key)?;
    serde_json::from_str(&raw).map_err(|e| ConfigError::InvalidJson {
        field: key.to_string(),
        source: e,
    })
}

/// Extracts an optional JSON-encoded string (S) attribute, returning a default
/// if the field is missing. Returns `ConfigError::InvalidJson` if the field
/// exists but contains malformed JSON.
fn get_optional_json_string_as<T: DeserializeOwned>(
    item: &HashMap<String, AttributeValue>,
    key: &str,
    default: T,
) -> T {
    match item.get(key).and_then(|v| v.as_s().ok()) {
        Some(raw) => serde_json::from_str(raw).unwrap_or(default),
        None => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a DynamoDB attribute map with all required and optional fields.
    fn full_item() -> HashMap<String, AttributeValue> {
        let mut item = HashMap::new();
        item.insert(
            "agent_name".to_string(),
            AttributeValue::S("test-agent".to_string()),
        );
        item.insert("version".to_string(), AttributeValue::S("v1".to_string()));
        item.insert(
            "system_prompt".to_string(),
            AttributeValue::S("You are a helpful assistant.".to_string()),
        );
        item.insert(
            "llm_provider".to_string(),
            AttributeValue::S("claude".to_string()),
        );
        item.insert(
            "llm_model".to_string(),
            AttributeValue::S("claude-sonnet-4-20250514".to_string()),
        );
        item.insert(
            "parameters".to_string(),
            AttributeValue::S(
                serde_json::json!({
                    "max_iterations": 5,
                    "temperature": 0.3,
                    "max_tokens": 2048,
                    "timeout_seconds": 60
                })
                .to_string(),
            ),
        );
        item.insert(
            "mcp_servers".to_string(),
            AttributeValue::S(
                serde_json::json!([
                    "https://calc.example.com/mcp",
                    "https://wiki.example.com/mcp"
                ])
                .to_string(),
            ),
        );
        item
    }

    #[test]
    fn test_parse_agent_config_full() {
        let item = full_item();
        let config = parse_agent_config(&item).expect("should parse");

        assert_eq!(config.agent_name, "test-agent");
        assert_eq!(config.version, "v1");
        assert_eq!(config.system_prompt, "You are a helpful assistant.");
        assert_eq!(config.provider_config.provider_id, "anthropic");
        assert_eq!(config.provider_config.model_id, "claude-sonnet-4-20250514");
        assert_eq!(config.parameters.max_iterations, 5);
        assert!((config.parameters.temperature - 0.3).abs() < f32::EPSILON);
        assert_eq!(config.parameters.max_tokens, 2048);
        assert_eq!(config.parameters.timeout_seconds, 60);
        assert_eq!(config.mcp_server_urls.len(), 2);
        assert_eq!(config.mcp_server_urls[0], "https://calc.example.com/mcp");
    }

    #[test]
    fn test_parse_agent_config_missing_optional_fields() {
        let mut item = HashMap::new();
        item.insert(
            "agent_name".to_string(),
            AttributeValue::S("minimal-agent".to_string()),
        );
        item.insert("version".to_string(), AttributeValue::S("v1".to_string()));
        item.insert(
            "system_prompt".to_string(),
            AttributeValue::S("System prompt.".to_string()),
        );
        item.insert(
            "llm_provider".to_string(),
            AttributeValue::S("openai".to_string()),
        );
        item.insert(
            "llm_model".to_string(),
            AttributeValue::S("gpt-4o".to_string()),
        );
        // No parameters or mcp_servers -- should use defaults

        let config = parse_agent_config(&item).expect("should parse with defaults");

        assert_eq!(config.agent_name, "minimal-agent");
        assert_eq!(config.provider_config.provider_id, "openai");
        assert_eq!(config.provider_config.model_id, "gpt-4o");
        // Default parameters
        assert_eq!(config.parameters.max_iterations, 10);
        assert!((config.parameters.temperature - 0.7).abs() < f32::EPSILON);
        assert_eq!(config.parameters.max_tokens, 4096);
        assert_eq!(config.parameters.timeout_seconds, 120);
        // Default empty mcp_servers
        assert!(config.mcp_server_urls.is_empty());
    }

    #[test]
    fn test_parse_agent_config_missing_required_field() {
        let mut item = HashMap::new();
        item.insert(
            "agent_name".to_string(),
            AttributeValue::S("test".to_string()),
        );
        item.insert("version".to_string(), AttributeValue::S("v1".to_string()));
        // Missing system_prompt, llm_provider, llm_model

        let result = parse_agent_config(&item);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            ConfigError::MissingField(field) => assert_eq!(field, "system_prompt"),
            other => panic!("Expected MissingField, got: {other:?}"),
        }
    }

    #[test]
    fn test_parse_agent_config_invalid_json() {
        let mut item = full_item();
        // Overwrite parameters with invalid JSON
        item.insert(
            "parameters".to_string(),
            AttributeValue::S("not valid json{{{".to_string()),
        );

        // Invalid JSON in optional field falls back to default (not an error)
        let config = parse_agent_config(&item).expect("should fall back to defaults");
        assert_eq!(config.parameters.max_iterations, 10); // default
    }

    #[test]
    fn test_map_provider_config_anthropic() {
        let config =
            map_provider_config("anthropic", "claude-sonnet-4-20250514").expect("should map");
        assert_eq!(config.provider_id, "anthropic");
        assert_eq!(config.model_id, "claude-sonnet-4-20250514");
        assert_eq!(config.endpoint, "https://api.anthropic.com/v1/messages");
        assert_eq!(config.auth_header_name, "x-api-key");
        assert!(config.auth_header_prefix.is_none());
        assert_eq!(config.secret_path, "prod/anthropic/api-key");
        assert_eq!(config.secret_key_name, "api_key");
        assert_eq!(config.request_transformer, "anthropic_v1");
        assert_eq!(config.response_transformer, "anthropic_v1");
        assert_eq!(config.timeout, 120);
        let headers = config.custom_headers.expect("should have custom headers");
        assert_eq!(headers.get("anthropic-version").unwrap(), "2023-06-01");
    }

    #[test]
    fn test_map_provider_config_claude_alias() {
        let config = map_provider_config("claude", "claude-sonnet-4-20250514")
            .expect("should map claude alias");
        assert_eq!(config.provider_id, "anthropic");
    }

    #[test]
    fn test_map_provider_config_openai() {
        let config = map_provider_config("openai", "gpt-4o").expect("should map");
        assert_eq!(config.provider_id, "openai");
        assert_eq!(config.model_id, "gpt-4o");
        assert_eq!(
            config.endpoint,
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(config.auth_header_name, "Authorization");
        assert_eq!(config.auth_header_prefix.as_deref(), Some("Bearer "));
        assert_eq!(config.secret_path, "prod/openai/api-key");
        assert_eq!(config.request_transformer, "openai_v1");
        assert_eq!(config.response_transformer, "openai_v1");
        assert_eq!(config.timeout, 120);
        assert!(config.custom_headers.is_none());
    }

    #[test]
    fn test_map_provider_config_unsupported() {
        let result = map_provider_config("gemini", "gemini-pro");
        assert!(result.is_err());
        match result.unwrap_err() {
            ConfigError::UnsupportedProvider(name) => assert_eq!(name, "gemini"),
            other => panic!("Expected UnsupportedProvider, got: {other:?}"),
        }
    }

    #[test]
    fn test_config_serde_round_trip() {
        let item = full_item();
        let config = parse_agent_config(&item).expect("should parse");

        let serialized = serde_json::to_string(&config).expect("serialize");
        let deserialized: AgentConfig = serde_json::from_str(&serialized).expect("deserialize");

        assert_eq!(deserialized.agent_name, config.agent_name);
        assert_eq!(deserialized.version, config.version);
        assert_eq!(deserialized.system_prompt, config.system_prompt);
        assert_eq!(
            deserialized.provider_config.provider_id,
            config.provider_config.provider_id
        );
        assert_eq!(
            deserialized.provider_config.model_id,
            config.provider_config.model_id
        );
        assert_eq!(
            deserialized.parameters.max_iterations,
            config.parameters.max_iterations
        );
        assert_eq!(deserialized.mcp_server_urls, config.mcp_server_urls);
    }

    #[test]
    fn test_get_string_missing_key() {
        let item: HashMap<String, AttributeValue> = HashMap::new();
        let result = get_string(&item, "missing");
        assert!(result.is_err());
        match result.unwrap_err() {
            ConfigError::MissingField(f) => assert_eq!(f, "missing"),
            other => panic!("Expected MissingField, got: {other:?}"),
        }
    }

    #[test]
    fn test_get_json_string_as_valid() {
        let mut item = HashMap::new();
        item.insert(
            "data".to_string(),
            AttributeValue::S(r#"["a","b","c"]"#.to_string()),
        );
        let result: Vec<String> = get_json_string_as(&item, "data").expect("should parse");
        assert_eq!(result, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_get_json_string_as_invalid() {
        let mut item = HashMap::new();
        item.insert(
            "data".to_string(),
            AttributeValue::S("not json".to_string()),
        );
        let result: Result<Vec<String>, ConfigError> = get_json_string_as(&item, "data");
        assert!(result.is_err());
        match result.unwrap_err() {
            ConfigError::InvalidJson { field, .. } => assert_eq!(field, "data"),
            other => panic!("Expected InvalidJson, got: {other:?}"),
        }
    }
}
