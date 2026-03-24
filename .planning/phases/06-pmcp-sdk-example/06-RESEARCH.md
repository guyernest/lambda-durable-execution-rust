# Phase 6: PMCP SDK Example - Research

**Researched:** 2026-03-24
**Domain:** MCP Agent as Client + Durable Execution + PMCP SDK Example Packaging
**Confidence:** HIGH

## Summary

Phase 6 creates a reference MCP agent example in the PMCP SDK repository (`~/Development/mcp/sdk/rust-mcp-sdk/examples/`) that demonstrates how to build an LLM + MCP tool loop agent using Durable Lambda for checkpointed execution. The example is client-side: the agent connects to MCP servers, discovers tools, calls the LLM, executes tool calls, and handles MCP Tasks (long-running tool results) via `ctx.wait_for_condition()`. The agent does NOT expose itself as an MCP server -- that is Phase 9 scope.

The PMCP SDK (v2.0.2) already provides robust client-side APIs for task-augmented tool calls: `call_tool_with_task()`, `tasks_get()`, `tasks_result()`, and the convenience `call_tool_and_poll()`. The durable execution SDK's `wait_for_condition()` primitive is the natural fit for polling MCP Tasks, as it suspends Lambda between poll attempts with zero compute cost. The production agent handler in this repo (`examples/src/bin/mcp_agent/handler.rs`) provides the proven pattern to simplify.

**Primary recommendation:** Build a self-contained, single-file PMCP SDK example (numbered `65_durable_mcp_agent.rs` or similar) that inlines a simplified agent loop using `pmcp` client APIs and `lambda-durable-execution-rust` durable primitives. Keep it educational and readable, under ~400 lines.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** Example lives in the PMCP SDK repo (`~/Development/mcp/sdk/rust-mcp-sdk/examples/`) as a reference for PMCP SDK users
- **D-02:** The example contains a simplified inline agent loop -- not the full production handler from this repo. Self-contained and easy to understand as a reference
- **D-03:** Depends on `lambda-durable-execution-rust` crate. For now, use git dependency pointing to this repo (fork). When the official AWS Rust Durable Lambda SDK releases (~April 2026), switch to the official crate. Consider publishing the fork to crates.io as an interim step to make it easier for others to use
- **D-04:** The example is primarily about building MCP agents that call MCP servers in an agent loop -- the core LLM + tool_use pattern. MCP Tasks handling is a secondary detail demonstrating the power of Durable Lambda for long-running processes
- **D-05:** Client-side MCP Tasks handling only. Server-side Tasks (exposing agents as MCP servers) is deferred to Phase 9 (Agent Teams) with the dynamic MCP server
- **D-06:** Aligns with PMCP SDK philosophy: stateless, serverless MCP patterns. The MCP agent is a natural extension of this philosophy
- **D-07:** When a tool returns a task (CreateTaskResult), the agent uses `ctx.wait_for_condition()` to poll for completion. The condition checks task status via `tasks/get`. The SDK manages wait intervals internally -- Lambda suspends between checks (no compute cost)
- **D-08:** Agent advertises task support in its client capabilities AND in each tool call
- **D-09:** Fallback behavior for non-task-aware paths: if a tool returns immediate results, handle normally. If it returns a task but the agent somehow doesn't support tasks, fall back to polling tools if available
- **D-10:** The original SDK-01, SDK-02, SDK-03 requirements describe the agent as an MCP server with TaskSupport::Required. These need revision to reflect the actual scope: agent as MCP client with task-aware tool handling. The requirements should be updated during planning to match the client-side focus.

### Claude's Discretion
- Exact simplified agent loop structure (how much of the production handler to include)
- Demo MCP server with slow tools for testing (optional -- can test against any task-supporting server)
- Documentation depth and code comments
- Example configuration approach (env vars, config file, or inline constants)

