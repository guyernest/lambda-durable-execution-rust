---
phase: 03-agent-loop
plan: 01
subsystem: agent
tags: [durable-execution, agent-loop, mcp, llm, run_in_child_context, ctx.map, ctx.step, exponential-backoff]

# Dependency graph
requires:
  - phase: 01-llm-client
    provides: UnifiedLLMService, LLMInvocation, LLMResponse, FunctionCall, ContentBlock types
  - phase: 02-config-mcp
    provides: AgentConfig, load_agent_config, discover_all_tools, resolve_tool_call, ToolsWithRouting
provides:
  - AgentRequest and AgentResponse types for handler input/output
  - IterationResult and ToolCallResult for checkpoint round-trip
  - establish_mcp_connections for persistent MCP client caching
  - execute_tool_call for routing tool calls to correct MCP server
  - agent_handler function implementing the full durable agent loop
  - main.rs wired with with_durable_execution_service
affects: [04-testing, 05-deployment]

# Tech tracking
tech-stack:
  added: []
  patterns: [run_in_child_context per iteration, ctx.step with ExponentialBackoff for LLM, ctx.map for parallel tool execution, incremental message history assembly]

key-files:
  created:
    - examples/src/bin/mcp_agent/types.rs
    - examples/src/bin/mcp_agent/handler.rs
  modified:
    - examples/src/bin/mcp_agent/mcp/client.rs
    - examples/src/bin/mcp_agent/mcp/error.rs
    - examples/src/bin/mcp_agent/mcp/mod.rs
    - examples/src/bin/mcp_agent/llm/mod.rs
    - examples/src/bin/mcp_agent/llm/error.rs
    - examples/src/bin/mcp_agent/llm/secrets.rs
    - examples/src/bin/mcp_agent/llm/service.rs
    - examples/src/bin/mcp_agent/config/mod.rs
    - examples/src/bin/mcp_agent/config/error.rs
    - examples/src/bin/mcp_agent/main.rs

key-decisions:
  - "MCP connections established outside durable steps (D-03) and cached in Arc<HashMap> for reuse across iterations"
  - "pmcp Client<StreamableHttpTransport> wrapped in Arc for shared access -- call_tool takes &self so Arc is sufficient"
  - "AgentResponse uses serde(flatten) on LLMResponse for Step Functions output compatibility (D-02)"
  - "Tool execution errors (is_error: true) passed to LLM as error tool_results, not handler failures (D-12, MCP-05)"

patterns-established:
  - "Agent loop pattern: for loop with run_in_child_context per iteration, ctx.step for LLM, ctx.map for tools"
  - "Message history assembly: incremental append from step results, system prompt prepended at invocation build time"
  - "MCP client lifecycle: establish_mcp_connections at handler start, share via Arc across iterations"

requirements-completed: [LOOP-01, LOOP-02, LOOP-03, LOOP-04, LOOP-05, LOOP-06, LOOP-07, MCP-04, MCP-05]

# Metrics
duration: 7min
completed: 2026-03-24
---

# Phase 3 Plan 1: Agent Loop Summary

**Durable agent handler with run_in_child_context iteration loop, ExponentialBackoff LLM calls via ctx.step, and parallel MCP tool execution via ctx.map**

## Performance

- **Duration:** 7 min
- **Started:** 2026-03-24T00:48:57Z
- **Completed:** 2026-03-24T00:55:57Z
- **Tasks:** 2
- **Files modified:** 12

## Accomplishments
- Created AgentRequest/AgentResponse/IterationResult/ToolCallResult types with full Serialize+Deserialize for checkpoint round-trip
- Implemented establish_mcp_connections and execute_tool_call for persistent MCP client caching and tool routing
- Built the complete agent_handler with durable loop: config loading, tool discovery, iteration with child contexts, LLM calls with retry, parallel tool execution, incremental message history
- Wired handler into main.rs with with_durable_execution_service -- binary compiles and passes clippy cleanly

## Task Commits

Each task was committed atomically:

1. **Task 1: Create agent types and MCP tool execution** - `0d180e1` (feat)
2. **Task 2: Create the durable agent handler** - `e6fe5a3` (feat)

