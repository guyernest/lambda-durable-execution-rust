//! Parallel fan-out (bounded concurrency) example.
//!
//! Demonstrates:
//! - `ctx.parallel()` to run multiple branches concurrently (each branch uses durable ops).
//! - `ParallelConfig::with_max_concurrency()` to bound in-flight branches.
//!
//! Event (JSON): `{}` (ignored)
//! Result (JSON): `["task 1 completed", "task 2 completed", "task 3 completed after wait"]`
//!
//! Deployed via `examples/template.yaml`.

use lambda_durable_execution_rust::context::BoxFuture;
use lambda_durable_execution_rust::prelude::*;
use lambda_durable_execution_rust::runtime::with_durable_execution_service;
use tracing_subscriber::EnvFilter;

pub async fn handler(
    _event: serde_json::Value,
    ctx: DurableContextHandle,
) -> DurableResult<Vec<String>> {
    let config: ParallelConfig<String> = ParallelConfig::new().with_max_concurrency(2);

    type BranchFn<T> =
        Box<dyn Fn(DurableContextHandle) -> BoxFuture<'static, DurableResult<T>> + Send + Sync>;

    let branches: Vec<BranchFn<String>> = vec![
        Box::new(|branch_ctx| {
            Box::pin(async move {
                branch_ctx
                    .step(
                        Some("task1"),
                        |_step_ctx| async move { Ok("task 1 completed".to_string()) },
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
                        |_step_ctx| async move { Ok("task 2 completed".to_string()) },
                        None,
                    )
                    .await
            })
        }),
        Box::new(|branch_ctx| {
            Box::pin(async move {
                branch_ctx
                    .wait(Some("wait_in_task3"), Duration::seconds(1))
                    .await?;
                Ok("task 3 completed after wait".to_string())
            })
        }),
    ];

    let batch = ctx
        .parallel(Some("parallel_operation"), branches, Some(config))
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
