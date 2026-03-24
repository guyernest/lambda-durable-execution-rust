---
phase: 06-pmcp-sdk-example
plan: 01
subsystem: examples
tags: [pmcp, mcp-agent, durable-execution, anthropic, lambda, reqwest, streamable-http]

# Dependency graph
requires:
  - phase: 03-agent-loop
    provides: production agent handler pattern (handler.rs, mcp/client.rs, types.rs)
  - phase: 05-deployment-and-validation
    provides: SAM deployment pattern and validation script
provides:
  - Self-contained durable MCP agent example (65_durable_mcp_agent.rs) in PMCP SDK repo
  - Example Cargo.toml dev-dependencies for lambda-durable-execution-rust integration
  - Reference implementation of LLM + MCP tool loop with durable primitives
affects: [06-02, pmcp-sdk-docs]

# Tech tracking
tech-stack:
  added: [lambda-durable-execution-rust (git dev-dep), lambda_runtime (dev-dep), reqwest 0.13 (dev-dep)]
  patterns: [durable agent loop with ctx.step/map/run_in_child_context, inline Anthropic API client, MCP tool prefix routing]

key-files:
  created:
    - ~/Development/mcp/sdk/rust-mcp-sdk/examples/65_durable_mcp_agent.rs
  modified:
    - ~/Development/mcp/sdk/rust-mcp-sdk/Cargo.toml

key-decisions:
  - "Used git dependency pointing to guyernest fork (not crates.io) since official AWS Rust SDK not yet released"
  - "reqwest dev-dep uses 'rustls' feature (matching PMCP SDK main deps) not 'rustls-tls'"
  - "Example is 723 lines (above 350-400 target) due to thorough educational doc comments per D-02"
  - "MCP connections established outside durable steps with explicit Pitfall 1 comments"

patterns-established:
  - "PMCP SDK example pattern: single-file example with //! doc header, [[example]] entry, required-features gate"
  - "Durable agent loop pattern: step for LLM calls, map for parallel tool execution, run_in_child_context for iteration isolation"

requirements-completed: [SDK-01, SDK-03]

# Metrics
duration: 9min
completed: 2026-03-24
---

# Phase 6 Plan 1: PMCP SDK Durable MCP Agent Example Summary

**Self-contained durable MCP agent example in PMCP SDK repo with LLM + MCP tool loop using ctx.step(), ctx.map(), and ctx.run_in_child_context()**

## Performance

- **Duration:** 9 min
- **Started:** 2026-03-24T22:11:45Z
- **Completed:** 2026-03-24T22:20:52Z
- **Tasks:** 2
- **Files modified:** 2

## Accomplishments
- Created 65_durable_mcp_agent.rs -- a 723-line self-contained reference example demonstrating an LLM + MCP tool loop agent with durable execution
- Added all required dev-dependencies to PMCP SDK Cargo.toml without polluting main dependencies
- Example compiles successfully with `cargo build --example 65_durable_mcp_agent --features streamable-http`
- Comprehensive educational comments explaining WHY each durable primitive is used

## Task Commits

Each task was committed atomically:

1. **Task 1: Add durable agent dev-dependencies and example entry** - `c4b9333` (feat)
2. **Task 2: Create 65_durable_mcp_agent.rs with core agent loop** - `04730c6` (feat)

## Files Created/Modified
- `~/Development/mcp/sdk/rust-mcp-sdk/Cargo.toml` - Added dev-dependencies (lambda-durable-execution-rust, lambda_runtime, reqwest, tracing-subscriber) and [[example]] entry for 65_durable_mcp_agent
- `~/Development/mcp/sdk/rust-mcp-sdk/examples/65_durable_mcp_agent.rs` - Self-contained durable MCP agent example with inline types, Anthropic API client, MCP tool discovery/execution, and durable agent loop

## Decisions Made
- Used `guyernest` GitHub remote (verified via `git remote get-url origin`) for the git dependency URL
- Used `rustls` feature for reqwest (matching PMCP SDK's main deps pattern) instead of `rustls-tls`
- Example is longer than the 350-400 line target (723 lines) because educational doc comments explain each durable primitive -- aligned with D-02 self-contained and easy to understand goal
- Advertise `ClientCapabilities::full()` (includes Tasks support) per D-08, preparing for Plan 02's task-aware tool handling

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed borrow-then-move error in discover_tools()**
- **Found during:** Task 2 (Build verification)
- **Issue:** `prefix` borrowed from `parsed` URL, but `parsed` was moved into `StreamableHttpTransportConfig`. Compiler error E0505.
- **Fix:** Changed `prefix` to be an owned `String` via `.to_string()` instead of a `&str` borrow
- **Files modified:** examples/65_durable_mcp_agent.rs
- **Verification:** `cargo build --example 65_durable_mcp_agent --features streamable-http` succeeds
- **Committed in:** 04730c6 (Task 2 commit)

---

**Total deviations:** 1 auto-fixed (1 bug)
**Impact on plan:** Standard Rust ownership fix. No scope creep.

## Issues Encountered
None beyond the auto-fixed borrow issue.

## User Setup Required
None - no external service configuration required. The example runs as a Lambda function deployed via SAM.

## Next Phase Readiness
- Plan 02 (MCP Tasks client-side handling with wait_for_condition) can proceed -- the example is structured to accept the task-aware tool execution extension
- The example already advertises `ClientCapabilities::full()` which includes Tasks support

## Self-Check: PASSED

- [x] `~/Development/mcp/sdk/rust-mcp-sdk/examples/65_durable_mcp_agent.rs` exists
- [x] Commit `c4b9333` (Task 1) exists in git history
- [x] Commit `04730c6` (Task 2) exists in git history
- [x] `cargo build --example 65_durable_mcp_agent --features streamable-http` succeeds

---
*Phase: 06-pmcp-sdk-example*
*Completed: 2026-03-24*
