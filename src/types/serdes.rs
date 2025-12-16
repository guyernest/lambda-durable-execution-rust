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
