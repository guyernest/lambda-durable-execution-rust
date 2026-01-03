// SPDX-License-Identifier: Apache-2.0
// Ported from aws-durable-execution-sdk-python-testing examples.
// Source: examples/src/map/map_with_custom_serdes.py
// See NOTICE for attribution.

//! Map fan-out with custom Serdes example.
//!
//! Demonstrates:
//! - Supplying an `item_serdes` to `ctx.map()` so each item result is serialized/deserialized with custom logic.
//! - Returning a JSON summary of per-item processing.
//!
//! Event (JSON): `{}` (ignored)
//! Result (JSON): `{ "values": [...], ... }` (shape depends on the batch wrapper)
//!
//! Deployed via `examples/template.yaml`.

use async_trait::async_trait;
use lambda_durable_execution_rust::prelude::*;
use lambda_durable_execution_rust::runtime::with_durable_execution_service;
use serde_json::json;
use tracing_subscriber::EnvFilter;

#[derive(Debug)]
struct CustomItemSerdes;

#[async_trait]
impl Serdes<serde_json::Value> for CustomItemSerdes {
    async fn serialize(
        &self,
        value: Option<&serde_json::Value>,
        _context: SerdesContext,
    ) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
        let Some(value) = value else {
            return Ok(None);
        };

        let wrapped = json!({
            "data": value,
            "serialized_by": "CustomItemSerdes",
            "version": "1.0",
        });

        Ok(Some(serde_json::to_string(&wrapped)?))
    }

    async fn deserialize(
        &self,
        data: Option<&str>,
        _context: SerdesContext,
    ) -> Result<Option<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>> {
        let Some(data) = data else {
            return Ok(None);
        };

        let wrapped: serde_json::Value = serde_json::from_str(data)?;
        Ok(wrapped.get("data").cloned())
    }
}

pub async fn handler(
    _event: serde_json::Value,
    ctx: DurableContextHandle,
) -> DurableResult<serde_json::Value> {
    let items = vec![
        json!({ "id": 1, "name": "item1" }),
        json!({ "id": 2, "name": "item2" }),
        json!({ "id": 3, "name": "item3" }),
    ];

    let config: MapConfig<serde_json::Value, serde_json::Value> =
        MapConfig::new().with_item_serdes(Arc::new(CustomItemSerdes));

    let batch = ctx
        .map(
            Some("map_with_custom_serdes"),
            items,
            |item, item_ctx, index| async move {
                let step_name = format!("process_{index}");
                item_ctx
                    .step(
                        Some(step_name.as_str()),
                        move |_step_ctx| async move {
                            let id = item.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
                            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            Ok(json!({
                                "processed": name,
                                "index": index,
                                "doubled_id": id * 2,
                            }))
                        },
                        None,
                    )
                    .await
            },
            Some(config),
        )
        .await?;

    let results: Vec<serde_json::Value> =
        batch.all.iter().filter_map(|i| i.result.clone()).collect();

    let processed_names: Vec<String> = results
        .iter()
        .filter_map(|r| {
            r.get("processed")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect();

    Ok(json!({
        "success_count": batch.success_count(),
        "results": results,
        "processed_names": processed_names,
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
