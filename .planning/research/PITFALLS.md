# Domain Pitfalls

**Domain:** Durable Lambda MCP Agent (replay-safe agentic loop with MCP tool execution and LLM calls)
**Researched:** 2026-03-23
**Confidence:** HIGH (based on direct SDK source code analysis and domain expertise)

## Critical Pitfalls

Mistakes that cause rewrites, data corruption, or execution failures.

### Pitfall 1: Non-Deterministic Branching Outside Durable Steps

**What goes wrong:** The agentic loop reads an LLM response and branches on it (e.g., `if response.has_tool_use() { ... } else { return response }`) _outside_ of a `ctx.step()`. On first run, the LLM returns tool calls so the handler enters the tool-execution branch and checkpoints tool results. On replay, the handler re-runs from the beginning, but the branching logic depends on the LLM response value which was obtained _inside_ a step. If the deserialized response is handled correctly, the branch will be the same. **But** if any logic between steps reads external state, generates random values, or uses time-dependent conditions, the operation counter diverges and every subsequent step ID mismatches its replay data.

**Why it happens:** The SDK's operation ID scheme is `{name}_{counter}` where counter increments globally (see `next_operation_id` in `execution_context.rs`). If the handler takes a different code path on replay vs first run, the counter sequence diverges, and operation IDs no longer match checkpoint data. All subsequent operations appear as "not found" in replay and re-execute instead of returning cached results.

**Consequences:**
- Silent re-execution of already-completed LLM calls (wasted cost, non-idempotent side effects)
- Checkpoint data orphaned (old IDs never matched again)
- If tool calls differ between runs, the agent could produce inconsistent results
- In worst case: infinite loop where the agent keeps re-executing and never makes progress

**Prevention:**
- **Every** piece of state the loop branches on must come from a durable `ctx.step()` result
- The agentic loop structure must be: `step("llm-call-N") -> branch on cached result -> step("tool-call-N-M") -> loop`
- Between steps, the handler must only use values derived from previous step results and the original event
- No `std::time::Instant::now()`, no `uuid::Uuid::new_v4()`, no reading DynamoDB/S3 outside steps
- **Test strategy:** Run the handler twice with the same replay data and assert the operation ID sequence is identical

**Detection:**
- Operations appearing as "Not found" during replay when they should be cached
- Step count increasing on every invocation instead of stabilizing
- LLM API charges on replay invocations (should be zero during pure replay)

**Phase mapping:** Phase 1 (core agent loop design). This is the foundational architectural decision.

---

### Pitfall 2: Checkpoint Payload Size Exceeded by LLM Message History

**What goes wrong:** Each `ctx.step()` checkpoints its result as a JSON string. The per-operation payload limit is **256KB** (`CHECKPOINT_SIZE_LIMIT_BYTES` in `src/context/durable_context/mod.rs`), and the per-batch limit is **750KB** (`MAX_PAYLOAD_SIZE` in `src/checkpoint/manager.rs`). A single Claude API response with a long chain-of-thought or large tool result can easily exceed 256KB. The checkpoint call fails, triggering `TerminationReason::CheckpointFailed` and killing the execution.

**Why it happens:**
- Anthropic Claude responses include full `content` arrays with text blocks, tool_use blocks, and metadata
- The agent accumulates message history: each iteration adds an assistant message + N tool_result messages
- By iteration 5-10 of a complex agent loop, the full message history can easily be 100KB-500KB
- If the entire conversation is checkpointed as the step result (naive approach), it exceeds 256KB quickly
- Even individual tool results from MCP servers can be large (e.g., file contents, search results, database query results)

**Consequences:**
- `TerminationReason::CheckpointFailed` -> execution terminates with error
- No graceful recovery; the execution is dead
- Users see agent failures on complex tasks that require many iterations

