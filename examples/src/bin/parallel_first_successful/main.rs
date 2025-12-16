//! Parallel early-completion (“first successful”) example.
//!
//! Demonstrates:
//! - `ctx.parallel()` with `CompletionConfig::with_min_successful(1)` to return once any branch succeeds.
//! - Collecting the first successful branch result from the batch.
//!
//! Event (JSON): `{}` (ignored)
//! Result (string): `"First successful result: Task 1"` (task may vary)
//!
//! Deployed via `examples/template.yaml`.

use lambda_durable_execution_rust::context::BoxFuture;
use lambda_durable_execution_rust::prelude::*;
use lambda_durable_execution_rust::runtime::with_durable_execution_service;
use lambda_durable_execution_rust::types::CompletionConfig;
use tracing_subscriber::EnvFilter;

pub async fn handler(
    _event: serde_json::Value,
    ctx: DurableContextHandle,
) -> DurableResult<String> {
    let completion = CompletionConfig::new().with_min_successful(1);
    let config: ParallelConfig<String> = ParallelConfig::new().with_completion_config(completion);

    type BranchFn<T> =
        Box<dyn Fn(DurableContextHandle) -> BoxFuture<'static, DurableResult<T>> + Send + Sync>;

    let branches: Vec<BranchFn<String>> = vec![
        Box::new(|branch_ctx| {
            Box::pin(async move {
                branch_ctx
                    .step(
                        Some("task1"),
                        |_step_ctx| async move { Ok("Task 1".to_string()) },
                        None,
                    )
                    .await
            })
        }),
        Box::new(|branch_ctx| {
            Box::pin(async move {
                branch_ctx
                    .step(
                        Some("task2"),
                        |_step_ctx| async move { Ok("Task 2".to_string()) },
                        None,
                    )
                    .await
            })
        }),
        Box::new(|branch_ctx| {
            Box::pin(async move {
                branch_ctx
                    .step(
                        Some("task3"),
                        |_step_ctx| async move { Ok("Task 3".to_string()) },
                        None,
                    )
                    .await
            })
        }),
    ];

    let batch = ctx
        .parallel(Some("first_successful_parallel"), branches, Some(config))
        .await?;

    let first_result = batch
        .all
        .iter()
        .filter_map(|i| i.result.as_ref())
        .next()
        .cloned()
        .unwrap_or_else(|| "None".to_string());

    Ok(format!("First successful result: {first_result}"))
}

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let svc = with_durable_execution_service(handler, None);
    lambda_runtime::run(svc).await
}
