# Durable Execution Examples (Rust)

This directory is a standalone Cargo package (`examples/Cargo.toml`) containing deployable AWS Lambda functions that exercise `lambda-durable-execution-rust`. Each example is also validated end-to-end by `examples/scripts/validate.py`, which invokes the deployed functions and generates diagrams from the returned durable execution history.

## Deploy

Prereqs:
- AWS credentials configured (e.g. `aws configure`) and permission to deploy Lambda/SAM + Durable Execution.
- AWS SAM CLI with the Rust `rust-cargolambda` build method enabled (preview).
- `uv` (for running the validator script).

Build and deploy to `us-east-1`:
```bash
sam build -t examples/template.yaml --beta-features
sam deploy -t examples/template.yaml --guided --region us-east-1 --stack-name durable-rust
```

## Validate & regenerate diagrams

Run all deployed examples, save artifacts under `examples/.durable-validation`, and regenerate Mermaid + Markdown diagrams under `examples/diagrams/`:
```bash
uv run examples/scripts/validate.py \
  --region us-east-1 \
  --stack durable-rust \
  --out examples/.durable-validation \
  --diagrams-out examples/diagrams \
  --mermaid \
  --timeout-seconds 240
```

Notes:
- The Mermaid is generated from the real durable execution history, so node ids may change between runs.
- The generated Markdown (`examples/diagrams/*.md`) embeds Mermaid so diagrams render directly on GitHub.

## Examples

### Hello World (`hello_world`)

A minimal typed workflow: a deterministic `ctx.step()` computes the input name length and then `ctx.wait()` suspends for 10 seconds (so the suspend/resume behavior is easy to see in history). The handler returns a typed JSON response.

- Source: [`src/bin/hello_world/main.rs`](src/bin/hello_world/main.rs)
- Diagram (Mermaid Markdown): [`diagrams/hello_world.md`](diagrams/hello_world.md)

---
### Callback / external approval (`callback_example`)

Shows `ctx.wait_for_callback()` for external coordination. The workflow submits an approval request (the “submitter” step) and then suspends until the callback is completed; `examples/scripts/validate.py` completes it using the Lambda callback APIs.

- Source: [`src/bin/callback_example/main.rs`](src/bin/callback_example/main.rs)
- Diagram (Mermaid Markdown): [`diagrams/callback_example.md`](diagrams/callback_example.md)

---
### Step retry (`step_retry`)

Demonstrates a `StepConfig` with an `ExponentialBackoff` retry strategy. Retry decisions and the scheduled delays are durable (checkpointed), so a replay resumes with the same retry schedule.

- Source: [`src/bin/step_retry/main.rs`](src/bin/step_retry/main.rs)
- Diagram (Mermaid Markdown): [`diagrams/step_retry.md`](diagrams/step_retry.md)

---
### Child context (“scoped workflow”) (`child_context`)

Uses `ctx.run_in_child_context()` to group a set of related steps under a single parent operation (“batch-processing-context”). This is useful for structuring workflows and keeping operation names scoped in the execution history.

- Source: [`src/bin/child_context/main.rs`](src/bin/child_context/main.rs)
- Diagram (Mermaid Markdown): [`diagrams/child_context.md`](diagrams/child_context.md)

---
### Map (bounded concurrency) (`map_operations`)

Illustrates `ctx.map()` for fan-out over a list of items with per-item durable steps. The example uses `MapConfig::with_max_concurrency(2)` to limit in-flight work.

- Source: [`src/bin/map_operations/main.rs`](src/bin/map_operations/main.rs)
- Diagram (Mermaid Markdown): [`diagrams/map_operations.md`](diagrams/map_operations.md)

---
### Parallel fan-out (`parallel`)

Uses `ctx.parallel()` to fan out into multiple concurrent branches and gather their results. `ParallelConfig::with_max_concurrency(2)` bounds concurrency, and one branch includes a durable wait to show that waits are replay-safe across suspends.

- Source: [`src/bin/parallel/main.rs`](src/bin/parallel/main.rs)
- Diagram (Mermaid Markdown): [`diagrams/parallel.md`](diagrams/parallel.md)

--
### Parallel "first successful" (`parallel_first_successful`)

Runs several branches concurrently but returns as soon as any branch succeeds using `CompletionConfig::with_min_successful(1)`. This pattern is useful for racing alternative strategies and keeping the fastest successful result.