**Prevention:**
- **Do not checkpoint the full message history as a single step result.** Instead, checkpoint only the new messages added in each iteration (the LLM response and the tool results from that iteration)
- Reconstruct the full message history during replay by accumulating results from all prior iteration steps
- Use custom `Serdes` implementation to compress or truncate large payloads
- Implement a message history size monitor that warns before hitting limits
- For tool results: consider truncating large outputs before adding to message history (e.g., first 50KB of a file)
- Consider using `run_in_child_context` for each loop iteration -- child contexts handle the `CHECKPOINT_SIZE_LIMIT_BYTES` overflow by setting `replay_children: true` and falling back to replaying child operations instead of storing the full payload

**Detection:**
- `checkpoint batch failed` errors in CloudWatch logs
- Agent consistently failing after N iterations (where N correlates with payload size threshold)
- `TerminationReason::CheckpointFailed` in execution output

**Phase mapping:** Phase 1 (data model design) and Phase 2 (implementation). The message storage strategy must be decided before writing any code.

---

### Pitfall 3: MCP Connection Lifecycle Mismatch with Lambda Invocations

**What goes wrong:** MCP is a stateful protocol: `connect -> initialize -> (capabilities exchange) -> list_tools -> call_tool*`. On each Lambda invocation (including replays), the MCP client must re-establish connections because Lambda invocations are stateless from the SDK's perspective. If MCP connection and tool discovery are done outside `ctx.step()`, they execute on every invocation including replay, adding latency. If done inside `ctx.step()`, the tool schemas are cached but the actual HTTP connection still needs to be live for `call_tool` operations in subsequent steps.

**Why it happens:**
- Lambda durable execution re-invokes the function from scratch on each resume
- MCP HTTP/SSE connections cannot be persisted across invocations
- The MCP `initialize` handshake and `list_tools` discovery add 100-500ms per server
- During replay, all steps before the "new work" frontier are instant (cached), but MCP connections needed by upcoming steps must still be established

**Consequences:**
- Connection overhead on every invocation (cold path: connect + initialize + list_tools per server)
- If an MCP server is down during replay, even cached steps that don't need the server might fail if the agent tries to eagerly connect
- Tool schemas might change between invocations (server updated tools), causing schema mismatch with what the LLM was originally given

**Prevention:**
- **Lazy MCP connections:** Do not connect to MCP servers at handler start. Connect only when `call_tool` is actually needed (not during replay of cached results)
- **Cache tool schemas in a durable step:** `ctx.step("discover-tools", || list_tools_from_all_servers())` so that tool schemas are stable across replays. The LLM always sees the same tool definitions that were discovered on the first invocation
- **Separate connection from discovery:** Tool discovery (schemas) is durable; tool execution (call_tool) uses a fresh connection each time but only when actually executing (not replaying)
- **Connection pooling within a single invocation:** Reuse connections across multiple `call_tool` steps within the same invocation
- **Timeout + retry on connect:** MCP server might be slow to respond; use the SDK's retry strategies for the `call_tool` steps

**Detection:**
- High latency on replay invocations (should be near-zero for cached steps)
- Agent failures when an MCP server has temporary downtime, even during replay
- Tool schema changes causing LLM confusion ("tool X no longer exists")

**Phase mapping:** Phase 2 (MCP client integration). Architecture must separate discovery from execution.

---

### Pitfall 4: Agentic Loop Iteration Count Creates Unbounded Operation Growth

**What goes wrong:** The agentic loop runs N iterations (LLM decides when to stop). Each iteration creates 1 LLM call step + M tool call steps. For a 20-iteration agent run with 3 tools per iteration, that's 80+ durable operations. The control plane returns all operation history on each re-invocation, and the SDK must iterate through all of them to find replay matches. The `initial_execution_state` grows with each invocation, eventually requiring pagination (`next_marker`).

**Why it happens:**
- The operation counter is linear: each step increments it. A typical agent run: `llm-call-0_0`, `tool-0-0_1`, `tool-0-1_2`, `llm-call-1_3`, `tool-1-0_4`, ... `llm-call-19_57`, `tool-19-0_58`, etc.
- The `step_data` HashMap (in-memory) grows with every operation
- Replay time is O(N) where N is the total number of operations (fast, but not free)
- The `initial_execution_state` payload from the control plane grows linearly; at some point `next_marker` pagination kicks in, adding API calls at the start of each invocation

