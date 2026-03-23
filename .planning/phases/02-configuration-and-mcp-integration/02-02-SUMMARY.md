---
phase: 02-configuration-and-mcp-integration
plan: 02
subsystem: mcp
tags: [mcp, pmcp, streamable-http, tool-discovery, routing, url-prefix]

# Dependency graph
requires:
  - phase: 01-llm-client
    provides: UnifiedTool type and clean_tool_schema utility for tool schema translation
  - phase: 02-configuration-and-mcp-integration
    provides: Plan 01 config module (mod config in main.rs)
provides:
  - MCP client module with discover_all_tools for connecting to MCP servers
  - ToolsWithRouting struct (Serialize+Deserialize) for checkpointed tool discovery
  - Tool name prefixing with host-based identifiers ({prefix}__{name})
  - Routing map and resolve_tool_call for dispatching tool calls to correct MCP server
  - McpError enum for MCP-specific error handling
affects: [03-agent-loop, 05-deployment]

# Tech tracking
tech-stack:
  added: [pmcp 2.0.0 (path dep with streamable-http feature), url 2.5]
  patterns: [host-prefix tool naming, splitn(2 "__") for safe prefix stripping, sequential MCP server connection with fail-fast]

key-files:
  created:
    - examples/src/bin/mcp_agent/mcp/client.rs
    - examples/src/bin/mcp_agent/mcp/types.rs
    - examples/src/bin/mcp_agent/mcp/error.rs
    - examples/src/bin/mcp_agent/mcp/mod.rs
  modified:
    - examples/Cargo.toml
    - examples/src/bin/mcp_agent/main.rs

key-decisions:
  - "Renamed thiserror 'source' fields to 'reason' to avoid thiserror special treatment of source fields"
  - "Used ToolInfo::new() constructor instead of struct literals since ToolInfo is #[non_exhaustive]"

patterns-established:
  - "MCP tool naming: {host_prefix}__{tool_name} with splitn(2, '__') for safe reverse mapping"
  - "Ephemeral MCP connections: only ToolsWithRouting (serializable) persists, not Client objects"

requirements-completed: [MCP-01, MCP-02, MCP-03, MCP-06]

# Metrics
duration: 4min
completed: 2026-03-23
---

# Phase 02 Plan 02: MCP Client Integration Summary

**MCP client module with pmcp StreamableHttpTransport, sequential tool discovery from 1-3 servers, host-prefix tool naming, and routing map for tool call dispatch**

## Performance

- **Duration:** 4 min
- **Started:** 2026-03-23T23:25:05Z
- **Completed:** 2026-03-23T23:29:48Z
- **Tasks:** 1
- **Files modified:** 6

## Accomplishments
- MCP client module (mcp/) with 4 files implementing full tool discovery pipeline
- discover_all_tools connects to MCP servers sequentially via pmcp StreamableHttpTransport, initializes, paginates list_tools, translates ToolInfo to UnifiedTool with clean_tool_schema normalization
- Tool names prefixed with host-based identifier (extract first segment before dot from URL host), routing map maps prefixed names back to server URLs
- resolve_tool_call uses splitn(2, "__") to safely handle tool names containing double underscores
- ToolsWithRouting derives Serialize + Deserialize for checkpoint persistence (ephemeral connections, only results checkpointed per D-07)
- 14 unit tests covering all pure functions (prefix extraction, tool translation, routing resolution, serde round-trip, empty servers error)

## Task Commits

Each task was committed atomically:

1. **Task 1: Add pmcp + url dependencies and create mcp module with types, error, client** - `bbda282` (feat)

## Files Created/Modified
- `examples/Cargo.toml` - Added pmcp (path dep with streamable-http) and url 2.5 dependencies
- `examples/src/bin/mcp_agent/main.rs` - Added `mod mcp` declaration
- `examples/src/bin/mcp_agent/mcp/mod.rs` - Module re-exports (discover_all_tools, resolve_tool_call, McpError, ToolsWithRouting)
- `examples/src/bin/mcp_agent/mcp/error.rs` - McpError enum with 7 variants for connection, discovery, routing, and config errors
- `examples/src/bin/mcp_agent/mcp/types.rs` - ToolsWithRouting struct with tools Vec and routing HashMap
- `examples/src/bin/mcp_agent/mcp/client.rs` - discover_all_tools, connect_and_discover, translate_mcp_tool, extract_host_prefix, resolve_tool_call + 14 tests

## Decisions Made
- Renamed thiserror `source` fields to `reason` because thiserror 2.0 treats `source` as a special field that must implement `std::error::Error`, but we store `String` error descriptions from pmcp
- Used `ToolInfo::new()` constructor instead of struct literal syntax because pmcp marks ToolInfo as `#[non_exhaustive]` -- struct literal construction would fail

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Renamed `source` field to `reason` in McpError variants**
- **Found during:** Task 1 (cargo check)
- **Issue:** thiserror 2.0 treats fields named `source` as error sources requiring `std::error::Error` impl. `String` does not implement this trait.
- **Fix:** Renamed `source` to `reason` in ConnectionFailed, InitializationFailed, and DiscoveryFailed variants
- **Files modified:** examples/src/bin/mcp_agent/mcp/error.rs, examples/src/bin/mcp_agent/mcp/client.rs
- **Verification:** cargo check passes, all tests pass
- **Committed in:** bbda282 (Task 1 commit)

---

**Total deviations:** 1 auto-fixed (1 bug fix)
**Impact on plan:** Minor naming change for thiserror compatibility. No scope creep.

## Issues Encountered
- Pre-existing clippy warning in `map_with_failure_tolerance` example binary (unrelated to MCP module) -- `clippy::io_other_error` lint. Not fixed (out of scope, not introduced by this plan).

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- MCP client module ready for integration with agent loop (Phase 3)
- discover_all_tools returns ToolsWithRouting that can be used inside ctx.step("discover-tools") for checkpointed tool discovery
- resolve_tool_call ready for use in the tool execution pipeline to route calls back to correct MCP server
- Integration tests with live MCP servers deferred to Phase 5

---
## Self-Check: PASSED

All created files verified present. Commit bbda282 verified in git log. SUMMARY.md exists.

*Phase: 02-configuration-and-mcp-integration*
*Completed: 2026-03-23*
