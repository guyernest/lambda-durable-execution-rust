# Phase 3: Agent Loop - Research

**Researched:** 2026-03-23
**Domain:** Durable agentic loop wiring -- LLM call, tool execution, message assembly, repeat
**Confidence:** HIGH

## Summary

Phase 3 wires together the Phase 1 LLM client (`UnifiedLLMService`) and Phase 2 configuration/MCP modules (`AgentConfig`, `ToolsWithRouting`, `discover_all_tools`, `resolve_tool_call`) into a complete durable agent handler. The handler implements the standard LLM agent loop: call LLM, check for tool_use, execute tools, append results to message history, repeat until end_turn or max_iterations.

The critical challenge is making this loop replay-safe with the durable execution SDK. Each loop iteration must be wrapped in `run_in_child_context` to isolate operation ID counters (per D-08, addressing Pitfall 1/5 from research). LLM calls use `ctx.step()` with `ExponentialBackoff` for retry on transient failures. Tool calls use `ctx.map()` for parallel execution with per-tool checkpointing. Message history is NOT checkpointed as a blob -- it is reconstructed from individual step results during replay, avoiding the 256KB checkpoint size limit (Pitfall 2).

The output format must match the existing Step Functions agent `LLMResponse` shape for drop-in caller compatibility (D-02). The handler also needs to implement MCP tool execution via `call_tool()` (MCP-04/MCP-05), which was deferred from Phase 2 because it only matters in the context of the running loop.

**Primary recommendation:** Build a `handler.rs` module with the agent loop function, a `types.rs` for request/response types, and wire into `main.rs` via `with_durable_execution_service`. Keep the loop structure simple: `for i in 0..max_iterations { run_in_child_context("iteration-{i}", |child| { step("llm-call") -> map("tools") }) }`.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** Input payload is `{ agent_name: String, version: String, messages: Vec<Message> }`. Caller specifies which agent and the initial conversation. The handler loads config from AgentRegistry using agent_name/version.
- **D-02:** Output format matches the existing Step Functions agent response format for caller compatibility (drop-in replacement). The handler should read the Step Functions agent output format and replicate it.
- **D-03:** Behave like a standard MCP client -- establish connections to all configured MCP servers once at handler start, cache the clients, and reuse them for all tool calls within the invocation. This happens OUTSIDE durable steps (alongside tool discovery, which is already outside steps per D-07/P2).
- **D-04:** MCP tool calls inside `ctx.step()` or `ctx.map()` are regular async HTTP calls over the cached connections. The Lambda stays active during tool execution -- no suspension. TCP connections survive across loop iterations within the same invocation.
- **D-05:** On Lambda resume (after suspension/replay), MCP clients are re-established at handler start before the loop replays. Cached step results are returned without re-connecting.
- **D-06:** Lambda only suspends for explicit `ctx.wait()` / `ctx.wait_for_callback()` / retry backoff. A long-running `ctx.step()` (e.g., slow MCP tool call) keeps the Lambda active while the async task completes. No idle cost concern for calls under the Lambda timeout.
- **D-07:** For truly long-running tool calls (minutes), the current approach works -- Lambda waits. Using `ctx.wait_for_callback()` to avoid Lambda idle time during long tool calls is a v2 pattern.
- **D-08:** Each loop iteration uses `run_in_child_context` to isolate operation ID counters for replay determinism (per Pitfall 1 from research).
- **D-09:** Each LLM call is a `ctx.step()` with `ExponentialBackoff` retry. The step result (LLMResponse) is cached -- on replay, the LLM is NOT re-called.
- **D-10:** Tool calls within an iteration are executed via `ctx.map()` for parallel execution. Even though most turns have single tool calls, `ctx.map()` handles both single and multiple correctly.
- **D-11:** Message history is assembled incrementally from step results. Each iteration's LLM response and tool results are step outputs that rebuild naturally during replay.
- **D-12:** MCP tool errors (isError: true from `call_tool()`) are passed to the LLM as error `tool_result` messages -- the agent does not fail, the LLM decides recovery. Transport errors (connection failure) propagate as step errors.

### Claude's Discretion
- Exact handler function structure and wiring
- How to extract the Step Functions output format (read the existing agent code or ask)
- Error types for agent-level failures (handler errors vs tool errors vs LLM errors)
- Test strategy (mock durable context, mock MCP responses, mock LLM responses)
- How to structure the message assembly (Vec<UnifiedMessage> accumulation pattern)
- Whether max_iterations check happens at loop start or after LLM response

