# Phase 6: PMCP SDK Example - Context

**Gathered:** 2026-03-24
**Status:** Ready for planning

<domain>
## Phase Boundary

Create a reference MCP agent example in the PMCP SDK repo (`~/Development/mcp/sdk/rust-mcp-sdk/`) that demonstrates how to build an MCP agent (LLM + MCP tool loop) using Durable Lambda for checkpointed execution. The example showcases PMCP as an MCP client connecting to MCP servers, with MCP Tasks client-side handling for long-running tools. This is a client-side example — the agent consumes MCP servers, it does not expose itself as one.

</domain>

<decisions>
## Implementation Decisions

### Example packaging
- **D-01:** Example lives in the PMCP SDK repo (`~/Development/mcp/sdk/rust-mcp-sdk/examples/`) as a reference for PMCP SDK users
- **D-02:** The example contains a simplified inline agent loop — not the full production handler from this repo. Self-contained and easy to understand as a reference
- **D-03:** Depends on `lambda-durable-execution-rust` crate. For now, use git dependency pointing to this repo (fork). When the official AWS Rust Durable Lambda SDK releases (~April 2026), switch to the official crate. Consider publishing the fork to crates.io as an interim step to make it easier for others to use

### Focus and scope
- **D-04:** The example is primarily about building MCP agents that call MCP servers in an agent loop — the core LLM + tool_use pattern. MCP Tasks handling is a secondary detail demonstrating the power of Durable Lambda for long-running processes
- **D-05:** Client-side MCP Tasks handling only. Server-side Tasks (exposing agents as MCP servers) is deferred to Phase 9 (Agent Teams) with the dynamic MCP server
- **D-06:** Aligns with PMCP SDK philosophy: stateless, serverless MCP patterns. The MCP agent is a natural extension of this philosophy

### MCP Tasks client behavior
- **D-07:** When a tool returns a task (CreateTaskResult), the agent uses `ctx.wait_for_condition()` to poll for completion. The condition checks task status via `tasks/get`. The SDK manages wait intervals internally — Lambda suspends between checks (no compute cost)
- **D-08:** Agent advertises task support in its client capabilities AND in each tool call
- **D-09:** Fallback behavior for non-task-aware paths: if a tool returns immediate results, handle normally. If it returns a task but the agent somehow doesn't support tasks, fall back to polling tools if available

### Requirements revision needed
- **D-10:** The original SDK-01, SDK-02, SDK-03 requirements describe the agent as an MCP server with TaskSupport::Required. These need revision to reflect the actual scope: agent as MCP client with task-aware tool handling. The requirements should be updated during planning to match the client-side focus.

### Claude's Discretion
- Exact simplified agent loop structure (how much of the production handler to include)
- Demo MCP server with slow tools for testing (optional — can test against any task-supporting server)
- Documentation depth and code comments
- Example configuration approach (env vars, config file, or inline constants)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### PMCP SDK (target repo)
- `~/Development/mcp/sdk/rust-mcp-sdk/docs/TASKS_WITH_POLLING.md` — MCP Tasks polling flow, TypedTool with TaskSupport, requestor-driven detection
- `~/Development/mcp/sdk/rust-mcp-sdk/crates/pmcp-tasks/src/lib.rs` — Task types, TaskStatus enum, CreateTaskResult, TaskSupport
- `~/Development/mcp/sdk/rust-mcp-sdk/src/server/builder.rs` — ServerCoreBuilder API (reference for understanding server-side; agent is client-side)
- `~/Development/mcp/sdk/rust-mcp-sdk/examples/` — Existing example patterns and conventions

### Durable Agent (source code to simplify)
- `examples/src/bin/mcp_agent/handler.rs` — Production agent handler (simplify for example)
- `examples/src/bin/mcp_agent/mcp/client.rs` — MCP client connections, tool discovery, tool execution
- `examples/src/bin/mcp_agent/llm/service.rs` — UnifiedLLMService (simplify or mock for example)
- `examples/src/bin/mcp_agent/types.rs` — AgentRequest, AgentResponse, IterationResult types

### Durable Execution SDK primitives
- `src/context/durable_context/` — `step()`, `map()`, `wait_for_condition()` APIs
- `src/types/` — StepConfig, Duration, retry strategies

### MCP Tasks spec
- MCP Tasks Specification (2025-11-25) — task lifecycle, status states, polling, cancellation

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `mcp_agent/handler.rs` agent_handler(): Full production agent loop — simplify for example (remove observability, error recovery complexity)
- `mcp_agent/mcp/client.rs` discover_all_tools(), execute_tool_call(): MCP client patterns to reuse directly
- `mcp_agent/llm/` UnifiedLLMService: Can simplify to Anthropic-only for the example
- `pmcp-tasks` crate: TaskStore, InMemoryTaskStore, TaskRouter — for understanding client-side types
- `pmcp` crate Client: `list_tools()`, `call_tool()` — the MCP client API the example uses

### Established Patterns
- Agent loop: `run_in_child_context` per iteration for replay determinism
- LLM calls: `ctx.step()` with ExponentialBackoff retry
- Tool execution: `ctx.map()` for parallel execution of multiple tool calls
- MCP connections: Established outside durable steps, cached in Arc<HashMap>
- Tool schemas: Prefixed with server identifier for routing (`{prefix}__{tool_name}`)

### Integration Points
- The example will be a new binary in PMCP SDK's examples/ directory
- It depends on `pmcp` (MCP client) and `lambda-durable-execution-rust` (durable primitives)
- MCP Tasks client handling integrates into the existing `execute_tool_call()` flow — after calling `call_tool()`, check if response is a CreateTaskResult and handle accordingly

</code_context>

<specifics>
## Specific Ideas

- "The PMCP SDK is encouraging MCP servers that are stateless and serverless, and the MCP Agent is such a natural extension"
- The example should demonstrate the simplicity of Durable Lambda — how `ctx.wait_for_condition()` elegantly handles long-running tool polling with zero compute cost during waits
- This repo is a fork of the AWS Durable Lambda SDK — the official Rust SDK hasn't been released yet (only Python and TypeScript). Expected within weeks (~April 2026). The example should be structured to easily switch from fork to official crate
- MCP Tasks is part of the evolving MCP spec deprecating SSE mode — the example should showcase modern MCP patterns

</specifics>

<deferred>
## Deferred Ideas

- Server-side MCP Tasks (wrapping agents as MCP servers) — Phase 9 (Agent Teams) with dynamic MCP server
- Demo MCP server with slow tools for testing — can be added later or tested against any existing task-supporting server
- Publishing the durable SDK fork to crates.io — related but separate effort from the example itself

</deferred>

---

*Phase: 06-pmcp-sdk-example*
*Context gathered: 2026-03-24*
