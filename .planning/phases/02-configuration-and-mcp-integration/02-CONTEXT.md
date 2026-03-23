# Phase 2: Configuration and MCP Integration - Context

**Gathered:** 2026-03-23
**Status:** Ready for planning

<domain>
## Phase Boundary

Agent loads its configuration from the existing AgentRegistry DynamoDB table and connects to MCP servers to discover and translate tool schemas. This phase produces a fully initialized agent config (system prompt, LLM settings, tools) ready for the agent loop in Phase 3.

</domain>

<decisions>
## Implementation Decisions

### AgentRegistry schema
- **D-01:** The `mcp_servers` field is a simple URL array: `["https://mcp1.example.com", "https://mcp2.example.com"]`. No per-server auth or metadata in DynamoDB. Simplest possible schema extension.
- **D-02:** Read existing `llm_provider` + `llm_model` fields from AgentRegistry (not a full ProviderConfig). Map to ProviderConfig in code (provider → endpoint, transformer ID, secret path). This preserves the operator workflow where different steps update different records (providers, API keys, model enablement, agent configuration).
- **D-03:** Config loading is a durable `ctx.step()` — the DynamoDB read result is cached on replay.

### MCP server connectivity
- **D-04:** MCP servers are deployed as Lambda functions with Web Adapter behind HTTP endpoints (API Gateway or Function URLs). The agent uses pmcp HttpTransport to connect.
- **D-05:** No application-level auth for PoC — servers are accessed via IAM/VPC. OAuth support deferred to later using existing PMCP SDK OAuth capabilities (user-forwarded tokens or M2M tokens).
- **D-06:** Expect 1-3 MCP servers per agent. Sequential connection is fine — total setup under 1 second. No need for parallel connection logic.
- **D-07:** MCP connections are ephemeral — established per Lambda invocation, NOT inside durable steps. Only the results (tool schemas, tool call outputs) are checkpointed. Connections cannot be serialized.

### Tool name collisions
- **D-08:** Prefix all tool names with a host-based identifier extracted from the MCP server URL. Format: `{host_prefix}__{tool_name}` (e.g., `calc__multiply`, `wiki__search`). This applies to all tools regardless of whether collisions exist — consistent naming for the LLM.
- **D-09:** When routing a tool call back to the correct MCP server, reverse-map the prefix to find the originating server URL.

### Claude's Discretion
- DynamoDB client setup and query implementation
- MCP schema-to-Claude API tool format translation (straightforward field mapping)
- Error types for config loading failures and MCP connection failures
- How to extract a short host prefix from URLs (e.g., `calc-server.us-east-1.amazonaws.com` → `calc-server`)
- Test strategy (mock DynamoDB, mock MCP server responses)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Agent code (Phase 1 output)
- `examples/src/bin/mcp_agent/llm/models.rs` — ProviderConfig, UnifiedTool types that config loading must produce
- `examples/src/bin/mcp_agent/llm/mod.rs` — Module structure to extend with config/mcp modules
- `examples/src/bin/mcp_agent/main.rs` — Entry point to wire config loading into

### Existing AgentRegistry
- `~/projects/step-functions-agent/lambda/shared/agent_registry.py` — DynamoDB schema (agent_name PK, version SK, system_prompt, llm_provider, llm_model, tools, parameters, mcp_servers)
- `~/projects/step-functions-agent/stacks/shared/agent_registry_stack.py` — CDK table definition with GSIs

### MCP SDK
- `~/Development/mcp/sdk/rust-mcp-sdk/src/client/mod.rs` — pmcp Client API (initialize, list_tools, call_tool)
- `~/Development/mcp/sdk/rust-mcp-sdk/src/types/tools.rs` — ToolInfo struct (name, description, input_schema)

### Durable SDK
- `src/context/durable_context/step.rs` — How ctx.step() works for config caching

### Research
- `.planning/research/ARCHITECTURE.md` — Component boundaries, MCP connection lifecycle
- `.planning/research/PITFALLS.md` — MCP connection replay safety, tool discovery caching

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `UnifiedTool` struct (models.rs): Already has `name`, `description`, `input_schema` fields matching Claude API tool format — MCP tool translation targets this type
- `ProviderConfig` struct (models.rs): Target for the provider mapping from AgentRegistry fields
- `LlmError` enum (error.rs): Can be extended with config/MCP error variants or a new error type created
- `clean_tool_schema()` (utils.rs): Already normalizes JSON Schema for tool input — reuse for MCP tool schemas

### Established Patterns
- Module-per-concern under `examples/src/bin/mcp_agent/` — add `config/` and `mcp/` modules
- Serde `Serialize + Deserialize` on all types for checkpoint compatibility
- `#[cfg(test)] mod tests` with focused unit tests per module

### Integration Points
- Config loading produces: system_prompt (String), ProviderConfig, Vec<UnifiedTool>, max_iterations (u32), mcp_server_urls (Vec<String>)
- MCP integration produces: Vec<UnifiedTool> (merged from all servers), server-tool mapping for routing
- Both feed into the agent handler (Phase 3)

</code_context>

<specifics>
## Specific Ideas

- "Drop-in replacement" means the operator workflow stays the same — they configure agents in the same UI, same DynamoDB fields, just with the added mcp_servers URL array
- The provider mapping (llm_provider → ProviderConfig) is where the join happens between operator-managed records (providers, keys) and agent config
- OAuth for MCP servers is a real future requirement — user tokens forwarded from the interface or M2M tokens. The PMCP SDK already supports this. Not for PoC but design should not preclude it.

</specifics>

<deferred>
## Deferred Ideas

- OAuth/token-based MCP server authentication — future phase, using PMCP SDK OAuth capabilities
- Parallel MCP server connection — not needed for 1-3 servers
- MCP server health checks / circuit breaker — future hardening
- Dynamic provider config from a separate DynamoDB table — current approach maps in code

</deferred>

---

*Phase: 02-configuration-and-mcp-integration*
*Context gathered: 2026-03-23*