### Deferred Ideas (OUT OF SCOPE)
- `ctx.wait_for_callback()` for long-running MCP tool calls (avoid Lambda idle) -- v2 pattern
- Streaming LLM responses -- out of scope per PROJECT.md
- Agent-to-agent delegation via `ctx.invoke()` -- future capability
- Context window management (summarization, truncation) -- Phase 4+ concern
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| LOOP-01 | Agentic loop: call LLM -> check for tool_use -> execute tools -> append results -> repeat until end_turn | Core handler function structure; loop with `run_in_child_context` per iteration; branch on `stop_reason` from `LLMResponse` |
| LOOP-02 | Each LLM call is a durable `ctx.step()` with ExponentialBackoff retry for transient failures | `ctx.step()` API with `StepConfig::new().with_retry_strategy(Arc::new(ExponentialBackoff::builder()...))` |
| LOOP-03 | Tool calls executed in parallel via `ctx.map()` when LLM returns multiple tool_use blocks | `ctx.map()` API takes `Vec<FunctionCall>` items; each executes in child context with own operation counters |
| LOOP-04 | Each loop iteration uses `run_in_child_context` to isolate operation ID counters for replay determinism | `ctx.run_in_child_context(Some("iteration-{i}"), \|child_ctx\| async { ... }, None)` |
| LOOP-05 | Message history assembled incrementally from step results -- rebuilds naturally during replay | `Vec<UnifiedMessage>` accumulation from step/map return values; no separate checkpoint for history |
| LOOP-06 | Max iterations guard from AgentRegistry config -- returns graceful error when exceeded | `AgentParameters.max_iterations` field; loop bounds check; return `DurableError::Internal` or custom error |
| LOOP-07 | Final LLM response returned as durable execution result | Extract text from final `LLMResponse`, build output matching Step Functions `LLMResponse` format |
| MCP-04 | Agent executes tool calls via MCP `call_tool()` with tool results mapped to Anthropic tool_result content blocks | pmcp `client.call_tool(name, arguments)` -> `CallToolResult` with `content: Vec<Content>` and `is_error: bool` |
| MCP-05 | MCP tool errors (isError: true) passed to LLM as error tool_results -- agent does not fail, LLM decides recovery | `CallToolResult.is_error` -> `ContentBlock::ToolResult { is_error: Some(true) }` |
</phase_requirements>

## Standard Stack

### Core (already in examples/Cargo.toml)
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `lambda-durable-execution-rust` | 0.1.0 (path dep) | `ctx.step()`, `ctx.map()`, `run_in_child_context` | This repo |
| `lambda_runtime` | 1.0.1 | `with_durable_execution_service` wrapper | Already in use |
| `pmcp` | path dep (1.10.x) | MCP client `call_tool()`, `list_tools()` | Already integrated in Phase 2 |
| `serde` / `serde_json` | 1.0 | Serialize/Deserialize for checkpoint round-trip | All step results must be serializable |
| `reqwest` | 0.13 | HTTP client for LLM API calls (via UnifiedLLMService) | Already in use |
| `aws-sdk-dynamodb` | 1.98 | Config loading (via load_agent_config) | Already integrated in Phase 2 |
| `aws-sdk-secretsmanager` | 1.98 | API key retrieval (via SecretManager) | Already integrated in Phase 1 |
| `tracing` | 0.1 | Structured logging | Already in use throughout |
| `thiserror` | 2.0 | Error type definitions | Already in use |
| `url` | 2.5 | URL parsing for MCP servers | Already integrated in Phase 2 |

### No New Dependencies
Phase 3 adds no new crate dependencies. All required libraries are already in `examples/Cargo.toml` from Phases 1 and 2.

## Architecture Patterns

### Recommended Project Structure (additions in Phase 3)
```
examples/src/bin/mcp_agent/
  main.rs           -- Wire handler into with_durable_execution_service (MODIFY)
  handler.rs        -- agent_handler() function with the durable loop (NEW)
  types.rs          -- AgentRequest, AgentResponse types (NEW)
  config/           -- (exists from Phase 2)
  llm/              -- (exists from Phase 1)
  mcp/              -- (exists from Phase 2, add execute_tool_call function)
```

### Pattern 1: Handler Entry Point (main.rs)

**What:** Wire the agent handler into the Lambda runtime using `with_durable_execution_service`.
**When to use:** Lambda startup.

```rust
// main.rs
mod config;
mod handler;
mod llm;
mod mcp;
mod types;

use lambda_durable_execution_rust::runtime::with_durable_execution_service;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    // Create LLM service ONCE, outside the handler.
    // UnifiedLLMService is Clone (Arc internals) so it can be
    // captured by the Fn closure that with_durable_execution_service requires.
    let llm_service = llm::UnifiedLLMService::new().await
        .map_err(|e| lambda_runtime::Error::from(format!("Failed to init LLM service: {e}")))?;

    let svc = with_durable_execution_service(
        move |event, ctx| {
            let llm = llm_service.clone();
            async move { handler::agent_handler(event, ctx, llm).await }
        },
        None,
    );

    lambda_runtime::run(svc).await
}
```

