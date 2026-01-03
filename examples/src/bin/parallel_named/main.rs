// SPDX-License-Identifier: Apache-2.0
// Ported from aws-durable-execution-sdk-python-testing examples.
// Constructed from scratch. No direct one-to-one example in the python-testing repo.
// See NOTICE for attribution.

//! Parallel with named branches example.
//!
//! Demonstrates:
//! - `ctx.parallel_named()` to run branches with custom names for debugging/tracking.
//! - Named branches appear in execution history with their assigned names.
//!
//! Event (JSON): `{}` (ignored)
//! Result (JSON): `["fetch_users completed", "fetch_orders completed", "fetch_inventory completed"]`
//!
//! Deployed via `examples/template.yaml`.

use lambda_durable_execution_rust::context::BoxFuture;
use lambda_durable_execution_rust::prelude::*;
use lambda_durable_execution_rust::runtime::with_durable_execution_service;
use lambda_durable_execution_rust::types::NamedParallelBranch;
use tracing_subscriber::EnvFilter;

pub async fn handler(
    _event: serde_json::Value,
    ctx: DurableContextHandle,
) -> DurableResult<Vec<String>> {
    // Define branch function type for clarity.
    type BranchFn = Box<
        dyn Fn(DurableContextHandle) -> BoxFuture<'static, DurableResult<String>> + Send + Sync,
    >;

    // Create the branch functions first.
    let fetch_users: BranchFn = Box::new(|branch_ctx| {
        Box::pin(async move {
            branch_ctx
                .step(
                    Some("get_users"),
                    |step_ctx| async move {
                        step_ctx.info("Fetching users from database");
                        Ok("fetch_users completed".to_string())
                    },
                    None,
                )
                .await
        })
    });

    let fetch_orders: BranchFn = Box::new(|branch_ctx| {
        Box::pin(async move {
            branch_ctx
                .step(
                    Some("get_orders"),
                    |step_ctx| async move {
                        step_ctx.info("Fetching orders from database");
                        Ok("fetch_orders completed".to_string())
                    },
                    None,
                )
                .await
        })
    });

    let fetch_inventory: BranchFn = Box::new(|branch_ctx| {
        Box::pin(async move {
            // Short wait to demonstrate async behavior
            branch_ctx
                .wait(Some("inventory_delay"), Duration::seconds(1))
                .await?;
            Ok("fetch_inventory completed".to_string())
        })
    });

    // Wrap each branch with a name - these names appear in the execution history,
    // making it easier to debug and understand the workflow.
    let branches = vec![
        NamedParallelBranch::new(fetch_users).with_name("fetch_users"),
        NamedParallelBranch::new(fetch_orders).with_name("fetch_orders"),
        NamedParallelBranch::new(fetch_inventory).with_name("fetch_inventory"),
    ];

    let config: ParallelConfig<String> = ParallelConfig::new().with_max_concurrency(2);

    let batch = ctx
        .parallel_named(Some("fetch_all_data"), branches, Some(config))
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
