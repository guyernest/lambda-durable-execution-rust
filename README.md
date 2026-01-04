# Lambda Durable Execution (rust/experimental)

lambda-durable-execution-rust is an experimental, community-maintained Rust SDK for AWS Lambda Durable Execution. It provides durable operations for checkpointing, waits, callbacks, invokes, and fan-out patterns.

> [!NOTE]
> This repository is not an official AWS project. The API follows the public JavaScript and Python SDKs. The implementation is developed independently. Parts of the implementation were drafted with AI assistants (see AGENTS.md and CLAUDE.md) and have been exercised only in the author's workloads. Consider this repository for experimentation. Evaluate production use against requirements.

## Status / expectations

- Experimental: APIs may change and edge cases are still being explored.
- Compatibility-first: The goal is to match Durable Execution service semantics and the official SDK behavior where practical.
- MSRV: Rust 1.88 (edition 2021).

## Documentation

- [ARCHITECTURE.md](ARCHITECTURE.md) - Internal architecture, operation diagrams, and code examples.
- [examples/](examples/) - Deployable Lambda examples with SAM template.

## Quickstart

Add the dependency:
```toml
[dependencies]
lambda-durable-execution-rust = "0.1.0"
lambda_runtime = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
serde = { version = "1", features = ["derive"] }
```

Minimal workflow (checkpointed step and durable wait):
```rust,no_run
use lambda_durable_execution_rust::prelude::*;
use lambda_durable_execution_rust::runtime::with_durable_execution_service;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct Event { name: String }

#[derive(Serialize)]
struct Response { message: String }

async fn handler(event: Event, ctx: DurableContextHandle) -> DurableResult<Response> {
    let name = event.name.clone();
    let len = ctx
        .step(Some("name-length"), move |_step_ctx| async move {
            Ok(name.len())
        }, None)
        .await?;
    ctx.wait(Some("wait-10s"), Duration::seconds(10)).await?;
    Ok(Response { message: format!("Hello {}, len={len}", event.name) })
}

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    lambda_runtime::run(with_durable_execution_service(handler, None)).await
}
```

## Durable Operations

The SDK provides the following durable operations with checkpointing and replay support. See ARCHITECTURE.md for diagrams and examples.

| Operation | Description | Examples |
|-----------|-------------|----------|
| `step` | Checkpointed work units with optional retry | [`hello_world`](examples/src/bin/hello_world/main.rs), [`step_retry`](examples/src/bin/step_retry/main.rs) |
| `wait` | Suspend without user code execution during the wait | [`hello_world`](examples/src/bin/hello_world/main.rs) |
| `wait_for_callback` | External system integration (approvals, webhooks) | [`callback_example`](examples/src/bin/callback_example/main.rs), [`wait_for_callback_heartbeat`](examples/src/bin/wait_for_callback_heartbeat/main.rs) |
| `wait_for_condition` | Poll until a condition is satisfied | [`wait_for_condition`](examples/src/bin/wait_for_condition/main.rs) |
| `invoke` | Durable Lambda invocation | [`invoke_caller`](examples/src/bin/invoke_caller/main.rs), [`invoke_target`](examples/src/bin/invoke_target/main.rs) |
| `parallel` | Concurrent branch execution | [`parallel`](examples/src/bin/parallel/main.rs), [`parallel_first_successful`](examples/src/bin/parallel_first_successful/main.rs) |
| `parallel_named` | Named concurrent branches | [`parallel_named`](examples/src/bin/parallel_named/main.rs) |
| `map` | Process items concurrently | [`map_operations`](examples/src/bin/map_operations/main.rs), [`map_with_failure_tolerance`](examples/src/bin/map_with_failure_tolerance/main.rs) |
| `run_in_child_context` | Grouped operations | [`child_context`](examples/src/bin/child_context/main.rs), [`block_example`](examples/src/bin/block_example/main.rs) |

## Runtime integration

- `with_durable_execution_service(handler, config)` wraps a handler for the Lambda runtime.
- `durable_handler(handler)` exposes a builder API and allows a custom Lambda client or service.
- `DurableExecutionConfig` controls logging, retry policies, and the LambdaService implementation.

The runtime automatically:
- parses the Durable Execution invocation payload
- initializes the execution context
- checkpoints results and failures
- suspends on waits, callbacks, and retries and returns `PENDING`

Large handler responses that exceed the Lambda response size limit are checkpointed. The response payload is empty. The result can be reconstructed from checkpoint state.

## Logging

The Rust SDK provides a durable logger that mirrors the intent of the official SDKs. The JS SDK exposes `context.logger` with enriched metadata and replay suppression. The Python SDK wraps standard logging with `LogInfo` and suppresses replay logs. The Rust SDK offers a similar experience with `ctx.logger()` and `StepContext` logging helpers.

Logging surfaces:
- `ctx.logger()` for context-level messages.
- `step_ctx.info/debug/warn/error` for step-level messages.

The default logger is `TracingLogger`, which emits structured fields via `tracing`. Logs are mode-aware by default and are suppressed during replay. Disable replay suppression if full logs are needed for debugging.