### Deferred Ideas (OUT OF SCOPE)
- Server-side MCP Tasks (wrapping agents as MCP servers) -- Phase 9 (Agent Teams) with dynamic MCP server
- Demo MCP server with slow tools for testing -- can be added later or tested against any existing task-supporting server
- Publishing the durable SDK fork to crates.io -- related but separate effort from the example itself
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| SDK-01 | **REVISED:** Durable agent acts as MCP client connecting to MCP servers in an LLM + tool loop, deployed as PMCP SDK reference example | Supported by: production handler pattern from `mcp_agent/handler.rs`, PMCP client APIs (`list_tools`, `call_tool`, `call_tool_with_task`), durable SDK primitives (`step`, `map`, `run_in_child_context`) |
| SDK-02 | **REVISED:** Client-side MCP Tasks handling -- agent detects `CreateTaskResult` from `call_tool_with_task()` and polls via `ctx.wait_for_condition()` using `tasks_get()` until terminal status | Supported by: PMCP client `call_tool_with_task()` returning `ToolCallResponse::Task(task)`, `tasks_get()` for polling, `TaskStatus::is_terminal()` for stop condition, durable `wait_for_condition()` with `WaitConditionDecision` |
| SDK-03 | **REVISED:** Progress logging during agent execution -- iteration count, tool calls made, and tokens used reported via structured tracing | Supported by: production handler pattern for token accumulation and `tracing::info!` structured logging per iteration |
</phase_requirements>

## Standard Stack

### Core (PMCP SDK Example Dependencies)
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `pmcp` | 2.0.2 | MCP client (list_tools, call_tool, call_tool_with_task, tasks_get) | The SDK this example lives in -- direct path dependency |
| `lambda-durable-execution-rust` | 0.1.0 (git dep) | Durable step/map/wait_for_condition | Provides checkpointed execution; git dep from this repo's fork |
| `lambda_runtime` | 1.0.1 | Lambda handler integration | Required by durable SDK |
| `tokio` | 1.x (features: full) | Async runtime | Required by lambda_runtime |
| `serde` / `serde_json` | 1.0 | Serialization | Checkpoint data, API payloads |
| `reqwest` | 0.13 | HTTP client for Anthropic API | Direct API calls to LLM provider |
| `tracing` | 0.1 | Structured logging | Already in PMCP SDK dev-deps |
| `tracing-subscriber` | 0.3 | Log formatting | Already in PMCP SDK dev-deps |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `url` | 2.5 | URL parsing for MCP server endpoints | Already in PMCP SDK deps |
| `pmcp-tasks` | path dep | Task types (TaskStatus) | Already in PMCP SDK dev-deps for examples |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Inline Anthropic client | `anthropic` crate | No official SDK; inline reqwest keeps the example self-contained and shows exactly what's happening |
| `call_tool_and_poll()` convenience | Manual `call_tool_with_task` + `wait_for_condition` loop | `call_tool_and_poll()` uses tokio::sleep (compute cost); `wait_for_condition()` suspends Lambda (zero cost). Manual approach is the whole point |
| Git dependency | crates.io publish | Git dep works now; switch to official crate when AWS releases it |

## Architecture Patterns

### Example File Location
```
~/Development/mcp/sdk/rust-mcp-sdk/
  Cargo.toml              # Add [[example]] entry + lambda deps to [dev-dependencies]
  examples/
    65_durable_mcp_agent.rs   # The example file (single file, self-contained)
```

**Numbering rationale:** Existing examples go up to 64. The 60-series covers Tasks. 65 fits naturally as "agent that uses tasks."

### PMCP SDK Example Convention
All PMCP SDK examples are:
1. Single `.rs` files in `examples/` directory
2. Registered as `[[example]]` in the workspace `Cargo.toml`
3. Documented with a `//!` doc comment header explaining purpose, what it demonstrates, and how to run
4. Self-contained (no external modules or sub-directories for simple examples)
5. Run via `cargo run --example <name>` (though this Lambda example would need SAM deployment)

### Pattern 1: Simplified Agent Loop
**What:** A minimal LLM + MCP tool loop using durable primitives.
**When to use:** This is the core of the example.
**Structure:**
```rust
// Simplified from production handler -- inline types, no config loading
async fn agent_handler(
    event: AgentInput,
    ctx: DurableContextHandle,
) -> DurableResult<AgentOutput> {
    // 1. Discover tools from configured MCP servers (durable step)
    let tools = ctx.step(Some("discover-tools"), |_| async {
        discover_tools(&server_urls).await
    }, None).await?;

    // 2. Agent loop with child context per iteration
    for i in 0..max_iterations {
        let result = ctx.run_in_child_context(
            Some(&format!("iteration-{i}")),
            |child_ctx| async {
                // LLM call via durable step
                let response = child_ctx.step(Some("llm-call"), |_| async {
                    call_anthropic(&messages, &tools).await
                }, Some(step_config_with_retry)).await?;

                // Execute tool calls via durable map
                if has_tool_calls(&response) {
                    let results = child_ctx.map(
                        Some("tools"),
                        function_calls,
                        |call, _ctx, _idx| async {
                            execute_tool_call_with_task_handling(&client, &call).await
                        },
                        None,
                    ).await?;
                }
                // ...
            },
            None,
        ).await?;
    }
}
```

