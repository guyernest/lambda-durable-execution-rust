# Architecture Patterns

**Domain:** Durable Lambda MCP Agent
**Researched:** 2026-03-23

## Recommended Architecture

The agent is a single Durable Lambda binary that implements a classic LLM agent loop (call LLM, execute tools, repeat) using the durable execution SDK for checkpointing. Each LLM call and each batch of tool executions is a durable `step()`, making the entire agent loop replay-safe across Lambda suspensions and restarts.

### High-Level Structure

```
                                    +----------------------------------+
                                    |        Agent Lambda Binary       |
                                    |                                  |
                                    |  +----------------------------+  |
+----------------+                  |  |     Agent Handler          |  |
| AgentRegistry  |   DynamoDB Get   |  |                            |  |
| (DynamoDB)     | <--------------> |  |  1. Load config (step)     |  |
+----------------+                  |  |  2. Connect MCP (step)     |  |
                                    |  |  3. Agent Loop:            |  |
+----------------+                  |  |     a. Call LLM (step)     |  |
| Anthropic API  | <--------------> |  |     b. If tool_use:        |  |
| (Claude)       |   HTTPS          |  |        Execute tools (map) |  |
+----------------+                  |  |     c. Append results      |  |
                                    |  |     d. Goto 3a             |  |
+----------------+                  |  |  4. Return final response  |  |
| MCP Servers    | <--------------> |  |                            |  |
| (HTTP/SSE)     |   MCP Protocol   |  +----------------------------+  |
+----------------+                  |                                  |
                                    |  +----------------------------+  |
                                    |  | Durable Execution SDK      |  |
                                    |  | (step, map, checkpoint)    |  |
                                    |  +----------------------------+  |
                                    +----------------------------------+
                                                    |
                                        Checkpoint API calls
                                                    |
                                    +----------------------------------+
                                    | AWS Durable Execution            |
                                    | Control Plane                    |
                                    +----------------------------------+
```

### Component Boundaries

| Component | Responsibility | Communicates With | Crate/Module |
|-----------|---------------|-------------------|--------------|
| **Agent Handler** | Orchestrates the agent loop: config loading, MCP connection, LLM calling, tool execution, message history management | All other components | `examples/src/bin/mcp_agent/handler.rs` |
| **Config Loader** | Reads agent configuration from AgentRegistry DynamoDB table (system prompt, model, MCP server endpoints, parameters) | DynamoDB via AWS SDK | `examples/src/bin/mcp_agent/config.rs` |
| **MCP Client Manager** | Connects to MCP servers, discovers tools via `list_tools()`, executes tools via `call_tool()`, translates MCP tool schemas to Claude API format | MCP servers via pmcp `HttpTransport` | `examples/src/bin/mcp_agent/mcp.rs` |
| **LLM Caller** | Builds Anthropic Messages API requests, sends them, parses responses including `tool_use` content blocks | Anthropic API via HTTPS | `examples/src/bin/mcp_agent/llm.rs` |
| **Message History** | Manages the conversation state (user/assistant/tool_result messages), serializes for checkpointing | Agent Handler (in-memory) | `examples/src/bin/mcp_agent/messages.rs` |
| **Durable Execution SDK** | Provides `step()` and `map()` for replay-safe checkpointed operations | AWS Durable Execution Control Plane | `lambda-durable-execution-rust` (this crate) |

### Data Flow

#### Initial Invocation

