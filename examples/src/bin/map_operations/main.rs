// SPDX-License-Identifier: Apache-2.0
// Ported from aws-durable-execution-sdk-python-testing examples.
// Source: examples/src/map/map_operations.py
// See NOTICE for attribution.

//! Map fan-out (bounded concurrency) example.
//!
//! Demonstrates:
//! - `ctx.map()` to process a list of items with per-item durable steps.
//! - `MapConfig::with_max_concurrency()` to limit in-flight work.
//!
//! Event (JSON): `{}` (ignored)
//! Result (JSON): `[2,4,6,8,10]`
//!
//! Deployed via `examples/template.yaml`.

use lambda_durable_execution_rust::prelude::*;
use lambda_durable_execution_rust::runtime::with_durable_execution_service;
use tracing_subscriber::EnvFilter;

pub async fn handler(
    _event: serde_json::Value,
    ctx: DurableContextHandle,
) -> DurableResult<Vec<i32>> {
    let items = vec![1, 2, 3, 4, 5];

    let config: MapConfig<i32, i32> = MapConfig::new().with_max_concurrency(2);

    let batch = ctx
        .map(
            Some("map_operation"),
            items,
            |item, item_ctx, index| async move {
                let step_name = format!("map_item_{index}");
                item_ctx
                    .step(
                        Some(step_name.as_str()),
                        move |_step_ctx| async move { Ok(item * 2) },
                        None,
                    )
                    .await
            },
            Some(config),
        )
        .await?;

    Ok(batch.values())
}

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let svc = with_durable_execution_service(handler, None);
    lambda_runtime::run(svc).await
}
