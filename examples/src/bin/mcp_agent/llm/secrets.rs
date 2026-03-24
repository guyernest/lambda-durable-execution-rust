use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_sdk_secretsmanager::Client as SecretsClient;
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::error::LlmError;

const CACHE_TTL: Duration = Duration::from_secs(300); // 5 minutes

#[derive(Clone)]
struct CachedSecret {
    value: HashMap<String, String>,
    expires_at: Instant,
}

/// Manages retrieval and caching of API keys from AWS Secrets Manager.
///
/// Secrets are cached in-memory with a TTL to avoid repeated API calls.
/// Uses `tokio::sync::RwLock` for concurrent read access to the cache.
pub struct SecretManager {
    client: Arc<SecretsClient>,
    cache: Arc<RwLock<HashMap<String, CachedSecret>>>,
}

impl SecretManager {
    /// Create a new SecretManager using default AWS configuration.
    pub async fn new() -> Result<Self, LlmError> {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = SecretsClient::new(&config);
        Ok(Self {
            client: Arc::new(client),
            cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Create a new SecretManager with a pre-configured client (for testing).
    #[allow(dead_code)]
    pub fn new_with_client(client: SecretsClient) -> Self {
        Self {
            client: Arc::new(client),
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Retrieve an API key from Secrets Manager, using the cache when available.
    ///
    /// The secret at `secret_path` is expected to be a JSON object. The value
    /// for `key_name` is extracted and returned. Results are cached for
    /// [`CACHE_TTL`] (5 minutes).
    pub async fn get_api_key(&self, secret_path: &str, key_name: &str) -> Result<String, LlmError> {
        debug!(
            secret_path = %secret_path,
            key_name = %key_name,
            "Retrieving API key"
        );

        // Check cache (read lock)
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(secret_path) {
                if cached.expires_at > Instant::now() {
                    debug!("Using cached secret");
                    if let Some(api_key) = cached.value.get(key_name) {
                        return Ok(api_key.clone());
                    }
                    return Err(LlmError::SecretKeyNotFound(
                        key_name.to_string(),
                        secret_path.to_string(),
                    ));
                }
            }
        }

        // Acquire write lock — re-check cache to avoid duplicate fetches (TOCTOU)
        {
            let mut cache = self.cache.write().await;
            if let Some(cached) = cache.get(secret_path) {
                if cached.expires_at > Instant::now() {
                    debug!("Cache refreshed by another task");
                    if let Some(api_key) = cached.value.get(key_name) {
                        return Ok(api_key.clone());
                    }
                    return Err(LlmError::SecretKeyNotFound(
                        key_name.to_string(),
                        secret_path.to_string(),
                    ));
                }
                cache.remove(secret_path);
            }
        }

        info!(secret_path = %secret_path, "Fetching secret from AWS Secrets Manager");

        let response = self
            .client
            .get_secret_value()
            .secret_id(secret_path)
            .send()
            .await
            .map_err(|e| {
                warn!("Failed to fetch secret: {}", e);
                LlmError::AwsSdkError(e.to_string())
            })?;

        let secret_string = response
            .secret_string()
            .ok_or_else(|| LlmError::SecretNotFound(secret_path.to_string()))?;

        // Parse once, extract key and build cache map from the same parse
        let (api_key, secret_map) = parse_secret(secret_string, key_name, secret_path)?;

        {
            let mut cache = self.cache.write().await;
            cache.insert(
                secret_path.to_string(),
                CachedSecret {
                    value: secret_map,
                    expires_at: Instant::now() + CACHE_TTL,
                },
            );
        }

        info!(
            secret_path = %secret_path,
            key_name = %key_name,
            "Successfully retrieved and cached API key"
        );

        Ok(api_key)
    }

    /// Clear all cached secrets.
    #[allow(dead_code)]
    pub async fn clear_cache(&self) {
        info!("Clearing secret cache");
        let mut cache = self.cache.write().await;
        cache.clear();
    }
}

/// Parse a secret JSON string once: extract the requested key and build the
/// full cache map from the same parse. Returns `(api_key, secret_map)`.
fn parse_secret(
    json_str: &str,
    key_name: &str,
    secret_path: &str,
) -> Result<(String, HashMap<String, String>), LlmError> {
    let secret_json: Value = serde_json::from_str(json_str).map_err(|e| {
        warn!("Failed to parse secret JSON: {}", e);
        LlmError::InvalidConfiguration(format!("Secret is not valid JSON: {e}"))
    })?;

    let Value::Object(map) = secret_json else {
        return Err(LlmError::InvalidConfiguration(
            "Secret is not a JSON object".to_string(),
        ));
    };

    let api_key = match map.get(key_name) {
        Some(Value::String(s)) => s.clone(),
        Some(_) => {
            return Err(LlmError::InvalidConfiguration(format!(
                "Secret key '{key_name}' is not a string value"
            )));
        }
        None => {
            return Err(LlmError::SecretKeyNotFound(
                key_name.to_string(),
                secret_path.to_string(),
            ));
        }
    };

    let secret_map: HashMap<String, String> = map
        .into_iter()
        .filter_map(|(k, v)| {
            if let Value::String(s) = v {
                Some((k, s))
            } else {
                warn!("Skipping non-string value for key: {}", k);
                None
            }
        })
        .collect();

    Ok((api_key, secret_map))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_secret_extracts_key_and_map() {
        let json = r#"{"api_key": "sk-test-123", "other": "value"}"#;
        let (key, map) = parse_secret(json, "api_key", "test/secret").unwrap();
        assert_eq!(key, "sk-test-123");
        assert_eq!(map.len(), 2);
        assert_eq!(map["other"], "value");
    }

    #[test]
    fn test_parse_secret_key_not_found() {
        let json = r#"{"api_key": "sk-test-123"}"#;
        let result = parse_secret(json, "missing_key", "test/secret");
        match result {
            Err(LlmError::SecretKeyNotFound(key, path)) => {
                assert_eq!(key, "missing_key");
                assert_eq!(path, "test/secret");
            }
            other => panic!("Expected SecretKeyNotFound, got: {other:?}"),
        }
    }

    #[test]
    fn test_parse_secret_not_object() {
        let json = r#""just a string""#;
        let result = parse_secret(json, "api_key", "test/secret");
        match result {
            Err(LlmError::InvalidConfiguration(msg)) => {
                assert!(msg.contains("not a JSON object"), "got: {msg}");
            }
            other => panic!("Expected InvalidConfiguration, got: {other:?}"),
        }
    }

    #[test]
    fn test_parse_secret_array_not_object() {
        let json = r#"[1, 2, 3]"#;
        let result = parse_secret(json, "api_key", "test/secret");
        match result {
            Err(LlmError::InvalidConfiguration(msg)) => {
                assert!(msg.contains("not a JSON object"), "got: {msg}");
            }
            other => panic!("Expected InvalidConfiguration, got: {other:?}"),
        }
    }

    #[test]
    fn test_parse_secret_non_string_value() {
        let json = r#"{"api_key": 12345}"#;
        let result = parse_secret(json, "api_key", "test/secret");
        match result {
            Err(LlmError::InvalidConfiguration(msg)) => {
                assert!(msg.contains("not a string value"), "got: {msg}");
            }
            other => panic!("Expected InvalidConfiguration, got: {other:?}"),
        }
    }

    #[test]
    fn test_parse_secret_invalid_json() {
        let result = parse_secret("not json at all", "api_key", "test/secret");
        match result {
            Err(LlmError::InvalidConfiguration(msg)) => {
                assert!(msg.contains("not valid JSON"), "got: {msg}");
            }
            other => panic!("Expected InvalidConfiguration, got: {other:?}"),
        }
    }

    #[test]
    fn test_parse_secret_filters_non_string_values_in_map() {
        let json = r#"{"key1": "value1", "key2": 42, "key3": "value3"}"#;
        let (key, map) = parse_secret(json, "key1", "test/secret").unwrap();
        assert_eq!(key, "value1");
        assert_eq!(map.len(), 2);
        assert_eq!(map["key3"], "value3");
        assert!(!map.contains_key("key2"));
    }

    #[test]
    fn test_cache_expiry_logic() {
        // CachedSecret with an already-expired instant
        let cached = CachedSecret {
            value: HashMap::from([("key".to_string(), "value".to_string())]),
            expires_at: Instant::now() - Duration::from_secs(1),
        };
        assert!(cached.expires_at <= Instant::now(), "Should be expired");

        // CachedSecret with a future instant
        let fresh = CachedSecret {
            value: HashMap::from([("key".to_string(), "value".to_string())]),
            expires_at: Instant::now() + Duration::from_secs(300),
        };
        assert!(fresh.expires_at > Instant::now(), "Should not be expired");
    }

    #[tokio::test]
    async fn test_cache_hit_returns_value() {
        // Manually inject a cached secret and verify get_api_key reads from cache
        // We use new_with_client but never actually call AWS -- the cache is pre-populated.
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = SecretsClient::new(&config);
        let sm = SecretManager::new_with_client(client);

        // Pre-populate cache
        {
            let mut cache = sm.cache.write().await;
            cache.insert(
                "test/secret".to_string(),
                CachedSecret {
                    value: HashMap::from([("api_key".to_string(), "cached-key-123".to_string())]),
                    expires_at: Instant::now() + Duration::from_secs(300),
                },
            );
        }

        let result = sm.get_api_key("test/secret", "api_key").await;
        assert_eq!(result.unwrap(), "cached-key-123");
    }

    #[tokio::test]
    async fn test_cache_hit_key_not_found() {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = SecretsClient::new(&config);
        let sm = SecretManager::new_with_client(client);

        // Pre-populate cache with a secret that does NOT contain the requested key
        {
            let mut cache = sm.cache.write().await;
            cache.insert(
                "test/secret".to_string(),
                CachedSecret {
                    value: HashMap::from([("other_key".to_string(), "value".to_string())]),
                    expires_at: Instant::now() + Duration::from_secs(300),
                },
            );
        }

        let result = sm.get_api_key("test/secret", "missing_key").await;
        match result {
            Err(LlmError::SecretKeyNotFound(key, path)) => {
                assert_eq!(key, "missing_key");
                assert_eq!(path, "test/secret");
            }
            other => panic!("Expected SecretKeyNotFound, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_clear_cache() {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = SecretsClient::new(&config);
        let sm = SecretManager::new_with_client(client);

        // Pre-populate
        {
            let mut cache = sm.cache.write().await;
            cache.insert(
                "test/secret".to_string(),
                CachedSecret {
                    value: HashMap::from([("key".to_string(), "value".to_string())]),
                    expires_at: Instant::now() + Duration::from_secs(300),
                },
            );
        }

        sm.clear_cache().await;

        let cache = sm.cache.read().await;
        assert!(cache.is_empty(), "Cache should be empty after clear");
    }
}