**Critical:** The handler closure must be `Fn` (not `FnOnce`) because it's called on every Lambda invocation. `UnifiedLLMService` is `Clone`-able (Arc internals) so this works. MCP clients cannot be created here -- they are created inside the handler per D-03/D-05.

### Pattern 2: Durable Agent Loop (handler.rs)

**What:** The core agent loop with child contexts per iteration.
**When to use:** Every handler invocation.

```rust
pub async fn agent_handler(
    event: AgentRequest,
    ctx: DurableContextHandle,
    llm_service: UnifiedLLMService,
) -> DurableResult<AgentResponse> {
    // 1. Load config (durable step, cached on replay)
    let table_name = std::env::var("AGENT_REGISTRY_TABLE")
        .unwrap_or_else(|_| "AgentRegistry".to_string());
    let agent_name = event.agent_name.clone();
    let version = event.version.clone();
    let config: AgentConfig = ctx
        .step(
            Some("load-config"),
            move |_| async move {
                load_agent_config(&table_name, &agent_name, &version)
                    .await
                    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })
            },
            None,
        )
        .await?;

    // 2. Discover tools (durable step, cached on replay)
    let server_urls = config.mcp_server_urls.clone();
    let tools_with_routing: ToolsWithRouting = ctx
        .step(
            Some("discover-tools"),
            move |_| async move {
                discover_all_tools(&server_urls)
                    .await
                    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })
            },
            None,
        )
        .await?;

    // 3. Establish MCP connections (OUTSIDE durable steps per D-03)
    // These are re-created on every invocation including replay (D-05).
    // On replay, cached steps skip tool execution, so connections only
    // matter for fresh steps.
    let mcp_clients = establish_mcp_connections(&config.mcp_server_urls).await?;

    // 4. Agent loop
    let mut messages: Vec<UnifiedMessage> = event.messages;
    let max_iterations = config.parameters.max_iterations;

    for i in 0..max_iterations {
        // Each iteration in a child context (D-08)
        let iteration_result = ctx
            .run_in_child_context(
                Some(&format!("iteration-{i}")),
                |child_ctx| {
                    // Clone what the async block needs
                    let llm = llm_service.clone();
                    let cfg = config.clone();
                    let tools = tools_with_routing.clone();
                    let msgs = messages.clone();
                    let clients = mcp_clients.clone();
                    async move {
                        execute_iteration(child_ctx, &llm, &cfg, &tools, &msgs, &clients, i).await
                    }
                },
                None,
            )
            .await?;

        // Append assistant message
        messages.push(iteration_result.assistant_message.clone());

        // Check if done
        if iteration_result.is_final {
            return Ok(build_agent_response(&iteration_result, &config));
        }

        // Append tool results as user message
        if let Some(tool_results_msg) = &iteration_result.tool_results_message {
            messages.push(tool_results_msg.clone());
        }
    }

    // Max iterations exceeded (LOOP-06)
    Err(DurableError::Internal(format!(
        "Max iterations ({max_iterations}) exceeded without end_turn"
    )))
}
```

### Pattern 3: Single Iteration Execution

**What:** Execute one LLM call + tool calls within a child context.
**When to use:** Inside `run_in_child_context` per iteration.