**Consequences:**
- Replay overhead grows with agent complexity (each invocation replays more cached steps)
- Memory pressure from large `step_data` HashMaps
- Cold start time increases: parsing large `initial_execution_state` + potential pagination calls
- Eventually hits control plane limits on total operation count (undocumented but likely exists)

**Prevention:**
- **Use `run_in_child_context` per iteration:** Wrap each loop iteration in a child context. Child context results, once complete, can use `replay_children: false` to skip replaying individual operations within that iteration. This collapses a completed iteration to a single cached result during replay
- **Design the step granularity carefully:** Don't make every micro-operation a step. Group logically related work (e.g., "execute all tool calls for iteration N" as a single `ctx.map()`)
- **Set a reasonable max_iterations limit** in agent configuration (e.g., 25-50)
- **Monitor operation counts** and alert if approaching thresholds

**Detection:**
- Increasing invocation duration as iteration count grows
- `get_durable_execution_state` pagination calls appearing in logs
- Memory usage spikes in CloudWatch metrics

**Phase mapping:** Phase 1 (loop architecture). Child context strategy must be decided upfront.

---

### Pitfall 5: Dynamic Tool Call Count Breaks Operation ID Determinism

**What goes wrong:** In iteration N, the LLM returns 3 tool calls. The handler creates `ctx.step("tool-N-0")`, `ctx.step("tool-N-1")`, `ctx.step("tool-N-2")`. These get IDs `tool-N-0_X`, `tool-N-1_X+1`, `tool-N-2_X+2`. On replay, these are correctly matched. **But** if the loop structure changes -- for example, if the handler generates step names based on tool call content rather than index, or if error handling introduces conditional steps -- the counter sequence diverges.

**More subtly:** Using `ctx.map("tools-N", tool_calls, ...)` for parallel tool execution is safe because map creates child contexts with independent counters (see `with_parent_id` in `execution_context.rs` which resets the counter to 0). But if some iterations use `map` and others use sequential steps based on a runtime condition, the parent-level counter diverges.

**Why it happens:** The SDK's `operation_counter` is a single `AtomicU64` shared across the execution context. Every call to `next_operation_id` increments it. The determinism contract requires that the _sequence_ of calls is identical across invocations. Any conditional logic that changes the number or order of `next_operation_id` calls breaks this.

**Consequences:**
- Steps execute with wrong cached data (type deserialization failures)
- Steps that should be cached re-execute instead
- Hard to debug: the mismatch is silent until a type error or unexpected behavior occurs

**Prevention:**
- **Always use `ctx.map()` for tool calls** regardless of how many there are. Even for a single tool call, use map with a 1-element vector. This isolates each tool call in a child context with its own counter
- **Never conditionally skip or add steps based on runtime values** between iterations. The control flow between steps must be identical on every invocation
- **Use indexed naming:** `tool-{iteration}-{index}` not `tool-{tool_name}` (tool names could change between invocations if schemas change, though if schemas are cached in a step this is less of a risk)
- **Wrap each iteration in `run_in_child_context`** to isolate counter sequences per iteration

**Detection:**
- Deserialization errors during replay ("expected type X but got Y")
- Steps that were previously completed appearing as "new" in logs
- Operation counter values at invocation end not matching expected values

**Phase mapping:** Phase 1 (loop architecture). The map-for-tools pattern must be established in the initial design.

## Moderate Pitfalls

### Pitfall 6: Anthropic Message Format Ordering Violations

**What goes wrong:** The Claude API has strict ordering requirements for messages:
1. Messages must alternate between `user` and `assistant` roles
2. An `assistant` message with `tool_use` content blocks must be immediately followed by a `user` message containing the corresponding `tool_result` content blocks
3. Every `tool_use` block has an `id` that must match exactly one `tool_result` block
4. `tool_result` blocks reference their `tool_use` by `tool_use_id`

