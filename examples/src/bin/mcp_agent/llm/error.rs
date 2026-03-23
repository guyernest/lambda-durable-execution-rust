use thiserror::Error;

/// Errors from the LLM client module.
///
/// Classifies errors as retryable or non-retryable for integration with
/// durable step retry strategies.
#[derive(Error, Debug)]
pub enum LlmError {
    /// The requested transformer was not found in the registry.
    #[error("Transformer not found: {0}")]
    TransformerNotFound(String),

    /// The specified secret was not found in Secrets Manager.
    #[error("Secret not found: {0}")]
    SecretNotFound(String),

    /// The specified key was not found within a secret.
    #[error("Secret key not found: {0} in secret: {1}")]
    SecretKeyNotFound(String, String),

    /// An AWS SDK error (stored as string to avoid tight coupling).
    #[error("AWS SDK error: {0}")]
    AwsSdkError(String),

    /// An HTTP transport error (connection, DNS, TLS, etc.).
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),

    /// A JSON serialization/deserialization error.
    #[error("JSON serialization error: {0}")]
    JsonError(#[from] serde_json::Error),

    /// The LLM provider returned a non-2xx HTTP status.
    #[error("Provider API error: {provider} returned {status}: {message}")]
    ProviderApiError {
        /// Provider identifier.
        provider: String,
        /// HTTP status code.
        status: u16,
        /// Error message from the provider.
        message: String,
    },

    /// Invalid or missing configuration.
    #[error("Invalid configuration: {0}")]
    InvalidConfiguration(String),

    /// An error during request/response transformation.
    #[error("Transform error: {0}")]
    TransformError(String),

    /// The request timed out after the specified number of seconds.
    #[error("Timeout after {0} seconds")]
    Timeout(u64),

    /// An unclassified error.
    #[error("Unknown error: {0}")]
    Unknown(String),
}

impl LlmError {
    /// Returns `true` if this error is transient and the operation should be retried.
    ///
    /// Retryable status codes: 429 (rate limit), 500 (internal server error),
    /// 502 (bad gateway), 503 (service unavailable), 529 (overloaded).
    pub fn is_retryable(&self) -> bool {
        match self {
            LlmError::ProviderApiError { status, .. } => {
                matches!(status, 429 | 500 | 502 | 503 | 529)
            }
            LlmError::HttpError(_) => true,
            LlmError::Timeout(_) => true,
            _ => false,
        }
    }
}

// Note: LlmError already implements std::error::Error via thiserror,
// so the blanket impl `From<E: Error> for Box<dyn Error + Send + Sync>`
// handles the conversion automatically. No manual From impl needed.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_api_error_429_is_retryable() {
        let err = LlmError::ProviderApiError {
            provider: "anthropic".to_string(),
            status: 429,
            message: "rate limited".to_string(),
        };
        assert!(err.is_retryable());
    }

    #[test]
    fn test_provider_api_error_400_is_not_retryable() {
        let err = LlmError::ProviderApiError {
            provider: "anthropic".to_string(),
            status: 400,
            message: "bad request".to_string(),
        };
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_provider_api_error_529_is_retryable() {
        let err = LlmError::ProviderApiError {
            provider: "anthropic".to_string(),
            status: 529,
            message: "overloaded".to_string(),
        };
        assert!(err.is_retryable());
    }

    #[test]
    fn test_provider_api_error_500_is_retryable() {
        let err = LlmError::ProviderApiError {
            provider: "anthropic".to_string(),
            status: 500,
            message: "internal server error".to_string(),
        };
        assert!(err.is_retryable());
    }

    #[test]
    fn test_provider_api_error_502_is_retryable() {
        let err = LlmError::ProviderApiError {
            provider: "anthropic".to_string(),
            status: 502,
            message: "bad gateway".to_string(),
        };
        assert!(err.is_retryable());
    }

    #[test]
    fn test_provider_api_error_503_is_retryable() {
        let err = LlmError::ProviderApiError {
            provider: "anthropic".to_string(),
            status: 503,
            message: "service unavailable".to_string(),
        };
        assert!(err.is_retryable());
    }

    #[test]
    fn test_provider_api_error_401_is_not_retryable() {
        let err = LlmError::ProviderApiError {
            provider: "anthropic".to_string(),
            status: 401,
            message: "unauthorized".to_string(),
        };
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_provider_api_error_403_is_not_retryable() {
        let err = LlmError::ProviderApiError {
            provider: "anthropic".to_string(),
            status: 403,
            message: "forbidden".to_string(),
        };
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn test_http_error_is_retryable() {
        // Create a real reqwest::Error by requesting an invalid URL
        let client = reqwest::Client::new();
        let reqwest_err = client
            .get("http://[::0]:1/invalid")
            .send()
            .await
            .unwrap_err();
        let err = LlmError::HttpError(reqwest_err);
        assert!(err.is_retryable());
    }

    #[test]
    fn test_timeout_is_retryable() {
        let err = LlmError::Timeout(30);
        assert!(err.is_retryable());
    }

    #[test]
    fn test_json_error_is_not_retryable() {
        let json_err: serde_json::Error = serde_json::from_str::<String>("invalid").unwrap_err();
        let err = LlmError::JsonError(json_err);
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_transformer_not_found_is_not_retryable() {
        let err = LlmError::TransformerNotFound("unknown_v1".to_string());
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_invalid_configuration_is_not_retryable() {
        let err = LlmError::InvalidConfiguration("missing field".to_string());
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_error_converts_to_boxed_error() {
        let err = LlmError::Unknown("test".to_string());
        let boxed: Box<dyn std::error::Error + Send + Sync> = err.into();
        assert!(boxed.to_string().contains("test"));
    }
}