```
1. AWS Control Plane invokes Lambda with DurableExecutionInvocationInput
   containing AgentRequest { user_message, agent_name, agent_version }

2. handler() begins:

   2a. ctx.step("load-config") -> AgentConfig
       - Read AgentRegistry DynamoDB item
       - Returns: system_prompt, model, mcp_servers[], parameters
       - Checkpointed: config is stable for this execution

   2b. ctx.step("discover-tools") -> Vec<ClaudeTool>
       - For each MCP server in config:
         - Connect via HttpTransport
         - Call list_tools()
         - Translate MCP tool schemas to Claude API tool format
       - Returns: merged tool list from all servers
       - Checkpointed: tool schemas are stable for this execution

   2c. Initialize message_history = [user_message]

   2d. LOOP (iteration i = 0, 1, 2, ...):

       2d-i. ctx.step("llm-call-{i}") -> AssistantMessage
             - Build Messages API request:
               { model, system: system_prompt, messages: message_history, tools }
             - POST to Anthropic API
             - Returns: AssistantMessage with content blocks
             - Checkpointed: LLM response is deterministic on replay

       2d-ii. Parse assistant response:
              - If stop_reason == "end_turn" -> break loop, return final text
              - If stop_reason == "tool_use" -> extract tool_use blocks

       2d-iii. Append assistant message to message_history

       2d-iv. ctx.map("tools-{i}", tool_calls, execute_tool) -> Vec<ToolResult>
              - For each tool_use block:
                - Determine which MCP server owns the tool
                - Connect to MCP server (fresh connection per map item)
                - call_tool(name, arguments)
                - Return ToolResult { tool_use_id, content }
              - Checkpointed per-item: each tool result is individually replay-safe

       2d-v. Append tool_result user message to message_history

3. Return final text response as execution result
```

#### Replay Invocation

```
1. AWS Control Plane re-invokes Lambda with updated initial_execution_state
   containing all previously checkpointed operations

2. handler() re-runs FROM THE BEGINNING:

   2a. ctx.step("load-config") -> Cache hit! Returns immediately from replay data
   2b. ctx.step("discover-tools") -> Cache hit! Returns immediately
   2c. Initialize message_history (from event, same as before)
   2d. LOOP replays:
       - ctx.step("llm-call-0") -> Cache hit!
       - ctx.map("tools-0", ...) -> Cache hit (all items)!
       - ctx.step("llm-call-1") -> Cache hit!
       - ctx.map("tools-1", ...) -> PARTIAL: items 0,1 cached, item 2 executes fresh
       - ctx.step("llm-call-2") -> NEW: executes fresh, checkpoints

   Each step that hits cache returns instantly; execution resumes
   exactly where it left off.
```

### Replay Safety Analysis

This is the most architecturally critical section. The durable execution model re-runs the handler from scratch on every invocation, replaying cached results for previously completed operations. Side effects must only happen inside `step()` closures.

#### What is Replay-Safe

| Operation | Replay Behavior | Notes |
|-----------|----------------|-------|
| `ctx.step("load-config", \| \| { dynamodb.get_item() })` | Cached on replay | Config read once, reused on all replays |
| `ctx.step("discover-tools", \| \| { mcp.list_tools() })` | Cached on replay | Tool schemas read once per execution |
| `ctx.step("llm-call-N", \| \| { anthropic.messages() })` | Cached on replay | LLM response checkpointed; identical result on replay |
| `ctx.map("tools-N", calls, \| call \| { mcp.call_tool() })` | Per-item caching | Each tool call individually checkpointed |
| Message history construction from step results | Deterministic | Rebuilt identically from cached step outputs |

#### What is NOT Replay-Safe (and must be avoided)

| Anti-pattern | Why Dangerous | Correct Alternative |
|--------------|--------------|---------------------|
| Connecting to MCP servers outside `step()` | Connection created on every replay, wasting time/resources | Connect inside `step()` or lazily inside tool execution steps |
| Reading config outside `step()` | DynamoDB read on every replay; config could change between invocations | Wrap in `step()` to checkpoint |
| Building message history from external state | Could differ between replays, breaking determinism | Build only from checkpointed step results |
| Using `Instant::now()` or randomness in handler body | Non-deterministic across replays | Use only inside `step()` closures |

#### MCP Connection Strategy: Connect-Per-Use (Recommended)

MCP client connections (`HttpTransport`) are stateful TCP connections. They cannot survive Lambda suspension. Three strategies were considered:

**Option A: Connect once at handler start (rejected)**
- Connection created before any step, outside durable context
- Connection dies on Lambda suspend; reconnection logic needed on every replay
- Tool execution would fail if connection stale

**Option B: Connect once in a step, pass connection through loop (rejected)**
- Connection checkpointed as "completed" but the actual TCP socket is dead on replay
- `step()` returns the cached *result*, not the live connection
- Would need to deserialize a connection (impossible)