If the agent reconstructs message history from checkpointed step results and gets the ordering wrong, the Claude API returns a 400 error.

**Why it happens:**
- Message history is reconstructed from individual step results during replay
- If the reconstruction logic doesn't maintain the exact alternating user/assistant structure, or if tool_result blocks are associated with the wrong tool_use IDs, the API rejects the request
- Particularly tricky when an iteration has parallel tool calls: the tool_results must all appear in a single user message following the assistant message, regardless of execution order

**Prevention:**
- Define a strict message reconstruction protocol: each "iteration result" step stores the assistant response AND the tool results together, preserving the pairing
- Validate message ordering before sending to the API (fail fast with a clear error rather than getting a cryptic 400)
- Use a typed message builder that enforces the alternation invariant at compile time
- Store tool_use IDs alongside tool results in checkpoint data so reconstruction can always pair them correctly

**Detection:**
- 400 errors from the Anthropic API during the LLM call step
- Error messages mentioning "tool_result" ordering or missing tool_use_id
- Agent succeeding on first run but failing on replay (reconstruction bug)

**Phase mapping:** Phase 2 (Anthropic API integration). The message type system should enforce correctness.

---

### Pitfall 7: MCP Server Unavailability During Tool Execution Steps

**What goes wrong:** A `ctx.step("call-tool-X")` calls an MCP server via `call_tool()`. The MCP server is temporarily down (network issue, deployment, cold start). The step fails. Without proper retry configuration, the step permanently fails and the entire agent execution fails. With retry, the Lambda suspends and is re-invoked, but on re-invocation, the step re-executes (for `AtLeastOncePerRetry` semantics, which is the default) and the server might still be down.

**Why it happens:**
- MCP servers are external services with their own availability characteristics
- Lambda-to-Lambda MCP communication involves HTTP/SSE transport, which can timeout
- Default step semantics (`AtLeastOncePerRetry`) re-execute on replay if the step was `Started` but not `Succeeded`
- No built-in circuit breaker for repeated failures to the same server

**Prevention:**
- **Configure retry strategies on tool call steps:** Use `ExponentialBackoff` with reasonable limits (e.g., 3 retries with 1s/2s/4s delays)
- **Use `AtLeastOncePerRetry` semantics** (the default) for tool calls, since they should be retried if interrupted. This is correct for tool calls that are idempotent or safe to retry
- **Distinguish transient from permanent failures:** Network timeouts and 5xx responses should retry; 4xx responses (bad input, tool not found) should not
- **Add a per-server health check** or timeout: if a server is consistently down, fail fast rather than burning retry budget
- **Set MCP client timeouts** shorter than the Lambda timeout to leave room for checkpoint and retry logic

**Detection:**
- Repeated `RETRY` actions in checkpoint history for tool call steps
- Agent executions stuck in retry loops
- CloudWatch showing repeated invocations without progress

**Phase mapping:** Phase 2 (tool execution). Retry configuration should be part of the MCP client setup.

---

### Pitfall 8: Cold Start Amplification from MCP Client Initialization

**What goes wrong:** Each Lambda invocation (including replays) must initialize the Rust binary, set up the MCP client(s), and potentially establish HTTP connections to MCP servers. For N MCP servers, this adds N * (connection_time + initialize_time) to every cold start. Combined with the standard Lambda cold start (binary load, runtime init), this can push total start time to several seconds.

**Why it happens:**
- MCP protocol requires a `connect -> initialize` handshake before any operations
- Each MCP server is a separate HTTP endpoint requiring its own connection
- Lambda durable execution re-invokes the function fresh on each resume (no warm container guarantee, though Lambda does reuse containers when possible)
- The Rust binary itself has fast cold starts (~50-100ms), but HTTP connection setup and TLS negotiation add latency