```rust
#[derive(Clone, Serialize, Deserialize)]
struct IterationResult {
    llm_response: LLMResponse,
    assistant_message: UnifiedMessage,
    tool_results_message: Option<UnifiedMessage>,
    is_final: bool,
}

async fn execute_iteration(
    ctx: DurableContextHandle,
    llm: &UnifiedLLMService,
    config: &AgentConfig,
    tools: &ToolsWithRouting,
    messages: &[UnifiedMessage],
    mcp_clients: &McpClientCache,
    iteration: u32,
) -> DurableResult<IterationResult> {
    // LLM call (D-09: step with ExponentialBackoff)
    let invocation = build_llm_invocation(config, messages, &tools.tools);
    let llm_clone = llm.clone();
    let retry = ExponentialBackoff::builder()
        .max_attempts(3)
        .initial_delay(Duration::seconds(2))
        .max_delay(Duration::seconds(30))
        .build();
    let step_config = StepConfig::<LLMResponse>::new()
        .with_retry_strategy(Arc::new(retry));

    let llm_response: LLMResponse = ctx
        .step(
            Some("llm-call"),
            move |_| async move {
                llm_clone.process(invocation)
                    .await
                    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })
            },
            Some(step_config),
        )
        .await?;

    let assistant_message = llm_response_to_assistant_message(&llm_response);
    let is_final = llm_response.metadata.stop_reason.as_deref() == Some("end_turn");

    if is_final || llm_response.function_calls.is_none() {
        return Ok(IterationResult {
            llm_response,
            assistant_message,
            tool_results_message: None,
            is_final: true,
        });
    }

    // Tool execution (D-10: map for parallel)
    let function_calls = llm_response.function_calls.clone().unwrap_or_default();
    let routing = tools.routing.clone();
    let clients = mcp_clients.clone();

    let tool_results: BatchResult<ToolCallResult> = ctx
        .map(
            Some("tools"),
            function_calls,
            move |call, _item_ctx, _idx| {
                let routing = routing.clone();
                let clients = clients.clone();
                async move {
                    execute_tool_call(&call, &routing, &clients)
                        .await
                }
            },
            None,
        )
        .await?;

    let tool_results_message = build_tool_results_message(tool_results.values());

    Ok(IterationResult {
        llm_response,
        assistant_message,
        tool_results_message: Some(tool_results_message),
        is_final: false,
    })
}
```

### Pattern 4: MCP Tool Execution (call_tool)

**What:** Execute a single MCP tool call using the cached client connection.
**When to use:** Inside `ctx.map()` items.

```rust
#[derive(Clone, Serialize, Deserialize)]
struct ToolCallResult {
    tool_use_id: String,
    content: String,
    is_error: bool,
}

async fn execute_tool_call(
    call: &FunctionCall,
    routing: &HashMap<String, String>,
    mcp_clients: &McpClientCache,
) -> DurableResult<ToolCallResult> {
    let (server_url, original_name) = resolve_tool_call(&call.name, routing)
        .map_err(|e| DurableError::Internal(e.to_string()))?;

    let client = mcp_clients.get(&server_url)
        .ok_or_else(|| DurableError::Internal(
            format!("No MCP client for server: {server_url}")
        ))?;

    // call_tool returns CallToolResult { content: Vec<Content>, is_error: bool }
    match client.call_tool(original_name, call.input.clone()).await {
        Ok(result) => {
            // Extract text from Content blocks
            let text = result.content.iter()
                .filter_map(|c| match c {
                    pmcp::types::Content::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");

            Ok(ToolCallResult {
                tool_use_id: call.id.clone(),
                content: text,
                is_error: result.is_error, // D-12: MCP errors passed to LLM
            })
        }
        Err(e) => {
            // Transport error -> step failure (will trigger retry if configured)
            Err(DurableError::Internal(format!(
                "MCP call_tool failed for {}: {e}", call.name
            )))
        }
    }
}
```

### Pattern 5: MCP Client Caching per Invocation (D-03/D-05)

**What:** Establish MCP connections once per handler invocation, cache for reuse.
**When to use:** At handler start, before the loop.

```rust
use std::collections::HashMap;
use std::sync::Arc;

type McpClientCache = Arc<HashMap<String, pmcp::Client<StreamableHttpTransport>>>;

async fn establish_mcp_connections(
    server_urls: &[String],
) -> DurableResult<McpClientCache> {
    let mut clients = HashMap::new();
    for url_str in server_urls {
        let parsed = url::Url::parse(url_str)
            .map_err(|e| DurableError::Internal(format!("Invalid MCP URL {url_str}: {e}")))?;

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
        let mut client = pmcp::Client::with_info(
            transport,
            pmcp::Implementation::new("durable-mcp-agent", "0.1.0"),
        );
        client.initialize(pmcp::ClientCapabilities::default())
            .await
            .map_err(|e| DurableError::Internal(
                format!("MCP init failed for {url_str}: {e}")
            ))?;
        clients.insert(url_str.clone(), client);
    }
    Ok(Arc::new(clients))
}
```

**IMPORTANT NOTE on D-03 vs Architecture Research:** The CONTEXT.md decision D-03 says to cache MCP clients per invocation and reuse across tool calls. The earlier Architecture research (ARCHITECTURE.md) recommended "connect-per-use" inside step closures. D-03 explicitly overrides this -- the user decided on connection caching. This is valid because:
1. MCP connections within a single Lambda invocation are stable (no suspension during active steps)
2. On replay, cached step results return without executing the closure, so the MCP client is never used for replayed steps
3. Fresh MCP connections are only needed for new (non-replayed) tool calls

However, there is a subtlety: `ctx.map()` closures need the MCP client, but `ctx.map()` items run in child contexts. The `pmcp::Client` must be `Send + Sync` to be passed into async closures. The `Arc<HashMap<..>>` wrapper handles this. The pmcp `Client` struct uses internal `Arc` state so cloning the `Arc<HashMap>` is safe.

