# Phase 3: Agent Loop - Context

**Gathered:** 2026-03-23
**Status:** Ready for planning

<domain>
## Phase Boundary

Wire the complete durable agent loop: load config, connect to MCP servers, then enter the agentic loop (call LLM → execute tool calls → append results → repeat until end_turn or max iterations). This is the handler function that ties together the LLM client (Phase 1) and config/MCP modules (Phase 2) using the durable execution SDK's `ctx.step()`, `ctx.map()`, and `run_in_child_context`.

</domain>

<decisions>
## Implementation Decisions

### Handler input/output
- **D-01:** Input payload is `{ agent_name: String, version: String, messages: Vec<Message> }`. Caller specifies which agent and the initial conversation. The handler loads config from AgentRegistry using agent_name/version.
- **D-02:** Output format matches the existing Step Functions agent response format for caller compatibility (drop-in replacement). The handler should read the Step Functions agent output format and replicate it.

### MCP client connection strategy
- **D-03:** Behave like a standard MCP client — establish connections to all configured MCP servers once at handler start, cache the clients, and reuse them for all tool calls within the invocation. This happens OUTSIDE durable steps (alongside tool discovery, which is already outside steps per D-07/P2).
- **D-04:** MCP tool calls inside `ctx.step()` or `ctx.map()` are regular async HTTP calls over the cached connections. The Lambda stays active during tool execution — no suspension. TCP connections survive across loop iterations within the same invocation.
- **D-05:** On Lambda resume (after suspension/replay), MCP clients are re-established at handler start before the loop replays. Cached step results are returned without re-connecting.

### Durable execution model clarification
- **D-06:** Lambda only suspends for explicit `ctx.wait()` / `ctx.wait_for_callback()` / retry backoff. A long-running `ctx.step()` (e.g., slow MCP tool call) keeps the Lambda active while the async task completes. No idle cost concern for calls under the Lambda timeout.
- **D-07:** For truly long-running tool calls (minutes), the current approach works — Lambda waits. Using `ctx.wait_for_callback()` to avoid Lambda idle time during long tool calls is a v2 pattern.

### Agent loop structure (constrained by SDK + research)
- **D-08:** Each loop iteration uses `run_in_child_context` to isolate operation ID counters for replay determinism (per Pitfall 1 from research).
- **D-09:** Each LLM call is a `ctx.step()` with `ExponentialBackoff` retry. The step result (LLMResponse) is cached — on replay, the LLM is NOT re-called.
- **D-10:** Tool calls within an iteration are executed via `ctx.map()` for parallel execution. Even though most turns have single tool calls, `ctx.map()` handles both single and multiple correctly.
- **D-11:** Message history is assembled incrementally from step results. Each iteration's LLM response and tool results are step outputs that rebuild naturally during replay.
- **D-12:** MCP tool errors (isError: true from `call_tool()`) are passed to the LLM as error `tool_result` messages — the agent does not fail, the LLM decides recovery. Transport errors (connection failure) propagate as step errors.

### Claude's Discretion
- Exact handler function structure and wiring
- How to extract the Step Functions output format (read the existing agent code or ask)
- Error types for agent-level failures (handler errors vs tool errors vs LLM errors)
- Test strategy (mock durable context, mock MCP responses, mock LLM responses)
- How to structure the message assembly (Vec<UnifiedMessage> accumulation pattern)
- Whether max_iterations check happens at loop start or after LLM response

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Agent modules (Phase 1 + 2 output)
- `examples/src/bin/mcp_agent/llm/service.rs` — UnifiedLLMService.process() for LLM calls
- `examples/src/bin/mcp_agent/llm/models.rs` — LLMInvocation, LLMResponse, FunctionCall, ContentBlock, UnifiedMessage types
- `examples/src/bin/mcp_agent/config/loader.rs` — load_agent_config, map_provider_config
- `examples/src/bin/mcp_agent/config/types.rs` — AgentConfig, AgentParameters
- `examples/src/bin/mcp_agent/mcp/client.rs` — discover_all_tools, resolve_tool_call, connect_and_discover_parsed
- `examples/src/bin/mcp_agent/mcp/types.rs` — ToolsWithRouting
- `examples/src/bin/mcp_agent/main.rs` — Entry point to wire handler into

### Durable SDK (handler patterns)
- `src/runtime/handler/execute.rs` — How durable handler execution works (tokio::select racing)
- `src/runtime/handler.rs` — with_durable_execution_service, DurableExecutionConfig
- `src/context/durable_context/step.rs` — ctx.step() API
- `src/context/durable_context/map.rs` — ctx.map() API
- `src/context/durable_context/child.rs` — run_in_child_context API
- `src/retry/strategy.rs` — ExponentialBackoff retry strategy
- `examples/src/bin/map_operations/main.rs` — Map pattern example
- `examples/src/bin/child_context/main.rs` — Child context example

### Step Functions agent (output format reference)
- `~/projects/step-functions-agent/lambda/call_llm_rust/src/models.rs` — LLMResponse output format

### Research
- `.planning/research/ARCHITECTURE.md` — Agent loop data flow, replay safety
- `.planning/research/PITFALLS.md` — Operation ID determinism, message ordering, error distinction

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `UnifiedLLMService` (llm/service.rs): Clone-able, process() takes LLMInvocation returns LLMResponse — ready for use inside ctx.step() closures
- `discover_all_tools` (mcp/client.rs): Returns ToolsWithRouting with tools + routing map — ready for tool discovery at handler start
- `resolve_tool_call` (mcp/client.rs): Maps prefixed tool name to (server_url, original_name) — ready for routing inside ctx.map()
- `AgentConfig` (config/types.rs): Serialize + Deserialize — ready for ctx.step() caching
- `clean_tool_schema` (llm/transformers/utils.rs): Normalizes MCP schemas to Claude API format
- Existing example handlers (hello_world, map_operations, child_context): Patterns for with_durable_execution_service integration

### Established Patterns
- Handler signature: `async fn(Event, DurableContextHandle) -> DurableResult<Response>`
- Step with retry: `ctx.step(Some("name"), |_| async { ... }, Some(StepConfig::new().with_retry(...)))`
- Map over items: `ctx.map(Some("name"), items, |item, _| async { ... }, None)`
- Child context: `ctx.run_in_child_context(Some("name"), |child_ctx| async { ... }, None)`

### Integration Points
- `main.rs`: Wire handler with `lambda_runtime::run(with_durable_execution_service(handler, config))`
- LLM service: Create outside handler (per D-05/P1), pass via closure capture or Arc
- MCP clients: Create alongside tool discovery at handler start (per D-03)
- Config: Load via `ctx.step("load-config", ...)` at first handler entry

</code_context>

<specifics>
## Specific Ideas

- "Drop-in replacement" for Step Functions agent — match the output format exactly
- Most tool calls are single (sequential reasoning), but parallel via ctx.map() handles both patterns
- Agents are for depth/reasoning, not speed — the overhead of ctx.step() per LLM call is negligible compared to LLM latency
- MCP servers are close (same VPC, Rust), tool calls return quickly except for heavy queries or remote agent triggers

</specifics>

<deferred>
## Deferred Ideas

- `ctx.wait_for_callback()` for long-running MCP tool calls (avoid Lambda idle) — v2 pattern
- Streaming LLM responses — out of scope per PROJECT.md
- Agent-to-agent delegation via `ctx.invoke()` — future capability
- Context window management (summarization, truncation) — Phase 4+ concern

</deferred>

---

*Phase: 03-agent-loop*
*Context gathered: 2026-03-23*