**Prevention:**
- **Lazy initialization:** Don't connect to all MCP servers at handler start. Build connection objects but defer actual TCP/TLS handshake until first use
- **Parallel connection setup:** When connections are needed, establish them concurrently (`tokio::join!` or similar), not sequentially
- **Lambda SnapStart** (if supported for the runtime wrapper): Not currently available for `provided.al2023`, but may be relevant in the future
- **Minimize server count:** Design MCP server topology to minimize the number of distinct servers the agent connects to
- **Keep-alive within invocation:** Once connected, reuse the connection for multiple `call_tool` requests within the same invocation

**Detection:**
- `INIT` duration in Lambda metrics consistently high
- Agent response latency dominated by setup rather than LLM/tool execution
- Connection timeout errors during initialization

**Phase mapping:** Phase 2 (MCP client setup). Performance optimization can be deferred but the lazy-init architecture should be established early.

---

### Pitfall 9: Tool Schema Translation Fidelity (MCP to Claude Format)

**What goes wrong:** MCP tool schemas use JSON Schema for `inputSchema`. Claude's tool format also uses JSON Schema for `input_schema` but has specific requirements and limitations:
- Claude expects `type: "object"` at the top level of `input_schema`
- Claude's `description` field has specific length limits
- Some JSON Schema features supported by MCP servers (e.g., `$ref`, `allOf`, `oneOf`) may not be handled well by Claude
- Property names, required fields, and default values need to be preserved exactly

If the translation loses information or produces invalid schemas, Claude either ignores the tool, generates incorrect tool_use calls, or the API rejects the request.

**Why it happens:**
- MCP and Claude both use JSON Schema but with different conventions and strictness levels
- MCP servers authored by third parties may use JSON Schema features that Claude doesn't support
- The translation code might miss edge cases in schema transformation

**Prevention:**
- **Validate translated schemas** before sending to Claude: ensure `type: "object"` at top level, all `$ref` resolved, no unsupported keywords
- **Test with real MCP server schemas** from the servers the agent will use, not just synthetic examples
- **Log schema translation warnings:** If a schema feature can't be translated, log it and use a degraded version rather than silently dropping information
- **Passthrough where possible:** MCP's `inputSchema` is already JSON Schema; if it's compatible with Claude's requirements, pass it through with minimal transformation

**Detection:**
- Claude generating tool_use calls with wrong parameter names or types
- API errors mentioning tool schema validation failures
- Tools that work in MCP test clients but fail when called through the agent

**Phase mapping:** Phase 2 (tool schema mapping). Should be tested with real MCP server schemas.

## Minor Pitfalls

### Pitfall 10: Secrets Management for LLM API Keys in Lambda

**What goes wrong:** The Anthropic API key must be available to the Lambda function. Hardcoding it, storing it in environment variables in plain text, or fetching it from Secrets Manager on every invocation (including replays) adds cost and latency.

