---
phase: 06-pmcp-sdk-example
plan: 02
subsystem: sdk-example
tags: [mcp-tasks, wait-for-condition, durable-polling, pmcp, lambda]

# Dependency graph
requires:
  - phase: 06-01
    provides: "Base durable MCP agent example with call_tool, agent loop, ctx.map"
provides:
  - "Task-aware tool execution handling both immediate and async MCP tool results"
  - "wait_for_condition polling pattern for long-running MCP tasks"
  - "Educational comments explaining zero-cost Lambda suspension during task polling"
affects: [07-agents-tab, 08-channels, 09-teams]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "ToolCallResponse::Task -> wait_for_condition -> tasks_get polling pattern"
    - "Duration ms-to-seconds conversion for durable SDK compatibility"

key-files:
  created: []
  modified:
    - "~/Development/mcp/sdk/rust-mcp-sdk/examples/65_durable_mcp_agent.rs"

key-decisions:
  - "Converted poll_interval milliseconds to seconds (min 1s) since durable Duration has second granularity"
  - "Used max_attempts(60) as safety limit for polling (~5min at default 5s intervals)"

patterns-established:
  - "Task polling pattern: call_tool_with_task -> match Result/Task -> wait_for_condition with tasks_get"
  - "Terminal status handling: Completed -> tasks_result, Failed/Cancelled -> error ToolResult for LLM recovery"

requirements-completed: [SDK-02]

# Metrics
duration: 2min
completed: 2026-03-24
---

# Phase 06 Plan 02: MCP Tasks Client-Side Handling Summary

**Task-aware tool execution with wait_for_condition polling for zero-cost Lambda suspension during long-running MCP tool operations**

## Performance

- **Duration:** 2 min
- **Started:** 2026-03-24T22:26:50Z
- **Completed:** 2026-03-24T22:29:11Z
- **Tasks:** 1
- **Files modified:** 1

## Accomplishments
- Replaced `call_tool()` with `call_tool_with_task()` for full MCP Tasks support
- Added `ToolCallResponse::Task` handling with `wait_for_condition` durable polling
- Added `ToolCallResponse::Result` handling for immediate tool results
- Added `extract_text_content()` helper for DRY text extraction from CallToolResult
- Included educational comments explaining zero compute cost advantage of durable polling vs tokio::sleep
- Handled all terminal task statuses: Completed (tasks_result), Failed/Cancelled (error to LLM)

## Task Commits

Each task was committed atomically:

1. **Task 1: Add task-aware tool execution with wait_for_condition polling** - `7fa33e9` (feat)

## Files Created/Modified
- `~/Development/mcp/sdk/rust-mcp-sdk/examples/65_durable_mcp_agent.rs` - Added task-aware execute_tool_call with ToolCallResponse matching, wait_for_condition polling, extract_text_content helper, educational comments

## Decisions Made
- Converted millisecond poll_interval to seconds (minimum 1s) since the durable SDK Duration type operates at second granularity -- sub-second polling would be wasteful for MCP Tasks anyway
- Used `max_attempts(60)` as a safety limit (~5 minutes at default 5s intervals) to prevent runaway polling
- Returned Failed/Cancelled task statuses as error ToolResults rather than DurableErrors, letting the LLM decide recovery strategy

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Duration::milliseconds does not exist in durable SDK**
- **Found during:** Task 1 (wait_for_condition configuration)
- **Issue:** Plan specified `Duration::milliseconds(poll_ms as i64)` but the durable SDK Duration type only supports seconds granularity (no milliseconds constructor)
- **Fix:** Converted poll_interval from milliseconds to seconds with `std::cmp::max(1, (poll_ms / 1000) as u32)` and used `Duration::seconds(poll_secs)`
- **Files modified:** `~/Development/mcp/sdk/rust-mcp-sdk/examples/65_durable_mcp_agent.rs`
- **Verification:** `cargo build --example 65_durable_mcp_agent --features streamable-http` succeeds
- **Committed in:** 7fa33e9

**2. [Rule 1 - Bug] execute_tool_call return type mismatch**
- **Found during:** Task 1 (function signature change)
- **Issue:** Plan changed `execute_tool_call` to return `DurableResult<ToolResult>` but the map closure was wrapping in `.map_err(|e| DurableError::Internal(e.to_string()))`. The function now returns DurableResult directly.
- **Fix:** Removed the `.map_err()` wrapper in the map closure since execute_tool_call now returns DurableResult natively
- **Files modified:** `~/Development/mcp/sdk/rust-mcp-sdk/examples/65_durable_mpc_agent.rs`
- **Verification:** Compilation succeeds
- **Committed in:** 7fa33e9

---

**Total deviations:** 2 auto-fixed (2 bug fixes)
**Impact on plan:** Both fixes necessary for compilation. No scope creep.

## Issues Encountered
None -- all changes compiled on second attempt after the Duration fix.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Phase 06 (PMCP SDK Example) is complete with both plans executed
- Example demonstrates full durable agent loop with MCP Tasks support
- Ready for Phase 07 (Agents Tab) which builds the management UI

## Self-Check: PASSED

- [x] examples/65_durable_mcp_agent.rs exists
- [x] Commit 7fa33e9 exists
- [x] 06-02-SUMMARY.md exists

---
*Phase: 06-pmcp-sdk-example*
*Completed: 2026-03-24*
