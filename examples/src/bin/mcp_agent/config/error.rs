use thiserror::Error;

/// Errors from the configuration loading module.
///
/// Covers DynamoDB access failures, missing/malformed fields, and
/// unsupported provider mappings.
#[derive(Error, Debug)]
pub enum ConfigError {
    /// The requested agent was not found in the AgentRegistry table.
    #[error("Agent not found: {agent_name} version {version}")]
    AgentNotFound {
        /// Agent name (partition key).
        agent_name: String,
        /// Version (sort key).
        version: String,
    },

    /// A required field is missing from the DynamoDB item.
    #[error("Missing required field: {0}")]
    MissingField(String),

    /// A JSON-encoded field could not be deserialized.
    #[error("Invalid JSON in field {field}: {source}")]
    InvalidJson {
        /// The DynamoDB field name.
        field: String,
        /// The underlying serde_json error.
        #[source]
        source: serde_json::Error,
    },

    /// The `llm_provider` value does not map to a known provider.
    #[error("Unsupported LLM provider: {0}")]
    UnsupportedProvider(String),

    /// A URL field contains an invalid URL.
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    /// An error from the DynamoDB SDK.
    #[error("DynamoDB error: {0}")]
    DynamoDbError(String),
}