**Prevention:**
- Fetch the API key once during Lambda initialization (outside the handler), not inside durable steps
- Use AWS Secrets Manager with the Lambda Secrets Manager extension for cached access
- Never checkpoint the API key (it would be stored in the control plane's checkpoint data)

**Phase mapping:** Phase 2 (LLM client setup).

---

### Pitfall 11: Logging Noise from Replay Suppression

**What goes wrong:** The SDK's mode-aware logging suppresses logs during replay (by default). This means that during debugging, log lines from cached steps are invisible, making it hard to trace the full execution flow. Conversely, disabling mode-aware logging produces duplicate log lines for every previously completed step on every invocation.

**Prevention:**
- Keep mode-aware logging enabled by default
- For debugging, use the `DurableExecutionConfig::with_mode_aware_logging(false)` option temporarily
- Add structured logging (e.g., `tracing` spans) with `iteration_index`, `step_name`, and `is_replay` fields so that replay logs can be filtered separately
- Log a summary at the start of each invocation: "Replaying N cached operations, executing from step M"

**Phase mapping:** Phase 3 (observability).

---

### Pitfall 12: Agent Configuration Changes Between Invocations

**What goes wrong:** The agent reads configuration (system prompt, model, MCP server list, max_iterations) from DynamoDB at startup. If configuration changes between invocations (e.g., admin updates the system prompt mid-execution), the agent's behavior changes mid-run. A different system prompt might cause the LLM to make different decisions, which is fine for new iterations, but if the configuration affects how previous results are interpreted, it can cause inconsistencies.

**Prevention:**
- **Read configuration in a durable step** on the first invocation: `ctx.step("load-config", || read_agent_config())`. Subsequent invocations replay the cached config, ensuring consistency
- If configuration must be updatable mid-execution (e.g., max_iterations increased), read it outside a step but design the loop to be resilient to config changes
- Document which config fields are "immutable per execution" vs "can change"

**Phase mapping:** Phase 1 (configuration loading).

---

### Pitfall 13: Error Propagation Asymmetry Between MCP and Durable Errors

**What goes wrong:** MCP `call_tool` returns results with an `isError` flag and content. Durable SDK steps expect `Result<T, Box<dyn Error>>`. If the MCP error result is converted to a Rust `Err`, the step fails and potentially triggers retries. But MCP "errors" are sometimes expected application-level results (e.g., "file not found" from a filesystem tool) that the LLM should see and reason about, not infrastructure errors that should trigger retries.

**Prevention:**
- **Distinguish MCP transport errors from tool-level errors:** Transport errors (connection refused, timeout, 5xx) should be Rust `Err` and trigger retries. Tool-level errors (isError: true in MCP response) should be `Ok` containing the error content, passed back to the LLM as a `tool_result` with `is_error: true`
- Define a clear type: `enum ToolCallResult { Success(String), ToolError(String), TransportError(Error) }`
- Only let `TransportError` propagate as a durable step failure

**Phase mapping:** Phase 2 (tool execution error handling).

## Phase-Specific Warnings

| Phase Topic | Likely Pitfall | Mitigation |
|-------------|---------------|------------|
| Core loop design (Phase 1) | Non-deterministic branching (#1), operation ID divergence (#5), unbounded growth (#4) | Use child contexts per iteration, always use `ctx.map()` for tool calls, strict determinism discipline |
| Message history (Phase 1) | Checkpoint size exceeded (#2) | Store incremental messages per iteration, not cumulative history. Consider custom Serdes for compression |
| Configuration loading (Phase 1) | Config changes mid-execution (#12) | Read config in a durable step on first invocation |
| MCP client integration (Phase 2) | Connection lifecycle mismatch (#3), cold start amplification (#8) | Lazy connections, cache tool schemas in step, parallel connection setup |
| Tool execution (Phase 2) | Server unavailability (#7), error propagation (#13) | Retry strategies per step, distinguish transport vs tool errors |
| Anthropic API (Phase 2) | Message ordering (#6), schema translation (#9) | Typed message builder, validate before send, test with real schemas |
| Secrets management (Phase 2) | API key handling (#10) | Fetch once at init, never checkpoint |
| Observability (Phase 3) | Replay logging noise (#11) | Mode-aware logging + structured tracing with replay flag |

## Sources

- Direct analysis of SDK source: `src/context/execution_context.rs` (operation ID generation, counter mechanism)
- Direct analysis of SDK source: `src/checkpoint/manager.rs` and `src/checkpoint/manager/queue.rs` (750KB batch limit, batching logic)
- Direct analysis of SDK source: `src/context/durable_context/mod.rs` (256KB per-operation payload limit)
- Direct analysis of SDK source: `src/context/durable_context/step/execute.rs` (step execution, checkpoint flow)
- Direct analysis of SDK source: `src/context/durable_context/step/replay.rs` (replay matching logic)
- Direct analysis of SDK source: `src/context/durable_context/map/execute.rs` (child context isolation, counter reset)
- Direct analysis of SDK source: `src/context/durable_context/child/execute.rs` (replay_children overflow handling)
- Direct analysis of SDK source: `src/context/durable_context/serdes.rs` (serialization failure -> termination)
- `ARCHITECTURE.md` replay mechanism section (determinism requirements, operation lifecycle)
- `.planning/PROJECT.md` (project constraints, MCP SDK reference, existing assets)