### Pattern 2: Task-Aware Tool Execution via wait_for_condition
**What:** When `call_tool_with_task()` returns a Task, use `ctx.wait_for_condition()` to poll.
**When to use:** MCP servers with long-running tools that return CreateTaskResult.
**Example:**
```rust
// Source: PMCP SDK client API + durable wait_for_condition
async fn execute_tool_with_task_support(
    ctx: &DurableContextHandle,
    client: &Client<StreamableHttpTransport>,
    tool_name: &str,
    args: serde_json::Value,
) -> DurableResult<String> {
    // Attempt task-augmented call
    let response = client.call_tool_with_task(
        tool_name.to_string(), args,
    ).await.map_err(|e| DurableError::Internal(e.to_string()))?;

    match response {
        ToolCallResponse::Result(result) => {
            // Immediate result -- extract text
            Ok(extract_text(&result))
        }
        ToolCallResponse::Task(initial_task) => {
            // Long-running: poll via wait_for_condition
            let task_id = initial_task.task_id.clone();
            let poll_interval = initial_task.poll_interval.unwrap_or(5000);
            let client_clone = client.clone();

            let config = WaitConditionConfig::new(
                initial_task,
                Arc::new(move |task: &Task, _attempt: u32| {
                    if task.status.is_terminal() {
                        WaitConditionDecision::Stop
                    } else {
                        WaitConditionDecision::Continue {
                            delay: Duration::seconds(poll_interval as i32 / 1000),
                        }
                    }
                }),
            ).with_max_attempts(60); // 5-minute timeout at 5s intervals

            let final_task = ctx.wait_for_condition(
                Some(&format!("poll-task-{task_id}")),
                move |_current: Task, _step_ctx: StepContext| {
                    let c = client_clone.clone();
                    let tid = task_id.clone();
                    async move {
                        c.tasks_get(&tid).await
                            .map_err(|e| DurableError::Internal(e.to_string()))
                    }
                },
                config,
            ).await?;

            // Get the result
            if final_task.status == TaskStatus::Completed {
                let result = client.tasks_result(&final_task.task_id).await
                    .map_err(|e| DurableError::Internal(e.to_string()))?;
                Ok(extract_text(&result))
            } else {
                Err(DurableError::Internal(format!(
                    "Task {} ended with status: {}", final_task.task_id, final_task.status
                )))
            }
        }
    }
}
```

### Pattern 3: Inline Anthropic Client (Simplified)
**What:** Direct reqwest calls to Anthropic Messages API, no abstraction layers.
**When to use:** Keep the example self-contained without importing production LLM service.
**Example:**
```rust
// Source: Production handler simplified for example clarity
async fn call_anthropic(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    messages: &[Message],
    tools: &[Tool],
) -> Result<AnthropicResponse, Box<dyn std::error::Error + Send + Sync>> {
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 4096,
        "messages": messages,
        "tools": tools,
    });

    let resp = client.post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;

    Ok(resp.json::<AnthropicResponse>().await?)
}
```

### Anti-Patterns to Avoid
- **Importing production handler modules:** The example must be self-contained in the PMCP SDK repo, not depend on the agent binary's module structure.
- **Using `call_tool_and_poll()` for task polling:** This uses `tokio::sleep()` which wastes Lambda compute. Use `wait_for_condition()` instead which suspends Lambda.
- **Complex configuration loading:** The example should use env vars or inline constants, not DynamoDB config. Keep it simple.
- **Non-deterministic side effects outside durable steps:** All LLM calls and tool executions must be inside `ctx.step()` or `ctx.map()` for replay safety.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| MCP client protocol | Raw HTTP/SSE client | `pmcp::Client` with `StreamableHttpTransport` | Protocol negotiation, session management, pagination built-in |
| Task-augmented calls | Manual `task` field insertion | `client.call_tool_with_task()` | Correctly parses `CreateTaskResult` vs `CallToolResult` responses |
| Task polling in Lambda | `tokio::sleep` loop | `ctx.wait_for_condition()` | Lambda suspends between polls -- zero compute cost |
| Tool schema translation | Custom JSON reshaping | Copy the `translate_mcp_tool` pattern | Already handles missing `type: "object"`, empty `required` arrays |
| Retry with backoff | Manual retry loop | `StepConfig::new().with_retry_strategy(Arc::new(ExponentialBackoff::builder()...))` | Durable SDK handles checkpoint + retry scheduling |