**PITFALL: `pmcp::Client` might not be `Clone` or might hold internal mutable state that prevents sharing.** The planner must verify this. If `Client` cannot be shared, fall back to creating a fresh connection per `ctx.map()` item (connect-per-use from Architecture research). This is a LOW-risk fallback since the durable step closures would just create their own connections.

### Pattern 6: Output Format Matching Step Functions Agent

**What:** Build the response in the same shape as the Step Functions agent.
**When to use:** When returning the final result from the handler.

The Step Functions `LLMResponse` from `~/projects/step-functions-agent/lambda/call_llm_rust/src/models.rs` has this shape:

```rust
// Step Functions agent output (the format to match)
LLMResponse {
    message: AssistantMessage {
        role: "assistant",
        content: Vec<ContentBlock>,      // Text, ToolUse, etc.
        tool_calls: Option<Value>,       // provider-specific raw format
    },
    function_calls: Option<Vec<FunctionCall>>,  // unified extracted calls
    metadata: ResponseMetadata {
        model_id: String,
        provider_id: String,
        latency_ms: u64,
        tokens_used: Option<TokenUsage>,
        stop_reason: Option<String>,
    },
}
```

The durable agent's `LLMResponse` type (from `llm/models.rs`) is **already identical** to this format -- it was adapted from the Step Functions agent code in Phase 1. So the final response can simply be the last `LLMResponse` from the final LLM call. The `AgentResponse` wrapper may add metadata (iteration count, total tokens) for observability, but the core format is already compatible.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentResponse {
    /// The final LLM response (matches Step Functions format)
    #[serde(flatten)]
    pub response: LLMResponse,
    /// Agent execution metadata (optional, Phase 4 expansion)
    pub agent_metadata: Option<AgentMetadata>,
}
```

### Anti-Patterns to Avoid

- **Checkpointing message history as a blob:** Each iteration adds ~2-10KB. After 10 iterations, a monolithic checkpoint could hit 256KB. Instead, reconstruct from step results.
- **Non-deterministic step names:** Never use `format!("tool-{}", call.name)` at the loop level. Tool names could vary between replays. Use `format!("iteration-{i}")` for child contexts and `"llm-call"` / `"tools"` inside them (counter isolation handles uniqueness).
- **Skipping child context:** Without `run_in_child_context`, the global operation counter would vary based on how many tool calls each iteration has, breaking replay determinism.
- **Treating MCP errors as handler failures:** MCP `is_error: true` means the tool returned an error result. This is data for the LLM, not a handler crash. Only transport failures should be `Err`.
- **Creating MCP clients inside `ctx.step()`:** Per D-03, MCP connections are outside durable steps. They are re-established on every invocation but only used by non-replayed steps.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Retry logic for LLM calls | Custom retry loops | `ExponentialBackoff` with `StepConfig.with_retry_strategy()` | SDK handles retry scheduling, suspension, and checkpoint state |
| Parallel tool execution | `tokio::join!` or manual fan-out | `ctx.map()` | `ctx.map()` provides per-item checkpointing and replay safety |
| Operation ID isolation | Manual counter management | `run_in_child_context` | Child contexts automatically scope operation counters |
| Message format validation | Manual alternation checks | Typed `UnifiedMessage` construction helpers | Build helpers that enforce user/assistant alternation |
| MCP tool routing | Re-parsing tool names each time | `resolve_tool_call()` from Phase 2 | Already handles prefix splitting and routing lookup |

## Common Pitfalls

### Pitfall 1: Closure Capture in Durable Steps
**What goes wrong:** The `ctx.step()` closure must be `FnOnce + Send + 'static`. Values captured must be `Send + 'static`. The `UnifiedLLMService` is `Clone` (Arc internals) but the `LLMInvocation` contains `Vec<UnifiedMessage>` which must be cloned before capture. Failing to clone properly causes borrow checker errors.
**Why it happens:** The step closure is `move` and outlives the current scope. References to local variables cannot be captured.
**How to avoid:** Clone all needed data before the `ctx.step()` call. Use the pattern: `let llm = llm.clone(); let invocation = invocation.clone(); ctx.step(name, move |_| async move { llm.process(invocation).await }, config)`.
**Warning signs:** Compile errors about lifetimes or `Send` bounds on step closures.

### Pitfall 2: `ctx.map()` Closure is `Fn`, Not `FnOnce`
**What goes wrong:** `ctx.map()` takes `F: Fn(TIn, DurableContextHandle, usize) -> Fut` (note: `Fn` not `FnOnce`). The closure is called once per item. Values captured must be cloneable or behind Arc. If capturing a `HashMap<String, String>` for routing, it must be cloned into the closure.
**Why it happens:** `map` processes multiple items with the same closure.
**How to avoid:** Wrap non-Clone values in `Arc`. Clone `Arc` references into the closure.
**Warning signs:** Compile errors about `Fn` vs `FnOnce` on the map closure.

### Pitfall 3: `IterationResult` Must Be Serializable
**What goes wrong:** `run_in_child_context` requires `T: Serialize + DeserializeOwned`. If the iteration result type contains non-serializable fields (e.g., `Arc`, function pointers, raw errors), the checkpoint fails.
**Why it happens:** Child context results are checkpointed just like step results.
**How to avoid:** Define a `#[derive(Serialize, Deserialize)]` struct for iteration results. Only include serializable data (strings, numbers, vectors of serializable types).
**Warning signs:** Runtime serialization errors during checkpoint.