**Option C: Connect-per-use inside step closures (recommended)**
- Each `step("discover-tools")` creates fresh MCP connections, calls `list_tools()`, and returns serializable tool schemas
- Each tool execution step in `ctx.map()` creates a fresh connection, calls `call_tool()`, and returns the serializable result
- Connections are ephemeral; only serializable results are checkpointed
- On replay, cached results are returned directly (no connection needed)
- On fresh execution, a new connection is created (always valid)

This matches how the existing SDK examples handle external calls: the side effect (HTTP request) happens inside `step()`, and only the serializable result is checkpointed.

**Performance note:** HTTP/SSE MCP connections to other Lambdas are cheap to establish (same VPC, low latency). The overhead of reconnecting per-use is negligible compared to LLM call latency. If profiling shows connection overhead matters, a connection cache local to a single invocation (not checkpointed) can be added without architectural changes.

### Message History: Serialization and Checkpoint Limits

Message history grows with each iteration of the agent loop. Each LLM response and each set of tool results adds to it. The 750KB checkpoint batch limit and 256KB per-operation payload limit are constraints.

**Strategy:** Message history is not stored as a single monolithic checkpoint. Instead, it is *reconstructed* from individual step results during replay:

```
message_history = [initial_user_message]
for each iteration:
    llm_response = ctx.step("llm-call-{i}", ...).await?   // cached on replay
    message_history.push(assistant_message_from(llm_response))

    tool_results = ctx.map("tools-{i}", ...).await?        // cached on replay
    message_history.push(tool_result_message_from(tool_results))
```

Each individual step caches only its own output (one LLM response, or one tool result). The full history is rebuilt deterministically from these cached pieces. This distributes checkpoint storage across many small operations rather than one growing blob.

**Limit analysis for a 10-iteration agent run:**
- Each LLM response: ~2-10KB typical (model output)
- Each tool result: varies, but typically 1-50KB per tool
- With 10 iterations and 3 tools per iteration: ~30 checkpointed operations
- Total: well within 750KB batch limits per checkpoint call

**If a single LLM response exceeds 256KB:** The SDK already handles large payloads (see `CHECKPOINT_SIZE_LIMIT_BYTES` in `durable_context/mod.rs`). The `Serdes` trait can be used to compress large payloads, and the checkpoint manager batches operations to stay within 750KB per batch. For extremely long conversations, a `max_iterations` guard prevents unbounded growth.

### Anthropic Messages API Data Model

The agent needs these core types. They should be defined in the agent binary, not in the SDK crate.

```rust
// Request
struct MessagesRequest {
    model: String,
    max_tokens: u32,
    system: Option<String>,
    messages: Vec<Message>,
    tools: Option<Vec<Tool>>,
}

struct Message {
    role: String,           // "user" or "assistant"
    content: Vec<ContentBlock>,
}

enum ContentBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, content: String, is_error: Option<bool> },
}

// Response
struct MessagesResponse {
    content: Vec<ContentBlock>,
    stop_reason: String,    // "end_turn" or "tool_use"
    usage: Usage,
}

// Tool definition (Claude API format)
struct Tool {
    name: String,
    description: Option<String>,
    input_schema: serde_json::Value,  // JSON Schema object
}
```

### MCP-to-Claude Tool Schema Translation

MCP `list_tools()` returns tools with JSON Schema `inputSchema`. The Claude API expects `input_schema` in the same JSON Schema format. The translation is straightforward:

```rust
fn mcp_tool_to_claude_tool(mcp_tool: &McpTool) -> Tool {
    Tool {
        name: mcp_tool.name.clone(),
        description: mcp_tool.description.clone(),
        input_schema: mcp_tool.input_schema.clone(),  // Already JSON Schema
    }
}
```

The field names differ (`inputSchema` in MCP vs `input_schema` in Claude API) but the content is identical JSON Schema. No deep transformation needed -- just a struct mapping.

