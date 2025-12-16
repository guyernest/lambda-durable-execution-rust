//! Wait-for-callback with heartbeat timeout example.
//!
//! Demonstrates:
//! - `ctx.wait_for_callback()` with both a total timeout and a heartbeat timeout.
//! - Suspending until the callback is completed (or times out).
//!
//! Event (JSON): `{}` (ignored)
//! Result (JSON): `{ "callbackResult": "...", "completed": true }`
//!
//! Deployed via `examples/template.yaml`.

use lambda_durable_execution_rust::prelude::*;
use lambda_durable_execution_rust::runtime::with_durable_execution_service;
use serde_json::json;
use tracing_subscriber::EnvFilter;

pub async fn handler(
    _event: serde_json::Value,
    ctx: DurableContextHandle,
) -> DurableResult<serde_json::Value> {
    let config = CallbackConfig::<String>::new()
        .with_timeout(Duration::seconds(120))
        .with_heartbeat_timeout(Duration::seconds(15));

    let result: String = ctx
        .wait_for_callback(
            None,
            |_callback_id, _step_ctx| async move {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                Ok(())
            },
            Some(config),
        )
        .await?;

    Ok(json!({
        "callbackResult": result,
        "completed": true,
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