### Pitfall 4: Message History Grows on Each Iteration
**What goes wrong:** The `messages` vector is cloned into each `run_in_child_context` closure. By iteration 10, each clone contains all previous messages. This is O(N^2) in total memory allocated across all iterations.
**Why it happens:** Each iteration needs the full history for the LLM call.
**How to avoid:** This is inherent to the pattern and acceptable for reasonable iteration counts (10-25). The clones are short-lived (freed after each iteration). For extreme cases, `max_iterations` provides the guard.
**Warning signs:** Memory pressure in CloudWatch metrics for long-running agents.

### Pitfall 5: pmcp::Client Sharing Across map() Items
**What goes wrong:** If `pmcp::Client` is not `Clone` or not `Send + Sync`, it cannot be shared across `ctx.map()` items which run concurrently in separate tasks.
**Why it happens:** `ctx.map()` spawns concurrent child contexts. The map closure is `Fn + Send + Sync`.
**How to avoid:** Verify `pmcp::Client` is `Send + Sync` at compile time. If not, create fresh connections inside each map item (connect-per-use fallback). Alternatively, use `Arc<Mutex<Client>>` but this serializes tool calls.
**Warning signs:** Compile errors about `Send` or `Sync` bounds on the map closure.

### Pitfall 6: Step Functions Output Compatibility
**What goes wrong:** The caller expects the Step Functions `LLMResponse` JSON shape. If the durable agent returns a different shape (e.g., wrapped in an envelope, missing fields, different field names), the caller breaks.
**Why it happens:** The durable agent's response types were adapted from the Step Functions agent but may have diverged (e.g., added `Deserialize` derive, changed `skip_serializing_if` behavior).
**How to avoid:** Write a test that serializes the agent response and asserts the JSON shape matches the Step Functions format. Verify field names and nesting.
**Warning signs:** Caller integration failures after switching from Step Functions to durable agent.

## Code Examples

### Building LLMInvocation from Config and Messages

```rust
fn build_llm_invocation(
    config: &AgentConfig,
    messages: &[UnifiedMessage],
    tools: &[UnifiedTool],
) -> LLMInvocation {
    // Prepend system prompt as a "system" role message.
    // The AnthropicTransformer.extract_system_prompt() extracts the first
    // message with role "system" and maps it to the API's top-level `system`
    // field. transform_messages() then filters it out of the messages array.
    // VERIFIED: see llm/transformers/anthropic.rs lines 175-199, 206-208.
    let mut all_messages = Vec::with_capacity(messages.len() + 1);
    all_messages.push(UnifiedMessage {
        role: "system".to_string(),
        content: MessageContent::Text {
            content: config.system_prompt.clone(),
        },
    });
    all_messages.extend_from_slice(messages);

    LLMInvocation {
        provider_config: config.provider_config.clone(),
        messages: all_messages,
        tools: if tools.is_empty() { None } else { Some(tools.to_vec()) },
        temperature: Some(config.parameters.temperature),
        max_tokens: Some(config.parameters.max_tokens as i32),
        top_p: None,
        stream: None,
    }
}
```

**RESOLVED:** The `system_prompt` from `AgentConfig` is passed as a `UnifiedMessage` with `role: "system"` prepended to the messages array. The Anthropic transformer's `extract_system_prompt()` method checks if the first message has `role == "system"` and extracts it to the API's top-level `system` field. The `transform_messages()` method then skips system-role messages. This is verified from `llm/transformers/anthropic.rs` lines 175-199 and 206-208. No changes to `LLMInvocation` are needed.

### Converting LLMResponse to Assistant Message

```rust
fn llm_response_to_assistant_message(response: &LLMResponse) -> UnifiedMessage {
    UnifiedMessage {
        role: "assistant".to_string(),
        content: MessageContent::Blocks {
            content: response.message.content.clone(),
        },
    }
}
```