The builder API supports the same configuration through `durable_handler(...).with_logger(...)`.

```rust,no_run
use lambda_durable_execution_rust::prelude::*;
use lambda_durable_execution_rust::runtime::with_durable_execution_service;
use std::sync::Arc;

async fn handler(_event: serde_json::Value, ctx: DurableContextHandle) -> DurableResult<()> {
    ctx.logger().info("handler start");
    ctx.step(
        Some("work"),
        |step_ctx| async move {
            step_ctx.info("step start");
            Ok(())
        },
        None,
    )
    .await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    let config = DurableExecutionConfig::new()
        .with_logger(Arc::new(TracingLogger))
        .with_mode_aware_logging(false);
    lambda_runtime::run(with_durable_execution_service(handler, Some(config))).await
}
```

## Design notes

- Replay safety: Step bodies should be deterministic and side-effect-free. Use durable operations to express side effects.
- ID hashing: Operation IDs are SHA-256 hashed and truncated to 128 bits (32 hex chars). This avoids MD5 and keeps IDs short. The JS SDK uses MD5-16 and the Python SDK uses BLAKE2b-64.
- Serdes: Operations can use custom serializers for individual items or full batch results (`map` and `parallel`).

## Project layout

- `Cargo.toml`: SDK crate (lambda-durable-execution-rust)
- `src/`: core SDK source
- `examples/`: separate Cargo package with deployable Lambda examples
  - `examples/src/bin/`: example handlers
  - `examples/template.yaml`: SAM template
  - `examples/scripts/`: validation tooling
  - `examples/diagrams/`: generated Mermaid and Markdown diagrams

## Module organization

Top-level modules:
- `context`: user-facing durable APIs (`DurableContextHandle`, `StepContext`, `BatchResult`, and more)
- `checkpoint`: checkpoint queueing and lifecycle management
- `termination`: termination signaling (wait, callback, retry)
- `retry`: retry strategies and presets
- `types`: configuration and SDK wire types
- `runtime`: handler wrappers and execution pipeline
- `error`: SDK error taxonomy

Within `context/durable_context`, each durable operation is split into focused submodules:
- `step`, `wait`, `wait_condition`, `callback`, `invoke`, `child`, `map`, `parallel`
- each operation has `execute` and `replay` logic separated for clarity and testability

Unit tests live alongside their modules in `#[cfg(test)]` blocks and the `context/durable_context/tests/` helpers.

## Build and test

```bash
cargo fmt
cargo test

# Lint
cargo clippy --all-targets --all-features -D warnings

# Run the examples package tests and build checks
cargo test --manifest-path examples/Cargo.toml --all-targets

# Coverage (requires llvm-cov)
cargo llvm-cov --all-features --summary-only
```

## Test utilities (feature-gated)

Mocks for the Lambda Durable Execution API are available behind the `testutils` feature (or automatically in this crate's own tests):

```toml
[dev-dependencies]
lambda-durable-execution-rust = { version = "0.1", features = ["testutils"] }
```

```rust,no_run
// Requires the `testutils` feature.
#[cfg(feature = "testutils")]
{
    use lambda_durable_execution_rust::mock::{MockCheckpointConfig, MockLambdaService};

    let mock = std::sync::Arc::new(MockLambdaService::new());
    mock.expect_checkpoint(MockCheckpointConfig::default());
}
```

The mock service queues expected responses and records calls. This allows testing of checkpoint and replay flows without AWS.

## Run examples on AWS (SAM)

See `examples/README.md` for:
- deploying `examples/template.yaml` with SAM
- validating all examples with `examples/scripts/validate.py`
- generated Mermaid and Markdown diagrams per example

## Runtime caveat (Durable Execution)

Durable Execution does not yet support the Rust runtime directly. The examples deploy using the Node.js runtime and set `AWS_LAMBDA_EXEC_WRAPPER` to `/var/task/bootstrap` so the Rust bootstrap is used. Without this, deployment fails with an error indicating that `al2023` is not a supported runtime for Durable Execution.

## License and attribution

This repository is licensed under Apache-2.0. See `LICENSE` and `NOTICE`.

The public API and behavior are modeled after the official AWS Durable Execution SDKs for JavaScript and Python. This repository is not affiliated with AWS.

## References

### AWS Documentation

For the official AWS Lambda Durable Execution service documentation:
- https://docs.aws.amazon.com/lambda/latest/dg/durable-functions.html (overview and concepts)
- https://docs.aws.amazon.com/lambda/latest/dg/durable-execution-sdk.html (SDK usage guide)
- https://docs.aws.amazon.com/lambda/latest/dg/durable-configuration.html (timeouts and retention)
- https://docs.aws.amazon.com/lambda/latest/dg/durable-best-practices.html (determinism and idempotency)

### Official SDKs

This repository follows the public Durable Execution SDK design and is validated against the official SDKs where practical:
- https://github.com/aws/aws-durable-execution-sdk-js (Node.js and TypeScript SDK)
- https://github.com/aws/aws-durable-execution-sdk-python (Python SDK)