**Tool routing:** When the LLM returns a `tool_use` block, the agent needs to know which MCP server to call. Maintain a `HashMap<String, McpServerConfig>` mapping tool names to their origin server. Built during `discover-tools` step, reconstructable from cached tool discovery results.

## Patterns to Follow

### Pattern 1: Durable Agent Loop

The core pattern: a `loop` with durable steps for LLM calls and `map` for tool execution.

```rust
async fn agent_handler(
    event: AgentRequest,
    ctx: DurableContextHandle,
) -> DurableResult<AgentResponse> {
    // Phase 1: Setup (checkpointed)
    let config: AgentConfig = ctx
        .step(Some("load-config"), |_| async move {
            load_agent_config(&event.agent_name, &event.agent_version).await
        }, None)
        .await?;

    let tools_with_routing: ToolsWithRouting = ctx
        .step(Some("discover-tools"), |_| async move {
            discover_all_tools(&config.mcp_servers).await
        }, None)
        .await?;

    // Phase 2: Agent loop
    let mut messages: Vec<Message> = vec![user_message(&event.user_message)];
    let max_iterations = config.parameters.max_iterations.unwrap_or(10);

    for i in 0..max_iterations {
        // LLM call (checkpointed)
        let llm_response: MessagesResponse = ctx
            .step(Some(&format!("llm-call-{i}")), |_| {
                let req = build_request(&config, &messages, &tools_with_routing.tools);
                async move { call_anthropic(req).await }
            }, None)
            .await?;

        messages.push(assistant_message_from(&llm_response));

        if llm_response.stop_reason == "end_turn" {
            return Ok(extract_final_response(&llm_response));
        }

        // Tool execution (checkpointed per-tool via map)
        let tool_calls = extract_tool_uses(&llm_response);
        let routing = tools_with_routing.routing.clone();

        let results: BatchResult<ToolResult> = ctx
            .map(
                Some(&format!("tools-{i}")),
                tool_calls,
                move |call, item_ctx, idx| {
                    let routing = routing.clone();
                    async move {
                        item_ctx.step(
                            Some(&format!("tool-{}", call.name)),
                            |_| async move {
                                execute_mcp_tool(&call, &routing).await
                            },
                            None,
                        ).await
                    }
                },
                None,
            )
            .await?;

        messages.push(tool_results_message(results.values()));
    }

    Err(DurableError::Internal("Max iterations exceeded".into()))
}
```

**Why this works:**
- Step names include iteration index (`llm-call-0`, `llm-call-1`) ensuring unique deterministic IDs
- `messages` is rebuilt from step results on replay (each `step()` returns the cached value)
- Tool calls fan out via `map()`, giving per-tool checkpointing and bounded concurrency
- MCP connections are ephemeral (created inside step closures, not persisted)

### Pattern 2: Config Loading with Checkpointing

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentConfig {
    system_prompt: String,
    model: String,
    mcp_servers: Vec<McpServerEndpoint>,
    parameters: AgentParameters,
}

async fn load_agent_config(
    agent_name: &str,
    agent_version: &str,
) -> Result<AgentConfig, Box<dyn Error + Send + Sync>> {
    let sdk_config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let ddb = aws_sdk_dynamodb::Client::new(&sdk_config);

    let result = ddb.get_item()
        .table_name("AgentRegistry")
        .key("agent_name", AttributeValue::S(agent_name.into()))
        .key("version", AttributeValue::S(agent_version.into()))
        .send()
        .await?;

    // Parse DynamoDB item into AgentConfig
    parse_agent_config(result.item())
}
```

### Pattern 3: Tool Discovery with Server-to-Tool Routing

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ToolsWithRouting {
    tools: Vec<Tool>,                         // Claude API format
    routing: HashMap<String, McpServerEndpoint>, // tool_name -> server
}

async fn discover_all_tools(
    servers: &[McpServerEndpoint],
) -> Result<ToolsWithRouting, Box<dyn Error + Send + Sync>> {
    let mut tools = Vec::new();
    let mut routing = HashMap::new();

    for server in servers {
        let transport = HttpTransport::new(&server.url)?;
        let client = Client::new(transport).await?;
        let server_tools = client.list_tools().await?;

        for mcp_tool in server_tools {
            routing.insert(mcp_tool.name.clone(), server.clone());
            tools.push(mcp_tool_to_claude_tool(&mcp_tool));
        }
    }

    Ok(ToolsWithRouting { tools, routing })
}
```