### Building Tool Results User Message

```rust
fn build_tool_results_message(results: Vec<ToolCallResult>) -> UnifiedMessage {
    let blocks: Vec<ContentBlock> = results
        .into_iter()
        .map(|r| ContentBlock::ToolResult {
            tool_use_id: r.tool_use_id,
            content: r.content,
            is_error: if r.is_error { Some(true) } else { None },
        })
        .collect();

    UnifiedMessage {
        role: "user".to_string(),
        content: MessageContent::Blocks { content: blocks },
    }
}
```

### ExponentialBackoff Configuration for LLM Calls

```rust
use lambda_durable_execution_rust::retry::ExponentialBackoff;
use lambda_durable_execution_rust::prelude::*;

let retry = ExponentialBackoff::builder()
    .max_attempts(3)
    .initial_delay(Duration::seconds(2))
    .max_delay(Duration::seconds(30))
    .backoff_rate(2.0)
    // Optionally filter retryable errors:
    // .retryable_pattern("rate limit")
    // .retryable_pattern("overloaded")
    .build();

let step_config = StepConfig::<LLMResponse>::new()
    .with_retry_strategy(Arc::new(retry));
```

## State of the Art

| Old Approach (Architecture Research) | Current Approach (CONTEXT.md Decisions) | When Changed | Impact |
|--------------------------------------|----------------------------------------|--------------|--------|
| Connect-per-use MCP inside step closures | Cache MCP clients per invocation (D-03) | CONTEXT.md discussion | Simpler code, less connection overhead, cached clients reused |
| Put entire loop in top-level (no child contexts) | `run_in_child_context` per iteration (D-08) | CONTEXT.md discussion | Replay determinism guaranteed regardless of tool call count variation |
| Sequential tool calls via step() | Parallel tool calls via map() (D-10) | CONTEXT.md discussion | Handles multi-tool turns correctly, per-item checkpointing |

## Open Questions

1. **pmcp::Client Send + Sync bounds**
   - What we know: `pmcp::Client<StreamableHttpTransport>` is used by `discover_all_tools` successfully in non-concurrent contexts.
   - What's unclear: Whether `Client` is `Send + Sync` for sharing across `ctx.map()` items which run concurrently.
   - Recommendation: Test at compile time early in implementation. If it fails, fall back to connect-per-use inside map items (create fresh connection per tool call).

2. **IterationResult serialization size**
   - What we know: Each iteration result contains `LLMResponse` (2-10KB) plus optional tool results.
   - What's unclear: Whether the child context checkpoint for a single iteration could exceed 256KB with large tool results.
   - Recommendation: For v1, accept the risk. Large iterations would require custom `Serdes` compression.

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | cargo test (Rust built-in + tokio::test) |
| Config file | `examples/Cargo.toml` (dev-dependencies section) |
| Quick run command | `cargo test --manifest-path examples/Cargo.toml --all-targets` |
| Full suite command | `cargo test --manifest-path examples/Cargo.toml --all-targets && cargo clippy --manifest-path examples/Cargo.toml --all-targets --all-features -D warnings` |

### Phase Requirements -> Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| LOOP-01 | Agent loop calls LLM, checks tool_use, executes tools, repeats | unit | `cargo test --manifest-path examples/Cargo.toml --bin mcp_agent -- handler::tests::test_loop` | Wave 0 |
| LOOP-02 | LLM call step with ExponentialBackoff retry | unit | `cargo test --manifest-path examples/Cargo.toml --bin mcp_agent -- handler::tests::test_llm_retry` | Wave 0 |
| LOOP-03 | Parallel tool execution via map | unit | `cargo test --manifest-path examples/Cargo.toml --bin mcp_agent -- handler::tests::test_parallel_tools` | Wave 0 |
| LOOP-04 | Child context per iteration | unit | `cargo test --manifest-path examples/Cargo.toml --bin mcp_agent -- handler::tests::test_child_context_isolation` | Wave 0 |
| LOOP-05 | Message history rebuilt from step results | unit | `cargo test --manifest-path examples/Cargo.toml --bin mcp_agent -- handler::tests::test_message_assembly` | Wave 0 |
| LOOP-06 | Max iterations guard | unit | `cargo test --manifest-path examples/Cargo.toml --bin mcp_agent -- handler::tests::test_max_iterations` | Wave 0 |
| LOOP-07 | Final response matches Step Functions format | unit | `cargo test --manifest-path examples/Cargo.toml --bin mcp_agent -- types::tests::test_output_format` | Wave 0 |
| MCP-04 | Tool execution via call_tool | unit | `cargo test --manifest-path examples/Cargo.toml --bin mcp_agent -- handler::tests::test_tool_execution` | Wave 0 |
| MCP-05 | MCP errors passed to LLM | unit | `cargo test --manifest-path examples/Cargo.toml --bin mcp_agent -- handler::tests::test_mcp_error_propagation` | Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test --manifest-path examples/Cargo.toml --all-targets`
- **Per wave merge:** `cargo test --manifest-path examples/Cargo.toml --all-targets && cargo clippy --manifest-path examples/Cargo.toml --all-targets --all-features -D warnings && cargo fmt --manifest-path examples/Cargo.toml --check`
- **Phase gate:** Full suite green before `/gsd:verify-work`

### Wave 0 Gaps
- [ ] `examples/src/bin/mcp_agent/handler.rs` -- handler module with `#[cfg(test)] mod tests` block
- [ ] `examples/src/bin/mcp_agent/types.rs` -- request/response types with `#[cfg(test)] mod tests` block
- [ ] Test helpers for mock LLM responses and mock tool results (no external service calls in unit tests)