- Source: [`src/bin/parallel_first_successful/main.rs`](src/bin/parallel_first_successful/main.rs)
- Diagram (Mermaid Markdown): [`diagrams/parallel_first_successful.md`](diagrams/parallel_first_successful.md)

---
### Map with failure tolerance (`map_with_failure_tolerance`)

Shows `ctx.map()` with `CompletionConfig::with_tolerated_failures(3)` so the batch can complete even if a few items fail. In this example, items divisible by 3 fail and are recorded in the batch result instead of aborting the workflow.

- Source: [`src/bin/map_with_failure_tolerance/main.rs`](src/bin/map_with_failure_tolerance/main.rs)
- Diagram (Mermaid Markdown): [`diagrams/map_with_failure_tolerance.md`](diagrams/map_with_failure_tolerance.md)

---
### Wait-for-condition (polling) (`wait_for_condition`)

Demonstrates `ctx.wait_for_condition()` to poll state until a stop condition is reached. The step increments a counter, and the decision function returns `Continue { delay }` to suspend between polls.

- Source: [`src/bin/wait_for_condition/main.rs`](src/bin/wait_for_condition/main.rs)
- Diagram (Mermaid Markdown): [`diagrams/wait_for_condition.md`](diagrams/wait_for_condition.md)

---
### Map with custom item Serdes (`map_with_custom_serdes`)

Shows how to attach an item-level `Serdes` to `ctx.map()` so each per-item result uses custom serialization/deserialization logic (useful for versioning or interop with existing payload formats). The example returns a JSON summary of the processed items.

- Source: [`src/bin/map_with_custom_serdes/main.rs`](src/bin/map_with_custom_serdes/main.rs)
- Diagram (Mermaid Markdown): [`diagrams/map_with_custom_serdes.md`](diagrams/map_with_custom_serdes.md)

---
### Wait-for-callback with heartbeat timeout (`wait_for_callback_heartbeat`)

Demonstrates configuring `CallbackConfig` with both a total timeout and a heartbeat timeout. The workflow suspends in `ctx.wait_for_callback()` until the callback is completed (the validator completes it), and the timeouts protect against forgotten callbacks.

- Source: [`src/bin/wait_for_callback_heartbeat/main.rs`](src/bin/wait_for_callback_heartbeat/main.rs)
- Diagram (Mermaid Markdown): [`diagrams/wait_for_callback_heartbeat.md`](diagrams/wait_for_callback_heartbeat.md)


---
### Multiple callbacks in one workflow (`wait_for_callback_multiple_invocations`)

Executes two `ctx.wait_for_callback()` operations sequentially, with durable waits and a step in between. This illustrates how callback ids and completions are scoped and recorded across multiple external interactions.

- Source: [`src/bin/wait_for_callback_multiple_invocations/main.rs`](src/bin/wait_for_callback_multiple_invocations/main.rs)
- Diagram (Mermaid Markdown): [`diagrams/wait_for_callback_multiple_invocations.md`](diagrams/wait_for_callback_multiple_invocations.md)


---
### Nested child contexts ("blocks") (`block_example`)

Shows nested `ctx.run_in_child_context()` calls to build a parent/child hierarchy in the workflow history. This pattern is useful for grouping operations (and their steps/waits) under meaningful names.

- Source: [`src/bin/block_example/main.rs`](src/bin/block_example/main.rs)
- Diagram (Mermaid Markdown): [`diagrams/block_example.md`](diagrams/block_example.md)

---
### Durable invoke target (`invoke_target`)

A small durable workflow intended to be called by another workflow via `ctx.invoke()`. It performs work inside a checkpointed `ctx.step()` and returns a typed response.

- Source: [`src/bin/invoke_target/main.rs`](src/bin/invoke_target/main.rs)
- Diagram (Mermaid Markdown): [`diagrams/invoke_target.md`](diagrams/invoke_target.md)


---
### Durable invoke caller (`invoke_caller`)

Demonstrates `ctx.invoke()` as a durable operation: the workflow invokes `invoke_target` (configured via `INVOKE_TARGET_FUNCTION` in the SAM template), then uses the returned value in a follow-up step.

- Source: [`src/bin/invoke_caller/main.rs`](src/bin/invoke_caller/main.rs)
- Diagram (Mermaid Markdown): [`diagrams/invoke_caller.md`](diagrams/invoke_caller.md)