## Files Created/Modified
- `examples/src/bin/mcp_agent/types.rs` - AgentRequest, AgentResponse, IterationResult, ToolCallResult types
- `examples/src/bin/mcp_agent/handler.rs` - agent_handler with durable loop, execute_iteration, build_llm_invocation, llm_response_to_assistant_message, build_tool_results_message
- `examples/src/bin/mcp_agent/mcp/client.rs` - Added establish_mcp_connections and execute_tool_call; fixed clippy split_once
- `examples/src/bin/mcp_agent/mcp/error.rs` - Added ToolExecutionFailed variant
- `examples/src/bin/mcp_agent/mcp/mod.rs` - Updated re-exports for Phase 3 consumption
- `examples/src/bin/mcp_agent/llm/mod.rs` - Added MessageContent to re-exports
- `examples/src/bin/mcp_agent/llm/error.rs` - Added allow(dead_code) for test-only variants
- `examples/src/bin/mcp_agent/llm/secrets.rs` - Added allow(dead_code) for test helpers
- `examples/src/bin/mcp_agent/llm/service.rs` - Added allow(dead_code) for test helper
- `examples/src/bin/mcp_agent/config/mod.rs` - Updated re-exports
- `examples/src/bin/mcp_agent/config/error.rs` - Added allow(dead_code) for InvalidJson variant
- `examples/src/bin/mcp_agent/main.rs` - Wired handler with with_durable_execution_service

## Decisions Made
- MCP connections established outside durable steps (D-03) and cached in `Arc<HashMap<String, Client<StreamableHttpTransport>>>` -- `call_tool` takes `&self` so `Arc` sharing is sufficient without `RwLock`
- AgentResponse uses `#[serde(flatten)]` on LLMResponse for Step Functions output format compatibility (D-02)
- Tool execution errors (`is_error: true` from MCP) are passed to the LLM as error `tool_result` content blocks, not handler failures -- the LLM decides recovery (D-12, MCP-05)
- Added `mod types;` to main.rs in Task 1 (earlier than planned) because mcp/client.rs imports from crate::types -- Rule 3 blocking fix

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Added mod types declaration early**
- **Found during:** Task 1 (Create agent types)
- **Issue:** mcp/client.rs imports `crate::types::ToolCallResult` but `mod types;` was planned for Task 2
- **Fix:** Added `mod types;` to main.rs in Task 1 to unblock compilation
- **Files modified:** examples/src/bin/mcp_agent/main.rs
- **Verification:** cargo check compiles successfully
- **Committed in:** 0d180e1 (Task 1 commit)

**2. [Rule 1 - Bug] Fixed clippy manual_split_once in resolve_tool_call**
- **Found during:** Task 2 verification (clippy)
- **Issue:** Pre-existing `splitn(2, "__").nth(1)` triggers clippy manual_split_once lint
- **Fix:** Changed to `split_once("__").map(|x| x.1)` -- identical behavior
- **Files modified:** examples/src/bin/mcp_agent/mcp/client.rs
- **Verification:** clippy passes clean
- **Committed in:** e6fe5a3 (Task 2 commit)

**3. [Rule 3 - Blocking] Added allow(dead_code) for pre-existing test helpers**
- **Found during:** Task 2 verification (clippy -D warnings)
- **Issue:** Removing #[allow(dead_code)] from module declarations exposed pre-existing dead_code warnings in test helpers, unused error variants, and the is_retryable method
- **Fix:** Added targeted #[allow(dead_code)] on specific items that are used by test modules but not the binary
- **Files modified:** config/error.rs, llm/error.rs, llm/secrets.rs, llm/service.rs
- **Verification:** clippy -D warnings passes clean
- **Committed in:** e6fe5a3 (Task 2 commit)

---

**Total deviations:** 3 auto-fixed (1 Rule 1, 2 Rule 3)
**Impact on plan:** All auto-fixes necessary for compilation and clippy compliance. No scope creep.

## Issues Encountered
None -- plan executed smoothly with only minor compilation ordering adjustments.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Agent handler compiles and is wired into the Lambda entry point
- Ready for Phase 3 Plan 2 (testing) and Phase 4+ (SAM deployment, integration testing)
- MCP client connection sharing pattern established (Arc without RwLock)

## Self-Check: PASSED

- All created files verified present on disk
- All commit hashes verified in git log
- cargo check, clippy -D warnings, and fmt --check all pass

---
*Phase: 03-agent-loop*
*Completed: 2026-03-24*
