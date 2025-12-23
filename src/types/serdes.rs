//! Serialization/Deserialization (Serdes) support.
//!
//! Mirrors the JS SDK Serdes interface, allowing users to customize how durable
//! operation payloads are stored and restored.

use crate::error::BoxError;
use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};

/// Context passed to Serdes implementations.
#[derive(Debug, Clone)]
pub struct SerdesContext {
    /// Unique identifier for the durable entity (hashed operation id).
    pub entity_id: String,
    /// ARN of the durable execution.
    pub durable_execution_arn: String,
}

/// Serdes (Serialization/Deserialization) interface.
///
/// Both methods are async to allow implementations that interact with external storage.
#[async_trait]
pub trait Serdes<T>: Send + Sync {
    /// Serialize a value to an optional string.
    async fn serialize(
        &self,
        value: Option<&T>,
        context: SerdesContext,
    ) -> Result<Option<String>, BoxError>;

    /// Deserialize an optional string to a value.
    async fn deserialize(
        &self,
        data: Option<&str>,
        context: SerdesContext,
    ) -> Result<Option<T>, BoxError>;
}

/// Default JSON Serdes implementation using `serde_json`.
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonSerdes;

#[async_trait]
impl<T> Serdes<T> for JsonSerdes
where
    T: Serialize + DeserializeOwned + Send + Sync,
{
    async fn serialize(
        &self,
        value: Option<&T>,
        _context: SerdesContext,
    ) -> Result<Option<String>, BoxError> {
        if let Some(v) = value {
            Ok(Some(serde_json::to_string(v)?))
        } else {
            Ok(None)
        }
    }

    async fn deserialize(
        &self,
        data: Option<&str>,
        _context: SerdesContext,
    ) -> Result<Option<T>, BoxError> {
        if let Some(d) = data {
            Ok(Some(serde_json::from_str(d)?))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Sample {
        value: u32,
    }

    fn context() -> SerdesContext {
        SerdesContext {
            entity_id: "entity-1".to_string(),
            durable_execution_arn: "arn:aws:lambda:us-east-1:123:function:durable".to_string(),
        }
    }

    #[tokio::test]
    async fn test_json_serdes_roundtrip() {
        let serdes = JsonSerdes;
        let value = Sample { value: 42 };

        let encoded = serdes
            .serialize(Some(&value), context())
            .await
            .expect("serialize");
        let decoded: Option<Sample> = serdes
            .deserialize(encoded.as_deref(), context())
            .await
            .expect("deserialize");

        assert_eq!(decoded, Some(value));
    }

    #[tokio::test]
    async fn test_json_serdes_none_passthrough() {
        let serdes = JsonSerdes;

        let encoded = serdes.serialize(None::<&Sample>, context()).await.unwrap();
        assert!(encoded.is_none());

        let decoded: Option<Sample> = serdes.deserialize(None, context()).await.unwrap();
        assert!(decoded.is_none());
    }
}