**Key insight:** The durable SDK's `wait_for_condition()` is the killer feature for MCP Tasks. Traditional polling wastes compute; `wait_for_condition()` checkpoints the state, suspends Lambda, and resumes when the delay expires. Zero cost during waits.

## Common Pitfalls

### Pitfall 1: MCP Client Connection Lifecycle in Durable Lambda
**What goes wrong:** Creating MCP client connections inside durable steps. On replay, the connection attempt is skipped (result replayed from checkpoint), but the live connection object is never created, so subsequent tool calls fail.
**Why it happens:** Natural instinct is to wrap everything in `ctx.step()` for durability.
**How to avoid:** Establish MCP connections OUTSIDE durable steps. Cache them in `Arc<HashMap>`. They will be re-established on each Lambda invocation (which is correct -- connections are ephemeral in Lambda).
**Warning signs:** `McpError::ToolExecutionFailed` with "No cached client" after a replay.

### Pitfall 2: Task Type Not Serializable for Checkpoint
**What goes wrong:** Using `pmcp::types::tasks::Task` as the state type for `wait_for_condition()` but it doesn't implement the required `Serialize + DeserializeOwned + Clone + Send + Sync + 'static` bounds.
**Why it happens:** PMCP's `Task` type is `#[non_exhaustive]` with `Serialize + Deserialize + Clone + Debug`.
**How to avoid:** Verify that `Task` satisfies all bounds required by `WaitConditionConfig<T>`. It derives `Serialize, Deserialize, Clone, Default` and is `Send + Sync` (only contains owned types). The `#[non_exhaustive]` attribute doesn't affect serialization -- it only prevents construction via struct literals.
**Warning signs:** Compile error on `WaitConditionConfig::new()` call.

### Pitfall 3: Checkpoint Size with Message History
**What goes wrong:** Agent message history grows with each iteration (LLM responses, tool results). At some point the checkpoint payload exceeds the 750KB batch limit.
**Why it happens:** Each `run_in_child_context` checkpoints its output, which includes the accumulated message history.
**How to avoid:** For the simplified example, this is unlikely (few iterations). For production, truncate or summarize old messages. Document this limitation in the example comments.
**Warning signs:** `DurableError::Internal("Checkpoint batch exceeds 750KB limit")`.

### Pitfall 4: split_once("__") for Tool Routing
**What goes wrong:** Using `split("__")` instead of `split_once("__")` to extract the original tool name from the prefixed format `{prefix}__{tool_name}`.
**Why it happens:** Tool names themselves might contain `__` (e.g., `my__tool`).
**How to avoid:** Use `split_once("__")` which splits at the first occurrence only. The prefix is guaranteed to not contain `__`.
**Warning signs:** Tool name mismatch errors when calling tools with `__` in their names.

### Pitfall 5: Example Dependency Isolation
**What goes wrong:** Adding `lambda-durable-execution-rust` and AWS SDK crates to the main PMCP SDK dependencies instead of dev-dependencies.
**Why it happens:** The example needs these crates, and it's tempting to add them to `[dependencies]`.
**How to avoid:** Add all Lambda/AWS deps to `[dev-dependencies]` only. Use `required-features` on the `[[example]]` entry if needed to gate compilation.
**Warning signs:** PMCP SDK users suddenly pulling in `lambda_runtime` and `aws-sdk-lambda` as transitive deps.

### Pitfall 6: wait_for_condition Check Function Must Return Updated State
**What goes wrong:** The check function in `wait_for_condition()` returns the same state object without actually polling. The condition never progresses.
**Why it happens:** Misunderstanding the API -- the check function must perform the actual poll (call `tasks_get()`) and return the new Task state.
**How to avoid:** The check function signature is `Fn(T, StepContext) -> Future<Output = DurableResult<T>>`. It receives the current state, should perform the poll, and return the updated state. The wait strategy then examines the returned state to decide Stop/Continue.
**Warning signs:** Infinite retry loop with the same state, eventually hitting `max_attempts`.

## Code Examples

### Complete Agent Input/Output Types (Simplified)
```rust
// Source: Simplified from examples/src/bin/mcp_agent/types.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInput {
    pub prompt: String,
    pub mcp_server_urls: Vec<String>,
    // Inline config for simplicity (no DynamoDB)
    pub model: Option<String>,
    pub api_key_env: Option<String>,
    pub max_iterations: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentOutput {
    pub response: String,
    pub iterations: u32,
    pub total_input_tokens: u32,
    pub total_output_tokens: u32,
    pub tools_called: Vec<String>,
}
```

