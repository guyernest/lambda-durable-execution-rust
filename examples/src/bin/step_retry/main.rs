// SPDX-License-Identifier: Apache-2.0
// Ported from aws-durable-execution-sdk-python-testing examples.
// Source: examples/src/step/steps_with_retry.py
// See NOTICE for attribution.

//! Step retries (exponential backoff) workflow.
//!
//! Demonstrates:
//! - `StepConfig` + `ExponentialBackoff` retry strategy for transient failures.
//! - Durable retries: retry decisions and scheduled delays are checkpointed.
//!
//! Event (JSON):
//! - `{ "url": "https://example.com", "max_retries": 3 }`
//!
//! Result (JSON):
//! - `{ "data": "Data from https://example.com" }`
//!
//! Deployed via `examples/template.yaml`.

use lambda_durable_execution_rust::prelude::*;
use lambda_durable_execution_rust::retry::{ExponentialBackoff, JitterStrategy};
use lambda_durable_execution_rust::runtime::with_durable_execution_service;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

/// Input event for the retry example.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryEvent {
    /// URL to fetch data from.
    pub url: String,
    /// Maximum number of retry attempts.
    pub max_retries: u32,
}

/// Response from the retry example.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryResponse {
    /// The fetched data.
    pub data: String,
}

/// Simulated HTTP client error.
#[derive(Debug)]
pub struct HttpError {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {} - {}", self.status, self.message)
    }
}

impl std::error::Error for HttpError {}

/// Simulates fetching data from a URL (may fail transiently).
pub async fn fetch_data(url: &str) -> Result<String, HttpError> {
    // In a real application, this would make an actual HTTP request
    // Simulate network latency
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    Ok(format!("Data from {}", url))
}

/// The durable handler with retry configuration.
pub async fn retry_handler(
    event: RetryEvent,
    ctx: DurableContextHandle,
) -> DurableResult<RetryResponse> {
    // Clone URL before the closure
    let url = event.url.clone();

    // Configure exponential backoff retry strategy
    let retry_strategy = ExponentialBackoff::builder()
        .max_attempts(event.max_retries)
        .initial_delay(Duration::seconds(1))
        .max_delay(Duration::seconds(30))
        .backoff_rate(2.0)
        .jitter(JitterStrategy::Full)
        // Only retry on timeout or connection errors
        .retryable_pattern("timeout")
        .retryable_pattern("connection")
        .build();

    // Create step config with retry strategy
    let step_config = StepConfig::<String>::new().with_retry_strategy(Arc::new(retry_strategy));

    // Execute the step with retry logic
    let data = ctx
        .step(
            Some("fetch-data"),
            move |step_ctx| {
                let url = url.clone();
                async move {
                    step_ctx.info(&format!("Fetching data from {}", url));

                    fetch_data(&url)
                        .await
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                }
            },
            Some(step_config),
        )
        .await?;

    Ok(RetryResponse { data })
}

/// Example of using preset retry strategies.
#[allow(dead_code)]
pub async fn preset_retry_handler(
    event: RetryEvent,
    ctx: DurableContextHandle,
) -> DurableResult<RetryResponse> {
    // Clone URL before the closure
    let url = event.url.clone();

    // Use a preset retry strategy from the presets module
    let step_config = StepConfig::<String>::new().with_retry_strategy(retry_presets::default());

    let data = ctx
        .step(
            Some("fetch-with-preset"),
            move |_step_ctx| {
                let url = url.clone();
                async move {
                    fetch_data(&url)
                        .await
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                }
            },
            Some(step_config),
        )
        .await?;

    Ok(RetryResponse { data })
}

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let handler = with_durable_execution_service(retry_handler, None);
    lambda_runtime::run(handler).await
}