**Testing note:** The handler tests cannot use a real `DurableContextHandle` (requires the Lambda durable execution runtime). Tests should validate:
1. Message assembly logic (pure functions, no ctx needed)
2. Tool result conversion (pure functions)
3. Output format serialization (serde round-trip)
4. Error classification (MCP errors vs transport errors)
5. LLM invocation building (pure function)

The actual durable loop integration (ctx.step, ctx.map, run_in_child_context) is verified via `cargo build` (type checking) and end-to-end deployment testing (Phase 5).

## Sources

### Primary (HIGH confidence)
- `examples/src/bin/mcp_agent/llm/service.rs` -- UnifiedLLMService.process() API, Clone-able
- `examples/src/bin/mcp_agent/llm/models.rs` -- LLMInvocation, LLMResponse, FunctionCall, ContentBlock, UnifiedMessage types
- `examples/src/bin/mcp_agent/llm/transformers/anthropic.rs` -- extract_system_prompt() and transform_messages() for system prompt handling
- `examples/src/bin/mcp_agent/config/types.rs` -- AgentConfig, AgentParameters (max_iterations, temperature, max_tokens)
- `examples/src/bin/mcp_agent/config/loader.rs` -- load_agent_config(table_name, agent_name, version) API
- `examples/src/bin/mcp_agent/mcp/client.rs` -- discover_all_tools, resolve_tool_call, connect_and_discover_parsed
- `examples/src/bin/mcp_agent/mcp/types.rs` -- ToolsWithRouting { tools, routing }
- `src/context/durable_context/step.rs` -- ctx.step() signature: `step<T, F, Fut>(name, step_fn, config) -> DurableResult<T>`
- `src/context/durable_context/map.rs` -- ctx.map() signature: `map<TIn, TOut, F, Fut>(name, items, map_fn, config) -> DurableResult<BatchResult<TOut>>`
- `src/context/durable_context/child.rs` -- ctx.run_in_child_context() signature and usage
- `src/retry/strategy.rs` -- ExponentialBackoff::builder() API
- `src/types/config/step.rs` -- StepConfig::new().with_retry_strategy()
- `src/types/batch.rs` -- BatchResult<T>.values() and error handling
- `src/runtime/handler.rs` -- with_durable_execution_service(handler, config) where handler: Fn(E, ctx) -> Fut
- `examples/src/bin/map_operations/main.rs` -- ctx.map() usage pattern
- `examples/src/bin/child_context/main.rs` -- run_in_child_context usage pattern
- `~/projects/step-functions-agent/lambda/call_llm_rust/src/models.rs` -- Step Functions LLMResponse output format
- pmcp crate source (1.10.3) -- `Client.call_tool(name: String, arguments: Value) -> Result<CallToolResult>`
- pmcp crate source -- `CallToolResult { content: Vec<Content>, is_error: bool }`

### Secondary (MEDIUM confidence)
- `.planning/research/ARCHITECTURE.md` -- Agent loop data flow patterns (pre-CONTEXT.md decisions; some patterns overridden by D-03)
- `.planning/research/PITFALLS.md` -- Operation ID determinism, message ordering, error distinction

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- all dependencies already in Cargo.toml, no new crates
- Architecture: HIGH -- all APIs verified from source code, patterns proven by existing examples
- Pitfalls: HIGH -- verified against SDK source (step.rs, map.rs, child.rs) and prior research
- Output format: HIGH -- Step Functions models.rs read directly, identical to durable agent LLMResponse

**Research date:** 2026-03-23
**Valid until:** 2026-04-23 (stable -- no external dependencies changing)