### MCP Client Setup (Outside Durable Steps)
```rust
// Source: examples/src/bin/mcp_agent/mcp/client.rs (simplified)
use pmcp::shared::streamable_http::{StreamableHttpTransport, StreamableHttpTransportConfig};
use pmcp::{Client, ClientCapabilities, Implementation};

async fn create_mcp_client(
    url: &str,
) -> Result<Client<StreamableHttpTransport>, Box<dyn std::error::Error + Send + Sync>> {
    let parsed = url::Url::parse(url)?;
    let config = StreamableHttpTransportConfig {
        url: parsed,
        extra_headers: vec![],
        auth_provider: None,
        session_id: None,
        enable_json_response: false,
        on_resumption_token: None,
        http_middleware_chain: None,
    };
    let transport = StreamableHttpTransport::new(config);
    let mut client = Client::with_info(
        transport,
        Implementation::new("durable-mcp-agent-example", "0.1.0"),
    );
    client.initialize(ClientCapabilities::default()).await?;
    Ok(client)
}
```

### Cargo.toml Changes for PMCP SDK
```toml
# In ~/Development/mcp/sdk/rust-mcp-sdk/Cargo.toml

[dev-dependencies]
# ... existing dev-deps ...
# For durable agent example:
lambda-durable-execution-rust = { git = "https://github.com/<user>/lambda-durable-execution-rust.git" }
lambda_runtime = "1.0.1"
reqwest = { version = "0.13", features = ["json", "rustls"] }
aws-config = { version = "1.8", features = ["behavior-version-latest"] }

[[example]]
name = "65_durable_mcp_agent"
path = "examples/65_durable_mcp_agent.rs"
required-features = ["streamable-http"]
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| MCP SSE transport | Streamable HTTP transport | MCP 2025-11-25 | SSE being deprecated; use `StreamableHttpTransport` |
| Client polls with tokio::sleep | Client polls with `wait_for_condition()` | Durable SDK 0.1.0 | Zero compute cost during waits -- Lambda suspends |
| Step Functions agent orchestration | Durable Lambda agent loop | This project | Single Lambda replaces entire state machine |
| `reqwest` 0.12 | `reqwest` 0.13 | PMCP SDK 2.0.2 | Already uses 0.13 in PMCP SDK; match the version |

**Deprecated/outdated:**
- SSE transport: Being replaced by Streamable HTTP in MCP protocol. Example should use `StreamableHttpTransport`.
- `call_tool_and_poll()`: Uses `tokio::sleep` internally. Works for non-Lambda contexts but wastes compute in Lambda. Use `wait_for_condition()` instead.

## Open Questions

1. **Exact git dependency URL for lambda-durable-execution-rust**
   - What we know: The repo is at `~/Development/mcp/lambda-durable-execution-rust` locally, and is a fork of the AWS SDK.
   - What's unclear: The GitHub remote URL for the git dependency. Likely the user's GitHub fork.
   - Recommendation: Use the local path during development (`path = "../../lambda-durable-execution-rust"`), switch to git URL before publishing.

2. **reqwest version alignment**
   - What we know: PMCP SDK uses `reqwest` 0.13 (confirmed in `Cargo.toml`). The durable execution examples use `reqwest` 0.13. The CLAUDE.md stack section says 0.12.
   - What's unclear: Whether 0.12 or 0.13 should be used.
   - Recommendation: Use 0.13 to match both PMCP SDK and current examples Cargo.toml. The CLAUDE.md stack section is stale.

3. **Example number assignment**
   - What we know: Current highest numbered example is 64. 60-series covers Tasks.
   - What's unclear: Whether 65 is the right number or if the user has a different convention.
   - Recommendation: Use 65 as it continues the sequence and sits near the Tasks examples.

4. **SAM template for the example**
   - What we know: Lambda examples need SAM templates for deployment. The durable SDK repo has one at `examples/template.yaml`.
   - What's unclear: Whether the PMCP SDK example should include its own SAM template or just document deployment.
   - Recommendation: Include a minimal SAM template as a companion file or inline it in the example's doc comment. Keep it separate from the PMCP SDK's main build.

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | cargo test (Rust built-in) |
| Config file | None needed -- standard `cargo test` |
| Quick run command | `cargo test --manifest-path ~/Development/mcp/sdk/rust-mcp-sdk/Cargo.toml --example 65_durable_mcp_agent` |
| Full suite command | `cargo build --manifest-path ~/Development/mcp/sdk/rust-mcp-sdk/Cargo.toml --example 65_durable_mcp_agent --features streamable-http` |

### Phase Requirements to Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| SDK-01 | Example compiles and contains agent loop with MCP client + durable primitives | build | `cargo build --example 65_durable_mcp_agent --features streamable-http` | Wave 0 |
| SDK-02 | Task-aware tool execution uses `call_tool_with_task` and `wait_for_condition` | manual-only | N/A -- requires deployed Lambda + MCP server with Tasks | N/A |
| SDK-03 | Progress logging emits structured tracing with iteration, tokens, tools | manual-only | N/A -- requires deployed Lambda execution logs | N/A |

### Sampling Rate
- **Per task commit:** `cargo build --example 65_durable_mcp_agent --features streamable-http`
- **Per wave merge:** Full PMCP SDK test suite: `cargo test --manifest-path ~/Development/mcp/sdk/rust-mcp-sdk/Cargo.toml`
- **Phase gate:** Example compiles with all features; doc comments render correctly

### Wave 0 Gaps
- [ ] `65_durable_mcp_agent.rs` -- the example file itself (SDK-01)
- [ ] `[[example]]` entry in PMCP SDK `Cargo.toml` + required dev-dependencies
- [ ] `lambda-durable-execution-rust` added as dev-dependency (git or path)

*(SDK-02 and SDK-03 are manual-only: they require a deployed Lambda + live MCP server. The build-time check confirms the code compiles and the patterns are correct.)*

## Sources

### Primary (HIGH confidence)
- `/Users/guy/Development/mcp/sdk/rust-mcp-sdk/Cargo.toml` -- PMCP SDK v2.0.2, dependency versions, example conventions
- `/Users/guy/Development/mcp/sdk/rust-mcp-sdk/src/client/mod.rs` -- `call_tool_with_task()`, `tasks_get()`, `tasks_result()`, `call_tool_and_poll()`, `ToolCallResponse` enum
- `/Users/guy/Development/mcp/sdk/rust-mcp-sdk/docs/TASKS_WITH_POLLING.md` -- MCP Tasks lifecycle, requestor-driven detection, polling flow
- `/Users/guy/Development/mcp/sdk/rust-mcp-sdk/src/types/tasks.rs` -- `TaskStatus` enum, `Task` struct, `is_terminal()`
- `/Users/guy/Development/mcp/sdk/rust-mcp-sdk/examples/60_tasks_basic.rs` -- Server-side task example (reference for understanding the protocol)
- `/Users/guy/Development/mcp/lambda-durable-execution-rust/src/context/durable_context/wait_condition.rs` -- `wait_for_condition()` API signature
- `/Users/guy/Development/mcp/lambda-durable-execution-rust/src/context/durable_context/wait_condition/execute.rs` -- `WaitConditionDecision::Continue/Stop`, checkpoint/retry/succeed lifecycle
- `/Users/guy/Development/mcp/lambda-durable-execution-rust/src/types/config/wait_condition.rs` -- `WaitConditionConfig<T>`, `WaitConditionDecision` types
- `/Users/guy/Development/mcp/lambda-durable-execution-rust/examples/src/bin/wait_for_condition/main.rs` -- `wait_for_condition` usage pattern
- `/Users/guy/Development/mcp/lambda-durable-execution-rust/examples/src/bin/mcp_agent/handler.rs` -- Production agent loop pattern to simplify
- `/Users/guy/Development/mcp/lambda-durable-execution-rust/examples/src/bin/mcp_agent/mcp/client.rs` -- MCP client setup, tool discovery, tool execution patterns
- `/Users/guy/Development/mcp/lambda-durable-execution-rust/examples/Cargo.toml` -- Dependency versions for the agent binary

### Secondary (MEDIUM confidence)
- PMCP SDK example numbering convention -- inferred from `Cargo.toml` `[[example]]` entries (01-64)

### Tertiary (LOW confidence)
- None

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- all libraries verified from actual Cargo.toml files in both repos
- Architecture: HIGH -- patterns directly extracted from working production code and SDK source
- Pitfalls: HIGH -- based on actual API signatures and type bounds verified in source code

**Research date:** 2026-03-24
**Valid until:** 2026-04-24 (stable -- core APIs unlikely to change in 30 days)
