//! Child context (“scoped workflow”) example.
//!
//! Demonstrates:
//! - `ctx.run_in_child_context()` to group a set of steps under a single parent operation.
//! - A simple sequential “batch processing” loop inside a child context.
//!
//! Event (JSON):
//! - `{ "items": ["a", "b", "c"] }`
//!
//! Result (JSON):
//! - `{ "successful": [...], "failed_count": 0, "processing_time_secs": 0 }`
//!
//! Deployed via `examples/template.yaml`.

use lambda_durable_execution_rust::prelude::*;
use lambda_durable_execution_rust::runtime::with_durable_execution_service;
use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;

/// Input event for the example.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BatchProcessEvent {
    /// Items to process.
    items: Vec<String>,
}

/// Result of processing a single item.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProcessedItem {
    original: String,
    result: String,
}

/// Response from the processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BatchProcessResponse {
    /// Successfully processed items.
    successful: Vec<ProcessedItem>,
    /// Number of failed items.
    failed_count: usize,
    /// Total processing time in seconds.
    processing_time_secs: u64,
}

/// Simulates processing a single item.
async fn process_item(
    item: &str,
) -> Result<ProcessedItem, Box<dyn std::error::Error + Send + Sync>> {
    // Simulate some async processing
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    Ok(ProcessedItem {
        original: item.to_string(),
        result: format!("processed_{}", item.to_uppercase()),
    })
}

/// Example using child contexts for grouped operations.
async fn child_context_handler(
    event: BatchProcessEvent,
    ctx: DurableContextHandle,
) -> DurableResult<BatchProcessResponse> {
    let start = std::time::Instant::now();
    let items = event.items.clone();

    // Use a child context to group related operations
    let results = ctx
        .run_in_child_context(
            Some("batch-processing-context"),
            move |child_ctx| {
                let items = items.clone();
                async move {
                    let mut successful = Vec::new();
                    let mut failed_count = 0;

                    for (i, item) in items.iter().enumerate() {
                        let item_clone = item.clone();
                        let step_name = format!("process-item-{}", i);

                        let result = child_ctx
                            .step(
                                Some(&step_name),
                                move |step_ctx| {
                                    let item = item_clone.clone();
                                    async move {
                                        step_ctx.info(&format!("Processing: {}", item));
                                        process_item(&item).await
                                    }
                                },
                                None,
                            )
                            .await;

                        match result {
                            Ok(processed) => successful.push(processed),
                            Err(_) => failed_count += 1,
                        }
                    }

                    Ok((successful, failed_count))
                }
            },
            None,
        )
        .await?;

    let elapsed = start.elapsed().as_secs();

    Ok(BatchProcessResponse {
        successful: results.0,
        failed_count: results.1,
        processing_time_secs: elapsed,
    })
}

/// Example with wait between steps.
#[allow(dead_code)]
async fn delayed_processing_handler(
    event: BatchProcessEvent,
    ctx: DurableContextHandle,
) -> DurableResult<BatchProcessResponse> {
    let start = std::time::Instant::now();
    let mut successful = Vec::new();
    let mut failed_count = 0;

    for (i, item) in event.items.iter().enumerate() {
        // Wait between items (no compute cost during wait)
        if i > 0 {
            ctx.wait(Some(&format!("delay-{}", i)), Duration::seconds(5))
                .await?;
        }

        let item_clone = item.clone();

        let result = ctx
            .step(
                Some(&format!("process-item-{}", i)),
                move |step_ctx| {
                    let item = item_clone.clone();
                    async move {
                        step_ctx.info(&format!("Processing: {}", item));
                        process_item(&item).await
                    }
                },
                None,
            )
            .await;

        match result {
            Ok(processed) => successful.push(processed),
            Err(_) => failed_count += 1,
        }
    }

    let elapsed = start.elapsed().as_secs();

    Ok(BatchProcessResponse {
        successful,
        failed_count,
        processing_time_secs: elapsed,
    })
}

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let handler = with_durable_execution_service(child_context_handler, None);
    lambda_runtime::run(handler).await
}