### Pattern 4: MCP Tool Execution with Fresh Connections

```rust
async fn execute_mcp_tool(
    call: &ToolUseBlock,
    routing: &HashMap<String, McpServerEndpoint>,
) -> Result<ToolResult, Box<dyn Error + Send + Sync>> {
    let server = routing.get(&call.name)
        .ok_or_else(|| format!("Unknown tool: {}", call.name))?;

    // Fresh connection per tool call -- replay-safe
    let transport = HttpTransport::new(&server.url)?;
    let client = Client::new(transport).await?;

    let result = client.call_tool(&call.name, call.input.clone()).await?;

    Ok(ToolResult {
        tool_use_id: call.id.clone(),
        content: result.content_as_text(),
        is_error: result.is_error,
    })
}
```

## Anti-Patterns to Avoid

### Anti-Pattern 1: Shared Mutable State Across Steps

**What:** Storing MCP connections, HTTP clients, or mutable state in the handler scope and referencing them across multiple `step()` calls.

**Why bad:** On replay, `step()` returns cached results without executing the closure. Any side effects on shared state (like updating a connection pool) are skipped. The shared state becomes inconsistent between replay and execution modes.

**Instead:** Each `step()` closure should be self-contained. Create connections inside the closure. Return all needed data as the step result.

### Anti-Pattern 2: Monolithic Message History Checkpoint

**What:** Checkpointing the entire `Vec<Message>` as a single step result after each iteration.

**Why bad:** Message history grows linearly. At 10 iterations with verbose tool results, a single checkpoint could exceed 256KB. Also, this would require a separate step just for history persistence, adding overhead.

**Instead:** Let message history be an emergent property of individual step results. Rebuild it from `step("llm-call-N")` and `map("tools-N")` results during replay.

### Anti-Pattern 3: Non-Deterministic Step Names

**What:** Using timestamps, random IDs, or external state in step names.

**Why bad:** The SDK generates deterministic operation IDs from step names/sequence. If names differ between invocations, replay cannot match operations to cached results. The handler would re-execute already-completed work or fail with mismatched state.

**Instead:** Use iteration indices (`llm-call-0`, `tools-0`) or fixed names (`load-config`, `discover-tools`). The SDK's `next_operation_id()` handles sequence numbering.

### Anti-Pattern 4: Trying to Checkpoint MCP Connections

**What:** Attempting to serialize/deserialize MCP client connections or HTTP transports via `Serdes`.

**Why bad:** TCP connections cannot be serialized. Even if you could serialize connection metadata, the socket is dead after Lambda suspension.

**Instead:** Only checkpoint serializable results (tool schemas, tool call results). Re-create connections when needed for fresh execution.

### Anti-Pattern 5: Putting the Agent Loop Inside `run_in_child_context`

**What:** Wrapping the entire agent loop in a child context.

**Why bad:** Child contexts create a nested operation scope. The agent loop's step names (`llm-call-0`, etc.) would all be scoped under one parent context operation. If the child context fails, all inner operations are lost. Child contexts are for grouping related sub-operations, not for top-level flows.

**Instead:** Run the agent loop directly in the top-level handler. Use `map()` for tool call fan-out (which already uses child contexts internally for each item).

## Suggested Build Order

Based on component dependencies, build in this order:

