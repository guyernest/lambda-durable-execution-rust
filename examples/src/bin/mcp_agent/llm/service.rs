use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::Client;
use serde_json::Value;
use tracing::{debug, error, info};

use super::error::LlmError;
use super::models::{LLMInvocation, LLMResponse, ProviderConfig, ResponseMetadata};
use super::secrets::SecretManager;
use super::transformers::TransformerRegistry;

/// Unified LLM service that orchestrates the full invocation pipeline:
/// secret retrieval, request transformation, HTTP call, and response parsing.
///
/// `Clone`-able so it can be moved into `ctx.step()` closures for durable
/// execution. All inner state is behind `Arc`, so clones are cheap.
#[derive(Clone)]
pub struct UnifiedLLMService {
    secret_manager: Arc<SecretManager>,
    http_client: Client,
    transformer_registry: Arc<TransformerRegistry>,
}

impl UnifiedLLMService {
    /// Create a new service with default AWS configuration and HTTP client.
    ///
    /// The HTTP client uses a 120-second timeout (agent LLM calls with tool
    /// use can be slow) and a 10-second connect timeout.
    pub async fn new() -> Result<Self, LlmError> {
        info!("Initializing UnifiedLLMService");

        let secret_manager = Arc::new(SecretManager::new().await?);

        let http_client = Client::builder()
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(10)
            .build()
            .map_err(LlmError::HttpError)?;

        let transformer_registry = Arc::new(TransformerRegistry::new());

        Ok(Self {
            secret_manager,
            http_client,
            transformer_registry,
        })
    }

    /// Create a service with pre-built components (for testing).
    pub fn new_with_components(
        secret_manager: Arc<SecretManager>,
        http_client: Client,
        transformer_registry: Arc<TransformerRegistry>,
    ) -> Self {
        Self {
            secret_manager,
            http_client,
            transformer_registry,
        }
    }

    /// Execute the full LLM invocation pipeline.
    ///
    /// Steps:
    /// 1. Retrieve API key from Secrets Manager (cached)
    /// 2. Look up request transformer
    /// 3. Transform request to provider format
    /// 4. Call provider HTTP endpoint
    /// 5. Look up response transformer
    /// 6. Transform response to unified format
    /// 7. Build `LLMResponse` with metadata
    pub async fn process(&self, invocation: LLMInvocation) -> Result<LLMResponse, LlmError> {
        let start = Instant::now();

        // 1. Get API key from Secrets Manager
        let api_key = self
            .secret_manager
            .get_api_key(
                &invocation.provider_config.secret_path,
                &invocation.provider_config.secret_key_name,
            )
            .await?;

        // 2. Get transformer for request
        let request_transformer = self
            .transformer_registry
            .get(&invocation.provider_config.request_transformer)?;

        // 3. Transform request to provider format
        debug!("Transforming request to provider format");
        let transformed_request = request_transformer.transform_request(&invocation)?;

        // 4. Make HTTP request to provider
        let response = self
            .call_provider(&invocation.provider_config, &transformed_request, &api_key)
            .await?;

        // 5. Get transformer for response
        let response_transformer = self
            .transformer_registry
            .get(&invocation.provider_config.response_transformer)?;

        // 6. Transform response to unified format
        debug!("Transforming response to unified format");
        let transformed_response = response_transformer.transform_response(response)?;

        // 7. Build final response with metadata
        let latency_ms = start.elapsed().as_millis() as u64;

        Ok(LLMResponse {
            message: transformed_response.message,
            function_calls: transformed_response.function_calls,
            metadata: ResponseMetadata {
                model_id: invocation.provider_config.model_id.clone(),
                provider_id: invocation.provider_config.provider_id.clone(),
                latency_ms,
                tokens_used: transformed_response.usage,
                stop_reason: transformed_response.stop_reason,
            },
        })
    }

    /// Make an HTTP request to the LLM provider.
    async fn call_provider(
        &self,
        config: &ProviderConfig,
        request: &super::models::TransformedRequest,
        api_key: &str,
    ) -> Result<Value, LlmError> {
        info!(
            provider = %config.provider_id,
            endpoint = %config.endpoint,
            "Making HTTP request to provider"
        );

        // Build request
        let mut req = self
            .http_client
            .post(&config.endpoint)
            .timeout(Duration::from_secs(config.timeout))
            .json(&request.body);

        // Add authentication header
        let auth_value = build_auth_value(config, api_key);
        req = req.header(&config.auth_header_name, &auth_value);

        // Add custom headers from config
        if let Some(custom_headers) = &config.custom_headers {
            for (key, value) in custom_headers {
                req = req.header(key, value);
            }
        }

        // Add request headers from transformer
        for (key, value) in &request.headers {
            req = req.header(key, value);
        }

        // Send request
        let response = req.send().await.map_err(|e| {
            error!("HTTP request failed: {}", e);
            LlmError::HttpError(e)
        })?;

        // Check status
        let status = response.status();
        if !status.is_success() {
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "No error body".to_string());
            error!(
                provider = %config.provider_id,
                status = %status,
                error = %error_body,
                "Provider API error"
            );
            return Err(LlmError::ProviderApiError {
                provider: config.provider_id.clone(),
                status: status.as_u16(),
                message: error_body,
            });
        }

