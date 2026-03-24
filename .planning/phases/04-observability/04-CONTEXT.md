# Phase 4: Observability - Context

**Gathered:** 2026-03-24
**Status:** Ready for planning

<domain>
## Phase Boundary

Add token tracking, iteration metadata, and structured per-step logging to the existing agent handler. This is instrumentation on a working agent — no behavioral changes to the loop itself.

</domain>

<decisions>
## Implementation Decisions

### Metadata in response
- **D-01:** Add a separate `agent_metadata` field to `AgentResponse` alongside the flattened `LLMResponse`. The `agent_metadata` struct contains: `iterations: u32`, `total_input_tokens: u32`, `total_output_tokens: u32`, `tools_called: Vec<String>`, `elapsed_ms: u64`. Step Functions callers ignore the extra field — the flattened LLM response fields remain unchanged.
- **D-02:** The `agent_metadata` field is `Option<AgentMetadata>` with `#[serde(skip_serializing_if = "Option::is_none")]` so it doesn't appear when not populated (backward compatible).

### Claude's Discretion
- Token accumulation pattern (accumulate from `LLMResponse.metadata.tokens_used` per iteration)
- Step naming convention for tracing (e.g., `"llm-call"` already used, tool calls already named via `ctx.map()`)
- How to track elapsed time (`Instant::now()` at handler start, elapsed at return)
- How to collect tools_called list (accumulate tool names from each iteration's function_calls)
- Whether to use the SDK's `step_ctx.info()` / `step_ctx.warn()` or direct `tracing::info!()` — both are available

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Agent handler (Phase 3 output)
- `examples/src/bin/mcp_agent/handler.rs` — agent_handler, execute_iteration, build_llm_invocation (where to add instrumentation)
- `examples/src/bin/mcp_agent/types.rs` — AgentResponse (add agent_metadata field), IterationResult
- `examples/src/bin/mcp_agent/llm/models.rs` — ResponseMetadata.tokens_used (TokenUsage with input_tokens, output_tokens)

### Durable SDK logging
- `src/types/logger.rs` — DurableLogger trait, TracingLogger, StepContext logging helpers

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `TokenUsage` (llm/models.rs): Already has `input_tokens`, `output_tokens`, `total_tokens` — accumulate per iteration
- `LLMResponse.metadata.tokens_used` (llm/models.rs): Available after each `ctx.step("llm-call")` result
- `tracing::info!()` with structured fields: Already used throughout the handler for iteration logging
- Step names: `"load-config"`, `"discover-tools"`, `"llm-call"`, `"tools"` already in handler.rs

### Integration Points
- `AgentResponse` in types.rs: Add `agent_metadata: Option<AgentMetadata>` field
- Handler loop in handler.rs: Accumulate tokens and tool names per iteration
- Handler return in handler.rs: Populate `agent_metadata` before returning `AgentResponse`

</code_context>

<specifics>
## Specific Ideas

- Token tracking is read from existing `LLMResponse.metadata.tokens_used` — no new API calls needed
- Elapsed time measured with `std::time::Instant` at handler start
- Tools called list is the set of unique tool names from all iterations' function_calls

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 04-observability*
*Context gathered: 2026-03-24*
