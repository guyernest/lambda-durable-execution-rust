use crate::error::{BoxError, DurableError};
use crate::types::{
    BatchCompletionReason, BatchItem, BatchItemStatus, BatchResult, Serdes, SerdesContext,
};
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const BATCH_RESULT_KIND: &str = "BatchResult";

#[derive(Debug, Clone, Copy, Default)]
/// Default Serdes for [`BatchResult<T>`](crate::types::BatchResult).
///
/// This Serdes stores a JSON payload that:
/// - can round-trip batch item results
/// - preserves per-item status (SUCCEEDED/FAILED/STARTED)
/// - stores errors as a message string (not a structured error type)
pub struct BatchResultSerdes;

impl BatchResultSerdes {
    /// Returns `true` if `payload` looks like a `BatchResultSerdes` JSON payload.
    ///
    /// This is intended for differentiating full batch payloads from legacy
    /// summary payloads (for example, `"type": "MapResult"`).
    pub fn is_batch_result_payload(payload: &str) -> bool {
        serde_json::from_str::<serde_json::Value>(payload)
            .ok()
            .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(str::to_owned))
            .as_deref()
            == Some(BATCH_RESULT_KIND)
    }
}

#[derive(Debug, Serialize)]
struct BatchItemPayload<'a, T> {
    index: usize,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<&'a T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct BatchResultPayload<'a, T> {
    kind: &'static str,
    completion_reason: &'static str,
    items: Vec<BatchItemPayload<'a, T>>,
}

#[derive(Debug, Deserialize)]
struct BatchItemPayloadOwned<T> {
    index: usize,
    status: String,
    result: Option<T>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BatchResultPayloadOwned<T> {
    kind: String,
    completion_reason: String,
    items: Vec<BatchItemPayloadOwned<T>>,
}

fn item_status_str(status: BatchItemStatus) -> &'static str {
    match status {
        BatchItemStatus::Succeeded => "SUCCEEDED",
        BatchItemStatus::Failed => "FAILED",
        BatchItemStatus::Started => "STARTED",
    }
}

fn parse_item_status(status: &str) -> Result<BatchItemStatus, BoxError> {
    match status {
        "SUCCEEDED" => Ok(BatchItemStatus::Succeeded),
        "FAILED" => Ok(BatchItemStatus::Failed),
        "STARTED" => Ok(BatchItemStatus::Started),
        other => Err(format!("Invalid batch item status: {other}").into()),
    }
}

fn completion_reason_str(reason: BatchCompletionReason) -> &'static str {
    match reason {
        BatchCompletionReason::AllCompleted => "ALL_COMPLETED",
        BatchCompletionReason::MinSuccessfulReached => "MIN_SUCCESSFUL_REACHED",
        BatchCompletionReason::FailureToleranceExceeded => "FAILURE_TOLERANCE_EXCEEDED",
    }
}

fn parse_completion_reason(reason: &str) -> Result<BatchCompletionReason, BoxError> {
    match reason {
        "ALL_COMPLETED" => Ok(BatchCompletionReason::AllCompleted),
        "MIN_SUCCESSFUL_REACHED" => Ok(BatchCompletionReason::MinSuccessfulReached),
        "FAILURE_TOLERANCE_EXCEEDED" => Ok(BatchCompletionReason::FailureToleranceExceeded),
        other => Err(format!("Invalid batch completion reason: {other}").into()),
    }
}

#[async_trait]
impl<T> Serdes<BatchResult<T>> for BatchResultSerdes
where
    T: Serialize + DeserializeOwned + Send + Sync,
{
    async fn serialize(
        &self,
        value: Option<&BatchResult<T>>,
        _context: SerdesContext,
    ) -> Result<Option<String>, BoxError> {
        let Some(batch) = value else {
            return Ok(None);
        };

        let items: Vec<_> = batch
            .all
            .iter()
            .map(|item| BatchItemPayload {
                index: item.index,
                status: item_status_str(item.status),
                result: item.result.as_ref(),
                error: item.error.as_ref().map(|err| match err.as_ref() {
                    DurableError::Internal(message) => message.clone(),
                    other => other.to_string(),
                }),
            })
            .collect();

        let payload = BatchResultPayload {
            kind: BATCH_RESULT_KIND,
            completion_reason: completion_reason_str(batch.completion_reason),
            items,
        };

        Ok(Some(serde_json::to_string(&payload)?))
    }

    async fn deserialize(
        &self,
        data: Option<&str>,
        _context: SerdesContext,
    ) -> Result<Option<BatchResult<T>>, BoxError> {
        let Some(data) = data else {
            return Ok(None);
        };

        let payload: BatchResultPayloadOwned<T> = serde_json::from_str(data)?;
        if payload.kind != BATCH_RESULT_KIND {
            return Err(format!("Unexpected batch payload kind: {}", payload.kind).into());
        }

        let completion_reason = parse_completion_reason(&payload.completion_reason)?;
        let mut all = Vec::new();
        for item in payload.items {
            let status = parse_item_status(&item.status)?;
            let error = match status {
                BatchItemStatus::Failed => Some(Arc::new(DurableError::Internal(
                    item.error
                        .unwrap_or_else(|| "Batch item failed".to_string()),
                ))),
                _ => None,
            };
            all.push(BatchItem {
                index: item.index,
                status,
                result: item.result,
                error,
            });
        }
        all.sort_by_key(|i| i.index);

        Ok(Some(BatchResult {
            all,
            completion_reason,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> SerdesContext {
        SerdesContext {
            entity_id: "entity-1".to_string(),
            durable_execution_arn: "arn:test:durable".to_string(),
        }
    }

    #[tokio::test]
    async fn test_batch_result_serdes_roundtrip() {
        let serdes = BatchResultSerdes;
        let batch = BatchResult {
            all: vec![
                BatchItem {
                    index: 0,
                    status: BatchItemStatus::Succeeded,
                    result: Some(10u32),
                    error: None,
                },
                BatchItem {
                    index: 1,
                    status: BatchItemStatus::Failed,
                    result: None,
                    error: Some(Arc::new(DurableError::Internal("boom".to_string()))),
                },
                BatchItem {
                    index: 2,
                    status: BatchItemStatus::Started,
                    result: None,
                    error: None,
                },
            ],
            completion_reason: BatchCompletionReason::FailureToleranceExceeded,
        };

        let encoded = serdes
            .serialize(Some(&batch), context())
            .await
            .expect("serialize")
            .expect("payload");
        assert!(BatchResultSerdes::is_batch_result_payload(&encoded));

        let decoded: BatchResult<u32> = serdes
            .deserialize(Some(&encoded), context())
            .await
            .expect("deserialize")
            .expect("value");

        assert_eq!(decoded.completion_reason, batch.completion_reason);
        assert_eq!(decoded.all.len(), batch.all.len());
        assert_eq!(decoded.all[0].index, 0);
        assert_eq!(decoded.all[0].status, BatchItemStatus::Succeeded);
        assert_eq!(decoded.all[0].result, Some(10));
        assert_eq!(decoded.all[1].status, BatchItemStatus::Failed);
        match decoded.all[1].error.as_deref() {
            Some(DurableError::Internal(message)) => assert_eq!(message, "boom"),
            Some(other) => panic!("unexpected error: {other:?}"),
            None => panic!("missing error"),
        }
        assert_eq!(decoded.all[2].status, BatchItemStatus::Started);
    }
}