        // Parse response
        let response_body = response.json::<Value>().await.map_err(|e| {
            error!("Failed to parse provider response: {}", e);
            LlmError::HttpError(e)
        })?;

        debug!(
            provider = %config.provider_id,
            "Successfully received response from provider"
        );

        Ok(response_body)
    }
}

/// Build the auth header value from provider config and API key.
fn build_auth_value(config: &ProviderConfig, api_key: &str) -> String {
    if let Some(prefix) = &config.auth_header_prefix {
        format!("{prefix}{api_key}")
    } else {
        api_key.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // Compile-time check: UnifiedLLMService must implement Clone
    fn assert_clone<T: Clone>() {}

    #[test]
    fn test_service_is_clone() {
        assert_clone::<UnifiedLLMService>();
    }

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

    #[test]
    fn test_auth_header_with_prefix() {
        let mut config = make_provider_config();
        config.auth_header_name = "Authorization".to_string();
        config.auth_header_prefix = Some("Bearer ".to_string());

        let value = build_auth_value(&config, "sk-xxx");
        assert_eq!(value, "Bearer sk-xxx");
    }

    #[test]
    fn test_auth_header_without_prefix() {
        let config = make_provider_config();
        // auth_header_prefix is None, auth_header_name is "x-api-key"

        let value = build_auth_value(&config, "sk-xxx");
        assert_eq!(value, "sk-xxx");
    }

    #[test]
    fn test_auth_header_empty_prefix() {
        let mut config = make_provider_config();
        config.auth_header_prefix = Some("".to_string());

        let value = build_auth_value(&config, "sk-xxx");
        assert_eq!(value, "sk-xxx");
    }

    #[test]
    fn test_provider_error_retryable_429() {
        let err = LlmError::ProviderApiError {
            provider: "anthropic".to_string(),
            status: 429,
            message: "rate limited".to_string(),
        };
        assert!(err.is_retryable());
    }

    #[test]
    fn test_provider_error_retryable_500() {
        let err = LlmError::ProviderApiError {
            provider: "openai".to_string(),
            status: 500,
            message: "internal error".to_string(),
        };
        assert!(err.is_retryable());
    }

    #[test]
    fn test_provider_error_non_retryable_400() {
        let err = LlmError::ProviderApiError {
            provider: "anthropic".to_string(),
            status: 400,
            message: "bad request".to_string(),
        };
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_provider_error_non_retryable_401() {
        let err = LlmError::ProviderApiError {
            provider: "openai".to_string(),
            status: 401,
            message: "unauthorized".to_string(),
        };
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_custom_headers_config() {
        let mut config = make_provider_config();
        let mut headers = HashMap::new();
        headers.insert("anthropic-version".to_string(), "2023-06-01".to_string());
        headers.insert("x-custom".to_string(), "value".to_string());
        config.custom_headers = Some(headers);

        assert_eq!(config.custom_headers.as_ref().unwrap().len(), 2);
        assert_eq!(
            config.custom_headers.as_ref().unwrap()["anthropic-version"],
            "2023-06-01"
        );
    }

    #[tokio::test]
    async fn test_call_provider_non_success_status() {
        // Use mockito-style test: start a local server that returns 400
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(400)
            .with_body(r#"{"error": "bad request"}"#)
            .create_async()
            .await;

        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let sm_client = aws_sdk_secretsmanager::Client::new(&config);
        let sm = Arc::new(SecretManager::new_with_client(sm_client));
        let http_client = Client::new();
        let registry = Arc::new(TransformerRegistry::new());
        let service = UnifiedLLMService::new_with_components(sm, http_client, registry);

        let mut provider_config = make_provider_config();
        provider_config.endpoint = format!("{}/v1/messages", server.url());

        let request = super::super::models::TransformedRequest {
            body: serde_json::json!({"model": "test"}),
            headers: HashMap::new(),
        };

        let result = service
            .call_provider(&provider_config, &request, "test-key")
            .await;

        match result {
            Err(LlmError::ProviderApiError {
                provider,
                status,
                message,
            }) => {
                assert_eq!(provider, "anthropic");
                assert_eq!(status, 400);
                assert!(message.contains("bad request"), "got: {message}");
            }
            other => panic!("Expected ProviderApiError, got: {other:?}"),
        }

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_call_provider_429_is_retryable() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(429)
            .with_body(r#"{"error": "rate limited"}"#)
            .create_async()
            .await;

        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let sm_client = aws_sdk_secretsmanager::Client::new(&config);
        let sm = Arc::new(SecretManager::new_with_client(sm_client));
        let http_client = Client::new();
        let registry = Arc::new(TransformerRegistry::new());
        let service = UnifiedLLMService::new_with_components(sm, http_client, registry);

        let mut provider_config = make_provider_config();
        provider_config.endpoint = format!("{}/v1/messages", server.url());

        let request = super::super::models::TransformedRequest {
            body: serde_json::json!({"model": "test"}),
            headers: HashMap::new(),
        };

        let result = service
            .call_provider(&provider_config, &request, "test-key")
            .await;

        match &result {
            Err(e) => assert!(e.is_retryable(), "429 should be retryable"),
            Ok(_) => panic!("Expected error, got success"),
        }

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_call_provider_success() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id": "msg_123", "content": [{"type": "text", "text": "Hello"}]}"#)
            .create_async()
            .await;

        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let sm_client = aws_sdk_secretsmanager::Client::new(&config);
        let sm = Arc::new(SecretManager::new_with_client(sm_client));
        let http_client = Client::new();
        let registry = Arc::new(TransformerRegistry::new());
        let service = UnifiedLLMService::new_with_components(sm, http_client, registry);

        let mut provider_config = make_provider_config();
        provider_config.endpoint = format!("{}/v1/messages", server.url());

        let request = super::super::models::TransformedRequest {
            body: serde_json::json!({"model": "test"}),
            headers: HashMap::new(),
        };

        let result = service
            .call_provider(&provider_config, &request, "test-key")
            .await;

        assert!(result.is_ok(), "Expected success, got: {result:?}");
        let body = result.unwrap();
        assert_eq!(body["id"], "msg_123");

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_call_provider_sends_auth_header() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .match_header("x-api-key", "sk-test-key")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok": true}"#)
            .create_async()
            .await;

        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let sm_client = aws_sdk_secretsmanager::Client::new(&config);
        let sm = Arc::new(SecretManager::new_with_client(sm_client));
        let http_client = Client::new();
        let registry = Arc::new(TransformerRegistry::new());
        let service = UnifiedLLMService::new_with_components(sm, http_client, registry);

        let mut provider_config = make_provider_config();
        provider_config.endpoint = format!("{}/v1/messages", server.url());

        let request = super::super::models::TransformedRequest {
            body: serde_json::json!({}),
            headers: HashMap::new(),
        };

        let _ = service
            .call_provider(&provider_config, &request, "sk-test-key")
            .await;

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_call_provider_sends_bearer_auth_header() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .match_header("Authorization", "Bearer sk-openai-key")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok": true}"#)
            .create_async()
            .await;

        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let sm_client = aws_sdk_secretsmanager::Client::new(&config);
        let sm = Arc::new(SecretManager::new_with_client(sm_client));
        let http_client = Client::new();
        let registry = Arc::new(TransformerRegistry::new());
        let service = UnifiedLLMService::new_with_components(sm, http_client, registry);

        let mut provider_config = make_provider_config();
        provider_config.endpoint = format!("{}/v1/chat/completions", server.url());
        provider_config.auth_header_name = "Authorization".to_string();
        provider_config.auth_header_prefix = Some("Bearer ".to_string());

        let request = super::super::models::TransformedRequest {
            body: serde_json::json!({}),
            headers: HashMap::new(),
        };

        let _ = service
            .call_provider(&provider_config, &request, "sk-openai-key")
            .await;

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_call_provider_sends_custom_headers() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .match_header("x-custom-header", "custom-value")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok": true}"#)
            .create_async()
            .await;

        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let sm_client = aws_sdk_secretsmanager::Client::new(&config);
        let sm = Arc::new(SecretManager::new_with_client(sm_client));
        let http_client = Client::new();
        let registry = Arc::new(TransformerRegistry::new());
        let service = UnifiedLLMService::new_with_components(sm, http_client, registry);

        let mut provider_config = make_provider_config();
        provider_config.endpoint = format!("{}/v1/messages", server.url());
        let mut custom = HashMap::new();
        custom.insert("x-custom-header".to_string(), "custom-value".to_string());
        provider_config.custom_headers = Some(custom);

        let request = super::super::models::TransformedRequest {
            body: serde_json::json!({}),
            headers: HashMap::new(),
        };

        let _ = service
            .call_provider(&provider_config, &request, "test-key")
            .await;

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_call_provider_sends_transformer_headers() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .match_header("anthropic-version", "2023-06-01")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok": true}"#)
            .create_async()
            .await;

        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let sm_client = aws_sdk_secretsmanager::Client::new(&config);
        let sm = Arc::new(SecretManager::new_with_client(sm_client));
        let http_client = Client::new();
        let registry = Arc::new(TransformerRegistry::new());
        let service = UnifiedLLMService::new_with_components(sm, http_client, registry);

        let mut provider_config = make_provider_config();
        provider_config.endpoint = format!("{}/v1/messages", server.url());

        let mut transformer_headers = HashMap::new();
        transformer_headers.insert("anthropic-version".to_string(), "2023-06-01".to_string());

        let request = super::super::models::TransformedRequest {
            body: serde_json::json!({}),
            headers: transformer_headers,
        };

        let _ = service
            .call_provider(&provider_config, &request, "test-key")
            .await;

        mock.assert_async().await;
    }
}
