// SPDX-License-Identifier: Apache-2.0
// Ported from aws-durable-execution-sdk-python-testing examples.
// Source: examples/src/map/map_with_failure_tolerance.py
// See NOTICE for attribution.

//! Map fan-out with tolerated failures example.
//!
//! Demonstrates:
//! - `ctx.map()` with `CompletionConfig::with_tolerated_failures(3)` to keep going despite some failures.
//! - Turning retries off for the failing step so failures surface immediately.
//!
//! Event (JSON): `{}` (ignored)
//! Result (JSON): includes `completionReason`, `successful`, and `failed` item summaries.
//!
//! Deployed via `examples/template.yaml`.

use lambda_durable_execution_rust::prelude::*;
use lambda_durable_execution_rust::runtime::with_durable_execution_service;
use lambda_durable_execution_rust::types::CompletionConfig;
use serde_json::json;
use tracing_subscriber::EnvFilter;

pub async fn handler(
    _event: serde_json::Value,
    ctx: DurableContextHandle,
) -> DurableResult<serde_json::Value> {
    let items: Vec<i32> = (1..=10).collect();

    let completion = CompletionConfig::new().with_tolerated_failures(3);
    let config: MapConfig<i32, i32> = MapConfig::new()
        .with_max_concurrency(5)
        .with_completion_config(completion);

    // Disable retries so failures surface immediately.
    let retry_strategy = retry_presets::none();

    let batch = ctx
        .map(
            Some("map_with_tolerance"),
            items,
            move |item, item_ctx, index| {
                let retry_strategy = Arc::clone(&retry_strategy);
                async move {
                    let step_config: StepConfig<i32> =
                        StepConfig::new().with_retry_strategy(retry_strategy);

                    let step_name = format!("item_{index}");
                    item_ctx
                        .step(
                            Some(step_name.as_str()),
                            move |_step_ctx| async move {
                                if item % 3 == 0 {
                                    let err: Box<dyn std::error::Error + Send + Sync> =
                                        Box::new(std::io::Error::new(
                                            std::io::ErrorKind::Other,
                                            format!("Item {item} failed"),
                                        ));
                                    return Err(err);
                                }
                                Ok(item * 2)
                            },
                            Some(step_config),
                        )
                        .await
                }
            },
            Some(config),
        )
        .await?;

    let completion_reason = match batch.completion_reason {
        BatchCompletionReason::AllCompleted => "ALL_COMPLETED",
        BatchCompletionReason::MinSuccessfulReached => "MIN_SUCCESSFUL_REACHED",
        BatchCompletionReason::FailureToleranceExceeded => "FAILURE_TOLERANCE_EXCEEDED",
    };

    let succeeded: Vec<i32> = batch.all.iter().filter_map(|i| i.result).collect();

    Ok(json!({
        "success_count": batch.success_count(),
        "failure_count": batch.failure_count(),
        "succeeded": succeeded,
        "failed_count": batch.failed().len(),
        "completion_reason": completion_reason,
    }))
}

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let svc = with_durable_execution_service(handler, None);
    lambda_runtime::run(svc).await
}
