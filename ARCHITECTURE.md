# Architecture

This document explains the internal architecture of the Lambda Durable Execution Rust SDK.

## Table of Contents

- [Overview](#overview)
- [Execution Lifecycle](#execution-lifecycle)
- [Core Components](#core-components)
- [Operation Types](#operation-types)
  - [Step](#step-srccontextdurable_contextstep)
  - [Wait](#wait-srccontextdurable_contextwaitrs)
  - [Callback](#callback-srccontextdurable_contextcallbackrs)
  - [ChainedInvoke](#chainedinvoke-srccontextdurable_contextinvoke)
  - [Parallel/Map](#parallelmap-srccontextdurable_contextparallelrs-maprs)
  - [WaitForCondition](#waitforcondition-srccontextdurable_contextwait_condition)
  - [Context and Execution Operations](#context-and-execution-operations)
- [Replay Mechanism](#replay-mechanism)
- [Checkpoint Protocol](#checkpoint-protocol)
- [Error Handling](#error-handling)
- [Module Structure](#module-structure)
- [Testing](#testing)

## Overview

The SDK enables Lambda functions to execute long-running workflows by checkpointing state to an AWS control plane. When a Lambda needs to wait (for time, callbacks, or chained invocations), it suspends and the control plane re-invokes it later with the updated state.


```mermaid
flowchart TB
    Handler["<b>User's Durable Function</b><br/>async fn handler(event: E, ctx: DurableContextHandle) -> Result&lt;R&gt;"]

    Runtime["<b>Runtime Handler</b> (src/runtime/handler/)<br/>- Parse DurableExecutionInvocationInput<br/>- Set up ExecutionContext<br/>- Race handler against termination<br/>- Return DurableExecutionInvocationOutput"]

    CM["<b>CheckpointManager</b><br/>Batches updates, calls Lambda API, tracks lifecycle"]
    TM["<b>TerminationManager</b><br/>Watch channel, termination reasons"]
    EC["<b>ExecutionContext</b><br/>Operation IDs, step data, parent context"]

    ControlPlane["<b>AWS Durable Execution Control Plane</b><br/>- Stores operation history<br/>- Manages checkpoint tokens<br/>- Invokes chained Lambdas<br/>- Schedules wait timers<br/>- Routes external callbacks<br/>- Re-invokes Lambda with updated state"]

    Handler --> Runtime
    Runtime --> CM
    Runtime --> TM
    Runtime --> EC
    CM --> ControlPlane
```

## Execution Lifecycle

### First Invocation

1. AWS control plane invokes Lambda with `DurableExecutionInvocationInput`:
   - `durable_execution_arn`: Unique identifier for this execution
   - `checkpoint_token`: Token for subsequent checkpoint calls
   - `initial_execution_state`: Contains the `Execution` operation with user's input payload

2. Runtime extracts user event from `execution_details.input_payload`

3. User handler runs, calling durable operations (`step`, `wait`, `invoke`, etc.)

4. Each operation:
   - Generates a deterministic ID (hashed from operation name/sequence)
   - Checks if result exists in replay data (it will not on first run)
   - Executes the operation
   - Checkpoints the result to the control plane

5. If handler completes: returns `SUCCEEDED` with result
6. If handler suspends (wait/callback/invoke): returns `PENDING`

### Subsequent Invocations (Replay)

1. Control plane re-invokes Lambda with updated `initial_execution_state` containing all previous operations

2. User handler re-runs **from the beginning**

3. Each operation checks replay data:
   - If `Succeeded`: returns cached result immediately (no re-execution)
   - If `Failed`: returns cached error
   - If `Started`: behavior depends on operation type and semantics (steps with `AtLeastOncePerRetry` re-execute; others suspend)
   - If `Pending`: suspends again (operation still in progress)

4. Handler continues until it hits a new operation or completes

**First Run:**
```mermaid
flowchart LR
    A1["ctx.step('a', ...)"] --> A2["ctx.step('b', ...)"]
    A2 --> A3["ctx.wait(60s)"]
    A3 --> A4(("SUSPENDS"))
    A4 --> A5["Lambda frozen"]
```

**Second Run (after wait completes):**
```mermaid
flowchart LR
    B1["ctx.step('a', ...)"] -->|cached| B2["ctx.step('b', ...)"]
    B2 -->|cached| B3["ctx.wait(60s)"]
    B3 -->|cached| B4["ctx.step('c', ...)"]
    B4 -->|NEW| B5["Returns result"]
```

## Core Components

### Runtime Handler (`src/runtime/handler/`)

The entry point wrapping user handlers:

```rust
pub fn with_durable_execution_service<E, R, F, Fut>(
    handler: F,
    config: Option<DurableExecutionConfig>,
) -> impl Service<LambdaEvent<DurableExecutionInvocationInput>, ...>
```

Example using with_durable_execution_service:

```rust,no_run
use lambda_durable_execution_rust::prelude::*;
use lambda_durable_execution_rust::runtime::with_durable_execution_service;

async fn handler(_event: serde_json::Value, _ctx: DurableContextHandle) -> DurableResult<()> {
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    let service = with_durable_execution_service(handler, None);
    lambda_runtime::run(service).await
}
```

The service wrapper is the shortest path for default configuration. The builder is useful when injecting a custom Lambda client or service configuration.

Example using the durable_handler builder:

```rust,no_run
use lambda_durable_execution_rust::prelude::*;
use lambda_durable_execution_rust::runtime::durable_handler;
use lambda_durable_execution_rust::types::DurableExecutionInvocationInput;
use lambda_runtime::{service_fn, LambdaEvent};

async fn handler(_event: serde_json::Value, _ctx: DurableContextHandle) -> DurableResult<()> {
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    let handler_fn = durable_handler(handler).build();
    let service = service_fn(move |event: LambdaEvent<DurableExecutionInvocationInput>| {
        let handler_fn = handler_fn.clone();
        async move { handler_fn(event.payload).await }
    });
    lambda_runtime::run(service).await
}
```

Key mechanism: **racing handler against termination**.

```rust
let result = tokio::select! {
    handler_result = handler_future => {
        Some(handler_result)  // Handler completed
    }
    termination = termination_manager.wait_for_termination() => {
        termination_result = Some(termination);
        None  // Termination triggered
    }
};
```

When an operation needs to suspend (wait, callback, invoke), it:
1. Calls `termination_manager.terminate_for_*()`
2. Awaits `std::future::pending()` to block

The `select!` detects the termination signal and cancels the handler future, allowing the runtime to return `PENDING` gracefully.

```mermaid
sequenceDiagram
    participant Handler as User Handler Task
    participant TM as Termination Monitor

    Handler->>Handler: ctx.invoke()
    Handler->>TM: terminate_for_invoke() sends signal
    Handler->>Handler: pending().await (blocks)

    TM->>TM: wait_for_termination() receives signal
    Note over TM: WINS THE RACE

    TM->>TM: Return PENDING to Lambda service
```

### DurableContextHandle (`src/context/`)

The user-facing API for durable operations:

```rust
impl DurableContextHandle {
    // Core operations
    pub async fn step<F, Fut, T>(&self, name, f, config) -> DurableResult<T>;
    pub async fn wait(&self, name, duration) -> DurableResult<()>;
    pub async fn invoke<I, O>(&self, name, function_id, input) -> DurableResult<O>;

    // Callback operations
    pub async fn wait_for_callback<T, F>(&self, name, submitter, config) -> DurableResult<T>;
    pub async fn create_callback<T>(&self, name, config) -> DurableResult<CallbackHandle<T>>;

    // Batch operations (return BatchResult<T> with per-item status)
    pub async fn parallel<T, F>(&self, name, branches, config) -> DurableResult<BatchResult<T>>;
    pub async fn parallel_named<T, F>(&self, name, branches, config) -> DurableResult<BatchResult<T>>;
    pub async fn map<T, U, F>(&self, name, items, f, config) -> DurableResult<BatchResult<U>>;

    // Condition waiting
    pub async fn wait_for_condition<T, F>(&self, name, check_fn, config) -> DurableResult<T>;

    // Child contexts
    pub async fn run_in_child_context<F, Fut, T>(&self, name, f, config) -> DurableResult<T>;
}
```

Key types:
- `CallbackHandle<T>`: Handle returned by `create_callback`, has `callback_id()` and `wait()` methods
- `BatchResult<T>`: Contains `all: Vec<BatchItem<T>>` with per-item status (`Succeeded`, `Failed`, `Started`), plus helpers like `values()`, `throw_if_error()`, `succeeded()`, `failed()`

Internally wraps `DurableContextImpl` which holds the `ExecutionContext`.

### ExecutionContext (`src/context/execution_context.rs`)

Shared state for a single Lambda invocation:

- `durable_execution_arn`: ARN identifying this execution
- `lambda_service`: AWS Lambda API client
- `checkpoint_manager`: For persisting state
- `termination_manager`: For signaling suspension
- `step_data`: HashMap of hashed operation ID -> Operation (replay data)
- `mode`: `Replay` or `Execution`
- `operation_counter`: AtomicU64 for generating unique operation IDs
- `current_parent_id`: Current parent for child context hierarchy
- `pending_completions`: HashSet tracking in-flight operations
- `logger`: DurableLogger for operation logging
- `mode_aware_logging`: Whether to suppress logs during replay

On initialization, if `initial_execution_state.next_marker` is set, the context paginates additional operations via `get_durable_execution_state` API calls.

### CheckpointManager (`src/checkpoint/manager.rs`)

Manages communication with the AWS control plane:

- **Batching**: Queues multiple operations, sends in batches (750KB SDK batch limit, derived from official JS/Python SDKs; AWS documents 256KB per checkpoint)
- **Coalescing**: Merges START+SUCCEED for same operation into single update
- **Lifecycle tracking**: Tracks operation states for termination decisions
- **Token management**: Updates checkpoint token after each successful call

```rust
pub async fn checkpoint(&self, step_id: String, update: OperationUpdate) -> DurableResult<()>
```

### TerminationManager (`src/termination/manager.rs`)

Coordinates Lambda suspension using tokio watch channels:

```rust
pub enum TerminationReason {
    RetryScheduled,          // Step retry delay
    WaitScheduled,           // ctx.wait()
    CallbackPending,         // ctx.wait_for_callback()
    InvokePending,           // ctx.invoke()
    CheckpointFailed,        // Unrecoverable checkpoint error
    SerdesFailed,            // Serialization/deserialization error
    ContextValidationError,  // Context validation error
    AllOperationsIdle,       // All operations complete or awaiting
    HandlerCompleted,        // Handler completed successfully
    HandlerFailed,           // Handler failed with error
}
```

When `terminate()` is called:
1. Sets `terminated` flag
2. Invokes checkpoint manager's terminating callback
3. Sends signal via watch channel
4. Runtime's `select!` picks up the signal

## Operation Types

### Step (`src/context/durable_context/step/`)

Executes a closure and checkpoints the result:

**Flow (local control):**
```mermaid
flowchart LR
    Start["START<br/>checkpoint"] --> Execute["Execute<br/>closure"]
    Execute --> Complete["SUCCEED /<br/>FAIL"]
```

**Sequence (control plane):**
```mermaid
sequenceDiagram
    participant SDK
    participant ControlPlane as Control Plane

    SDK->>ControlPlane: checkpoint(Step, START)
    Note over SDK: AtMostOncePerRetry only. AtLeastOncePerRetry queues START
    SDK->>SDK: Execute step_fn

    alt Success
        SDK->>ControlPlane: checkpoint(Step, SUCCEED)
    else Failure + retry
        SDK->>ControlPlane: checkpoint(Step, RETRY)
        Note over SDK: Lambda suspends for delay
    else Failure + no retry
        SDK->>ControlPlane: checkpoint(Step, FAIL)
    end
```

Supports retry strategies (`ExponentialBackoff`, `ConstantDelay`, etc.) with the `RETRY` action.

**Usage:**
```rust,ignore
// Basic step
let result = ctx
    .step(Some("fetch-data"), |step_ctx| async move {
        step_ctx.info("Fetching data from API");
        Ok(fetch_from_api().await?)
    }, None)
    .await?;

// With retry strategy
let config = StepConfig::new()
    .with_retry_strategy(Arc::new(ExponentialBackoff::new(3)));

ctx.step(Some("unreliable-call"), |_| async move {
    Ok(call_flaky_service().await?)
}, Some(config)).await?;
```

**Examples:** [`hello_world`](examples/src/bin/hello_world/main.rs), [`step_retry`](examples/src/bin/step_retry/main.rs)

---

### Wait (`src/context/durable_context/wait.rs`)

Suspends for a duration:

**Flow (local control):**
```mermaid
flowchart LR
    Call["ctx.wait()"] --> Replay{"Replay status?"}
    Replay -->|Succeeded| Return["Return cached result"]
    Replay -->|Not completed| Start["checkpoint(Wait, START)"]
    Start --> Suspend["Terminate invocation"]
    Suspend --> Resume["Re-invoke after timer"]
    Resume --> Return
```

**Sequence (control plane):**
```mermaid
sequenceDiagram
    participant SDK
    participant ControlPlane as Control Plane

    SDK->>ControlPlane: checkpoint(Wait, START)
    Note over SDK: Lambda suspends
    ControlPlane->>ControlPlane: Schedules timer
    ControlPlane->>SDK: Re-invoke with Succeeded status
    Note over SDK: Replay returns cached result
```

**Usage:**
```rust,ignore
ctx.wait(Some("wait-1-hour"), Duration::hours(1)).await?;
```

**Examples:** [`hello_world`](examples/src/bin/hello_world/main.rs)

---

### Callback (`src/context/durable_context/callback.rs`)

Waits for external system to call back:

**Flow (local control):**
```mermaid
flowchart LR
    Call["ctx.wait_for_callback()"] --> Replay{"Replay status?"}
    Replay -->|Succeeded| Return["Return cached result"]
    Replay -->|Failed| Error["Return error"]
    Replay -->|Not completed| Start["checkpoint(Callback, START)"]
    Start --> Awaited["mark_awaited + terminate"]
    Awaited --> External["External completes callback"]
    External --> Resume["Re-invoke with result"]
    Resume --> Return
```

**Sequence (control plane):**
```mermaid
sequenceDiagram
    participant SDK
    participant ControlPlane as Control Plane
    participant External as External System

    SDK->>ControlPlane: checkpoint(Callback, START)
    ControlPlane-->>SDK: Returns callback_id (in replay state)
    Note over SDK: SDK uses callback_id from replay,<br/>or falls back to "{arn}:{hashed_id}"
    Note over SDK: Lambda suspends

    External->>ControlPlane: SendDurableExecutionCallback(callback_id, result)
    ControlPlane->>SDK: Re-invoke with result
    Note over SDK: Replay returns cached result
```

**Usage:**
```rust,ignore
// Using wait_for_callback with submitter function
let config = CallbackConfig::<ApprovalDecision>::new()
    .with_timeout(Duration::hours(24));

let decision: ApprovalDecision = ctx
    .wait_for_callback(
        Some("await-approval"),
        |callback_id, step_ctx| async move {
            step_ctx.info(&format!("Callback ID: {}", callback_id));
            send_approval_email(&callback_id).await
        },
        Some(config),
    )
    .await?;

// Using create_callback for more control
let handle: CallbackHandle<Result> = ctx.create_callback(Some("payment"), None).await?;
initiate_payment(handle.callback_id()).await?;
// Do other work...
let result = handle.wait().await?;
```

**Examples:** [`callback_example`](examples/src/bin/callback_example/main.rs), [`wait_for_callback_heartbeat`](examples/src/bin/wait_for_callback_heartbeat/main.rs)

---

### ChainedInvoke (`src/context/durable_context/invoke/`)

Invokes another Lambda function:

**Flow (local control):**
```mermaid
flowchart LR
    Call["ctx.invoke()"] --> Replay{"Replay status?"}
    Replay -->|Succeeded| Return["Return cached result"]
    Replay -->|Failed| Error["Return error"]
    Replay -->|Not completed| Start["checkpoint(ChainedInvoke, START)"]
    Start --> Suspend["Terminate invocation"]
    Suspend --> Invoke["Control plane invokes target"]
    Invoke --> Resume["Re-invoke with result"]
    Resume --> Return
```

**Sequence (control plane):**
```mermaid
sequenceDiagram
    participant SDK
    participant ControlPlane as Control Plane
    participant Target as Target Lambda

    SDK->>ControlPlane: checkpoint(ChainedInvoke, START)<br/>function_name, payload
    Note over SDK: Lambda suspends

    ControlPlane->>Target: Invoke
    Target->>ControlPlane: Response
    ControlPlane->>ControlPlane: Store result in operation

    ControlPlane->>SDK: Re-invoke with result
    Note over SDK: Replay returns cached result
```

The SDK does **not** invoke the target directly. It checkpoints the intent and suspends. The control plane performs the actual invocation and re-invokes the original Lambda with the result.

**Usage:**
```rust,ignore
let response: TargetResponse = ctx
    .invoke(
        Some("call-processor"),
        &target_function_arn,
        Some(TargetEvent { data: "input" }),
    )
    .await?;
```

**Examples:** [`invoke_caller`](examples/src/bin/invoke_caller/main.rs), [`invoke_target`](examples/src/bin/invoke_target/main.rs)

---

### Parallel/Map (`src/context/durable_context/parallel.rs`, `map.rs`)

Executes multiple operations concurrently within a parent context:

**Flow (local control):**
```mermaid
flowchart TB
    Parent["Parent Context<br/>(Context: Parallel/Map)"] --> Gate["Scheduler<br/>(max_concurrency)"]
    Gate --> B1["Child Context A<br/>(ParallelBranch/MapItem)"]
    Gate --> B2["Child Context B<br/>(ParallelBranch/MapItem)"]
    Gate --> B3["Child Context C<br/>(ParallelBranch/MapItem)"]

    B1 --> R1["Result / Error / Started"]
    B2 --> R2["Result / Error / Started"]
    B3 --> R3["Result / Error / Started"]

    R1 --> Aggregate["BatchResult<br/>+ CompletionConfig"]
    R2 --> Aggregate
    R3 --> Aggregate
    Aggregate --> Done["Complete when:<br/>all done OR min_successful reached OR failure tolerance exceeded"]
```

**Sequence (control plane):**
```mermaid
sequenceDiagram
    participant SDK
    participant ControlPlane as Control Plane

    SDK->>ControlPlane: checkpoint(Context, START)<br/>sub_type=Parallel/Map
    loop Branch/Item (max_concurrency)
        SDK->>ControlPlane: checkpoint(Context, START)<br/>sub_type=ParallelBranch/MapItem
        SDK->>SDK: Execute child context
        SDK->>ControlPlane: checkpoint(Context, SUCCEED/FAIL)<br/>sub_type=ParallelBranch/MapItem
    end
    SDK->>ControlPlane: checkpoint(Context, SUCCEED/FAIL)<br/>sub_type=Parallel/Map
    Note over SDK: CompletionConfig may allow early completion
```

Uses child contexts with `parent_id` to group related operations. Creates `Context` operations for the parent (`sub_type = "Parallel"` or `"Map"`) and each branch/item (`"ParallelBranch"` or `"MapItem"`). See [Context and Execution Operations](#context-and-execution-operations) for details.
On replay, completed children are reconstructed from checkpoint data, while incomplete branches/items are either skipped or re-executed based on the replay state and completion rules.

**Usage (parallel):**
```rust,ignore
let branches: Vec<BranchFn<String>> = vec![
    Box::new(|ctx| Box::pin(async move { ctx.step(...).await })),
    Box::new(|ctx| Box::pin(async move { ctx.step(...).await })),
];

let config = ParallelConfig::new().with_max_concurrency(2);
let batch = ctx.parallel(Some("fetch-all"), branches, Some(config)).await?;

batch.throw_if_error()?;  // Fail fast on any error
let results = batch.values();  // Get successful results
```

**Usage (parallel_named):**
```rust,ignore
use lambda_durable_execution_rust::types::NamedParallelBranch;

let branches = vec![
    NamedParallelBranch::new(fetch_users_fn).with_name("fetch_users"),
    NamedParallelBranch::new(fetch_orders_fn).with_name("fetch_orders"),
];

let batch = ctx.parallel_named(Some("fetch-data"), branches, None).await?;
```

**Usage (map):**
```rust,ignore
let items = vec![1, 2, 3, 4, 5];
let config = MapConfig::new().with_max_concurrency(2);

let batch = ctx
    .map(
        Some("double-items"),
        items,
        |item, item_ctx, index| async move {
            item_ctx.step(Some(&format!("item-{index}")), |_| async move {
                Ok(item * 2)
            }, None).await
        },
        Some(config),
    )
    .await?;

let doubled: Vec<i32> = batch.values();  // [2, 4, 6, 8, 10]
```

**Examples:** [`parallel`](examples/src/bin/parallel/main.rs), [`parallel_named`](examples/src/bin/parallel_named/main.rs), [`parallel_first_successful`](examples/src/bin/parallel_first_successful/main.rs), [`map_operations`](examples/src/bin/map_operations/main.rs), [`map_with_failure_tolerance`](examples/src/bin/map_with_failure_tolerance/main.rs)

---

### WaitForCondition (`src/context/durable_context/wait_condition/`)

Polls a condition function until a configured strategy signals completion:

**Flow (local control):**
```mermaid
flowchart TB
    Call["ctx.wait_for_condition()"] --> Replay{"Replay status?"}
    Replay -->|Succeeded| Return["Return cached state"]
    Replay -->|Failed| Error["Return error"]
    Replay -->|Not completed| Check["check_fn(state) -> new_state"]
    Check --> Decide["wait_strategy(new_state)"]
    Decide -->|Continue| Retry["checkpoint(Step, RETRY)<br/>terminate + delay"]
    Decide -->|Stop| Succeed["checkpoint(Step, SUCCEED)<br/>return state"]
```

**Sequence (control plane):**
```mermaid
sequenceDiagram
    participant SDK
    participant ControlPlane as Control Plane

    loop Until wait_strategy returns Stop
        SDK->>SDK: Execute check_fn(state) -> new_state
        SDK->>SDK: Call wait_strategy(new_state) -> decision
        alt WaitConditionDecision::Continue { delay }
            SDK->>ControlPlane: checkpoint(Step, RETRY)<br/>sub_type="WaitForCondition"
            Note over SDK: Lambda suspends for delay
            ControlPlane->>SDK: Re-invoke after delay
        else WaitConditionDecision::Stop
            SDK->>ControlPlane: checkpoint(Step, SUCCEED)
            Note over SDK: Return current state
        end
    end
```

The `check_fn` receives the current state and returns only the updated state. The `WaitConditionDecision` (Continue/Stop) comes from the configured `wait_strategy` function, which inspects the state to decide whether to continue polling. On `Stop`, the final state is returned. Encoded as `OperationType::Step` with `sub_type = "WaitForCondition"`.

**Usage:**
```rust,ignore
let wait_strategy = Arc::new(|state: &i32, _attempt: u32| {
    if *state >= 3 {
        WaitConditionDecision::Stop
    } else {
        WaitConditionDecision::Continue { delay: Duration::seconds(5) }
    }
});

let final_state = ctx
    .wait_for_condition(
        Some("poll-status"),
        |state: i32, _step_ctx| async move { Ok(check_status(state).await?) },
        WaitConditionConfig::new(0, wait_strategy),
    )
    .await?;
```

**Examples:** [`wait_for_condition`](examples/src/bin/wait_for_condition/main.rs)

---

### Context and Execution Operations

Two additional operation types:

- **Context**: Used for grouping related operations. Created by:
  - `run_in_child_context` -> `sub_type = "RunInChildContext"`
  - `parallel`/`parallel_named` parent -> `sub_type = "Parallel"`
  - `parallel` branches -> `sub_type = "ParallelBranch"`
  - `map` parent -> `sub_type = "Map"`
  - `map` items -> `sub_type = "MapItem"`

  Context operations include `ContextDetails` with a `ReplayChildren` boolean field that controls whether child operations should be replayed.

- **Execution**: The top-level operation representing the entire durable execution. Contains `input_payload` and `output_payload` in `execution_details`.

**Usage (run_in_child_context):**
```rust,ignore
let result = ctx
    .run_in_child_context(
        Some("batch-processing"),
        |child_ctx| async move {
            let a = child_ctx.step(Some("step-a"), |_| async { Ok(1) }, None).await?;
            let b = child_ctx.step(Some("step-b"), |_| async { Ok(2) }, None).await?;
            Ok(a + b)
        },
        None,
    )
    .await?;
```

**Examples:** [`child_context`](examples/src/bin/child_context/main.rs), [`block_example`](examples/src/bin/block_example/main.rs)

## Replay Mechanism

### Operation ID Generation

Operations are identified by deterministic hashed IDs:

```rust
// ExecutionContext generates sequential IDs
fn next_operation_id(&self, name: Option<&str>) -> String {
    let counter = self.operation_counter.fetch_add(1, Ordering::SeqCst);
    match name {
        Some(n) => format!("{n}_{counter}"),  // e.g., "step_0", "fetch-data_1"
        None => format!("op_{counter}"),       // e.g., "op_0", "op_1"
    }
}

// CheckpointManager hashes the ID for storage
fn hash_id(id: &str) -> String {
    // SHA-256 hash, truncated to 32 chars
}
```

Parent context is tracked separately in `current_parent_id` and sent as `parent_id` in checkpoint updates. This ensures the same operation gets the same ID across invocations, as long as the handler is deterministic.

### Replay Flow

```mermaid
flowchart TB
    Start["Operation called<br/>(step, invoke, etc.)"]
    Hash["Generate hashed_id"]
    Check{"In replay data?"}

    Succeeded["Return cached result"]
    Failed["Return cached error"]
    Pending["Suspend again"]
    Execute["Execute operation"]
    StepCheck{"Step semantics?"}
    OpType{"Operation type?"}

    Start --> Hash --> Check
    Check -->|"Succeeded"| Succeeded
    Check -->|"Failed"| Failed
    Check -->|"Ready/Unknown"| OpType
    OpType -->|"Step"| Execute
    OpType -->|"invoke/callback/wait"| Pending
    Check -->|"Started"| StepCheck
    StepCheck -->|"AtLeastOncePerRetry"| Execute
    StepCheck -->|"AtMostOncePerRetry"| Pending
    Check -->|"Pending"| Pending
    Check -->|"Not found"| Execute
```

Operation statuses: `Ready`, `Started`, `Pending`, `Succeeded`, `Failed`, `Unknown`

```rust
// In each operation (e.g., step, invoke):
let hashed_id = Self::hash_id(&step_id);

// Check replay data
if let Some(operation) = self.execution_ctx.get_step_data(&hashed_id).await {
    match operation.status {
        OperationStatus::Succeeded => {
            // Return cached result
            return Ok(deserialize(operation.details.result));
        }
        OperationStatus::Failed => {
            // Return cached error
            return Err(operation.details.error);
        }
        OperationStatus::Ready | OperationStatus::Unknown => {
            // For steps: proceed to execute
            // For invoke/callback/wait: suspend
        }
        OperationStatus::Started => {
            // For steps: depends on StepSemantics
            //   AtLeastOncePerRetry: re-execute (default)
            //   AtMostOncePerRetry: suspend and wait
            // For invoke/callback/wait: always suspend
        }
        OperationStatus::Pending => {
            // Always suspend - operation in progress
        }
    }
}

// Not in replay - execute normally
execute_operation().await
```

### Determinism Requirements

For replay to work correctly, handlers must be deterministic:

**DO:**
- Use `ctx.step()` for any side effects
- Use the same operation names in the same order
- Pass inputs via the event, not external state

**DON'T:**
- Use `rand()` or `Uuid::new_v4()` outside of steps
- Branch on current time outside of steps
- Read external state that might change between invocations

## Checkpoint Protocol

### OperationUpdate Structure

```rust
pub struct OperationUpdate {
    pub id: String,                    // Hashed operation ID
    pub parent_id: Option<String>,     // For child contexts
    pub name: Option<String>,          // User-provided name
    pub operation_type: OperationType, // Step, Wait, Callback, ChainedInvoke, Context, Execution
    pub sub_type: Option<String>,      // e.g., "WaitForCondition", "Parallel", "Map", "Callback"
    pub action: OperationAction,       // Start, Succeed, Fail, Retry, Cancel
    pub payload: Option<String>,       // Result JSON
    pub error: Option<ErrorObject>,    // Error details

    // Type-specific options
    pub step_options: Option<StepUpdateOptions>,
    pub wait_options: Option<WaitUpdateOptions>,
    pub callback_options: Option<CallbackUpdateOptions>,
    pub chained_invoke_options: Option<ChainedInvokeUpdateOptions>,
    pub context_options: Option<ContextUpdateOptions>,
}
```

Operation types:
- `Step`: Regular step execution, also used for `wait_for_condition` (with `sub_type = "WaitForCondition"`)
- `Wait`: Time-based wait
- `Callback`: External callback
- `ChainedInvoke`: Lambda invocation
- `Context`: Grouping operation for child contexts. Sub-types: `"RunInChildContext"`, `"Parallel"`, `"ParallelBranch"`, `"Map"`, `"MapItem"`
- `Execution`: Top-level execution operation

### Checkpoint Flow

```mermaid
sequenceDiagram
    participant SDK
    participant ControlPlane as Control Plane

    SDK->>ControlPlane: checkpoint_durable_execution
    Note over SDK,ControlPlane: durable_execution_arn<br/>checkpoint_token<br/>updates: [...]

    ControlPlane->>SDK: Response
    Note over SDK,ControlPlane: checkpoint_token: "new-token"<br/>new_execution_state: {...}
```

The SDK must use the new `checkpoint_token` for subsequent calls.

## Error Handling

### DurableError Categories

```rust
pub enum DurableError {
    // Recoverable - will retry
    CheckpointFailed { recoverable: true, ... },

    // Non-recoverable - execution fails
    StepFailed { ... },
    InvocationFailed { ... },
    Internal(String),

    // Validation errors
    ContextValidationError { ... },
}
```

### Termination on Errors

Certain errors trigger immediate termination:

1. **Checkpoint failure** -> `TerminationReason::CheckpointFailed` -> Lambda error
2. **Serialization failure** -> `TerminationReason::SerdesFailed` -> Lambda error
3. **Context validation** -> `TerminationReason::ContextValidationError` -> Failed output

## Module Structure

```
src/
|-- lib.rs                     # Public API, prelude
|-- context/
|   |-- mod.rs
|   |-- execution_context.rs   # Shared invocation state
|   |-- step_context.rs        # StepContext for step closures
|   `-- durable_context/
|       |-- mod.rs             # DurableContextHandle, DurableContextImpl, CallbackHandle
|       |-- batch.rs           # Batch result building helpers
|       |-- serdes.rs          # Serialization helpers
|       |-- step.rs            # ctx.step() - delegates to step/
|       |-- step/
|       |   |-- execute.rs     # Step execution logic
|       |   `-- replay.rs      # Step replay logic
|       |-- wait.rs            # ctx.wait() - delegates to wait/
|       |-- wait/
|       |   `-- execute.rs
|       |-- wait_condition.rs  # ctx.wait_for_condition()
|       |-- wait_condition/
|       |   |-- execute.rs
|       |   `-- replay.rs
|       |-- callback.rs        # ctx.wait_for_callback(), ctx.create_callback()
|       |-- callback/
|       |   |-- execute.rs
|       |   `-- replay.rs
|       |-- invoke.rs          # ctx.invoke()
|       |-- invoke/
|       |   |-- execute.rs
|       |   `-- replay.rs
|       |-- parallel.rs        # ctx.parallel(), ctx.parallel_named()
|       |-- parallel/
|       |   |-- execute.rs
|       |   `-- replay.rs
|       |-- map.rs             # ctx.map()
|       |-- map/
|       |   |-- execute.rs
|       |   `-- replay.rs
|       |-- child.rs           # ctx.run_in_child_context()
|       `-- child/
|           |-- execute.rs
|           `-- replay.rs
|-- checkpoint/
|   |-- mod.rs
|   |-- manager.rs             # CheckpointManager (main file, includes submodules)
|   `-- manager/
|       |-- coalesce.rs        # Update coalescing logic
|       |-- lifecycle.rs       # Operation lifecycle tracking
|       |-- queue.rs           # Batch queue processing
|       `-- hash.rs            # ID hashing
|-- termination/
|   |-- mod.rs
|   `-- manager.rs             # TerminationManager
|-- runtime/
|   |-- handler.rs             # with_durable_execution_service, durable_handler
|   `-- handler/
|       `-- execute.rs         # Core execution logic
|-- retry/
|   |-- mod.rs                 # RetryStrategy trait
|   |-- strategy.rs            # ExponentialBackoff, ConstantDelay, etc.
|   `-- presets.rs             # Common configurations
|-- types/
|   |-- mod.rs
|   |-- invocation.rs          # Input/Output types, Operation, OperationStatus
|   |-- lambda_service.rs      # LambdaService trait, RealLambdaService
|   |-- batch.rs               # BatchResult, BatchItem, BatchItemStatus
|   |-- duration.rs            # Duration wrapper
|   |-- logger.rs              # DurableLogger trait, TracingLogger
|   |-- serdes.rs              # Serdes trait for custom serialization
|   `-- config/
|       |-- mod.rs
|       |-- step.rs            # StepConfig, StepSemantics
|       |-- callback.rs        # CallbackConfig
|       |-- invoke.rs          # InvokeConfig
|       |-- parallel.rs        # ParallelConfig
|       |-- map.rs             # MapConfig
|       |-- child.rs           # ChildContextConfig
|       |-- wait_condition.rs  # WaitConditionConfig, WaitConditionDecision
|       |-- completion.rs      # CompletionConfig for batch operations
|       `-- durable_execution.rs  # DurableExecutionConfig
`-- error/
    |-- mod.rs
    `-- types.rs               # DurableError, ErrorObject
```

## Quotas and Limits

The following limits apply to Lambda durable executions:

| Resource | Limit | Notes |
|----------|-------|-------|
| Execution timeout | 31,536,000 seconds (1 year) | Maximum duration for a durable execution |
| State retention | 1-365 days | How long execution state is preserved |
| Checkpoint payload | 256KB | Per checkpoint call (AWS-documented) |
| SDK batch limit | 750KB | SDK batches updates before sending (derived from official SDKs) |
| Step retry delay | 1-31,622,400 seconds | `NextAttemptDelaySeconds` range (~366 days max) |
| Operations per GetDurableExecutionState | 1,000 max | Default 100; pagination token expires after 24 hours |
| Invocation payload | 6MB sync, 1MB async | Standard Lambda limits apply |

## Testing

The SDK provides mock implementations for testing:

```rust
use lambda_durable_execution_rust::mock::{MockLambdaService, MockCheckpointConfig};

let mock = Arc::new(MockLambdaService::new());
mock.expect_checkpoint(MockCheckpointConfig {
    checkpoint_token: Some("token-1".to_string()),
    operations: vec![...],
    ..Default::default()
});

let config = DurableExecutionConfig::new()
    .with_lambda_service(mock);
```

This allows testing handler logic without actual AWS calls.