```
Phase 1: Types + LLM Caller
  - Define message types (Request, Response, ContentBlock, Tool)
  - Implement Anthropic API HTTP client
  - Test: send a message, get a response
  Dependencies: None

Phase 2: MCP Client Integration
  - Integrate pmcp Client with HttpTransport
  - Implement tool discovery (list_tools)
  - Implement tool execution (call_tool)
  - Implement MCP-to-Claude tool schema translation
  - Test: connect to MCP server, discover tools, call a tool
  Dependencies: Phase 1 types (Tool struct)

Phase 3: Config Loader
  - Define AgentConfig, McpServerEndpoint types
  - Implement DynamoDB reader
  - Test: load config from DynamoDB
  Dependencies: None (can parallelize with Phase 1-2)

Phase 4: Agent Handler (integration)
  - Wire all components into durable handler
  - Implement the agent loop with step() and map()
  - Implement message history management
  - Test: end-to-end with mock MCP server and mock Anthropic API
  Dependencies: Phases 1, 2, 3

Phase 5: Deployment
  - Add to SAM template
  - Configure DurableConfig, permissions (DynamoDB, Secrets Manager, VPC)
  - Deploy and validate with real MCP servers
  Dependencies: Phase 4
```

**Critical path:** Phase 1 -> Phase 2 -> Phase 4. Phase 3 is parallelizable.

## Scalability Considerations

| Concern | At 1 agent | At 10 agents | At 100+ agents |
|---------|------------|--------------|----------------|
| MCP connections | Ephemeral per-use, trivial | Same pattern, no pooling needed | Consider connection reuse within a single invocation |
| Checkpoint size | ~50-200KB per iteration | Same (per-execution isolation) | Same (each execution is independent) |
| DynamoDB reads | 1 read per execution (cached by step) | 10 reads total (each cached) | Use DynamoDB DAX if read throughput matters |
| LLM API rate limits | Not an issue | May approach limits | Token bucket / backoff via step retry strategies |
| Message history size | 10 iterations ~50KB | Same | Add max_tokens guard + conversation summarization step |
| Lambda concurrent executions | Default 1000 | Sufficient | Request limit increase; each agent is one concurrent execution |

## Binary Structure Decision

**Recommended: Example binary in this repo** (matching PROJECT.md decision)

Place the agent binary at `examples/src/bin/mcp_agent/` with this structure:

```
examples/src/bin/mcp_agent/
  main.rs           -- Lambda entry point, with_durable_execution_service()
  handler.rs        -- agent_handler() function with the durable loop
  config.rs         -- AgentConfig, DynamoDB loader
  llm.rs            -- Anthropic API client, request/response types
  mcp.rs            -- MCP client wrapper, tool discovery, tool execution
  messages.rs       -- Message history types, Anthropic message format
  types.rs          -- Shared types (AgentRequest, AgentResponse, ToolResult)
```

This follows the existing example binary pattern (see `map_operations/main.rs`, `child_context/main.rs`) and keeps the PoC close to the SDK. The binary depends on:
- `lambda-durable-execution-rust` (this crate, via `path = ".."`)
- `pmcp` (MCP SDK, new dependency in `examples/Cargo.toml`)
- `reqwest` (for Anthropic API calls)
- `aws-sdk-dynamodb` (for config loading)
- `aws-sdk-secretsmanager` (for API key retrieval)

## Sources

- SDK architecture: `ARCHITECTURE.md` in this repo (HIGH confidence)
- Execution lifecycle: `src/runtime/handler/execute.rs` (HIGH confidence)
- Step replay mechanism: `src/context/durable_context/step/replay.rs` (HIGH confidence)
- Step execution flow: `src/context/durable_context/step/execute.rs` (HIGH confidence)
- Map pattern: `src/context/durable_context/map.rs`, `examples/src/bin/map_operations/main.rs` (HIGH confidence)
- Parallel pattern: `examples/src/bin/parallel/main.rs` (HIGH confidence)
- Checkpoint limits: `src/checkpoint/manager.rs` (750KB batch, 256KB per-op) (HIGH confidence)
- SAM deployment pattern: `examples/template.yaml` (HIGH confidence)
- Anthropic Messages API format: training data (MEDIUM confidence -- API is stable and well-known, but verify exact field names against current docs when implementing)
- MCP protocol tool schema format: training data (MEDIUM confidence -- JSON Schema-based `inputSchema` is core MCP spec, but verify pmcp v2.0.0 Client API when implementing)
- pmcp crate API (`Client`, `HttpTransport`, `list_tools`, `call_tool`): PROJECT.md description (LOW confidence -- could not read pmcp source directly; verify API when integrating)
