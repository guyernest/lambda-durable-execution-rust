---
phase: 01-llm-client
plan: 03
subsystem: llm
tags: [secrets-manager, rwlock, reqwest, http-client, service-pipeline, clone, mockito]

# Dependency graph
requires:
  - phase: 01-llm-client plan 01
    provides: "LLMInvocation, LLMResponse, ProviderConfig, LlmError, TransformedRequest, ResponseMetadata"
  - phase: 01-llm-client plan 02
    provides: "MessageTransformer trait, TransformerRegistry with anthropic_v1 and openai_v1"
provides:
  - "SecretManager with get_api_key() and RwLock-based TTL cache for AWS Secrets Manager"
  - "UnifiedLLMService with Clone support and 7-step process() pipeline"
  - "Complete LLM client module ready for ctx.step() integration in Phase 3"
affects: [03-agent-loop]

# Tech tracking
tech-stack:
  added: [mockito 1.7]
  patterns: [RwLock cache with TTL expiry, Arc-wrapped service components for Clone, extracted parse helpers for testability, mockito HTTP mocks for provider tests]

key-files:
  created:
    - examples/src/bin/mcp_agent/llm/secrets.rs
    - examples/src/bin/mcp_agent/llm/service.rs
  modified:
    - examples/src/bin/mcp_agent/llm/mod.rs
    - examples/Cargo.toml

key-decisions:
  - "Used tokio::sync::RwLock<HashMap> instead of DashMap for secret cache to avoid adding dashmap dependency"
  - "Extracted parse_secret_json/parse_secret_to_map helpers as standalone functions for unit testability without AWS mocking"
  - "Used mockito for HTTP mock tests rather than testing auth header logic only through unit tests"
  - "120-second HTTP timeout (up from 60s in original) because agent LLM calls with tool use can be slow"

patterns-established:
  - "SecretManager cache pattern: read lock for cache hit, write lock for cache miss/expiry, separate parse helpers"
  - "UnifiedLLMService Clone pattern: all state behind Arc for cheap Clone into ctx.step() closures"
  - "Service constructor pattern: new() for production, new_with_client/new_with_components for testing"
  - "Provider HTTP testing: mockito server for full request/response cycle verification"

requirements-completed: [LLM-01, LLM-05]

# Metrics
duration: 4min
completed: 2026-03-23
---

# Phase 01 Plan 03: Service & Secrets Summary

**SecretManager with RwLock TTL cache and UnifiedLLMService with Clone support, 7-step process() pipeline (get key, transform request, call provider, transform response, build metadata), and full mockito HTTP test coverage**

## Performance

- **Duration:** 4 min
- **Started:** 2026-03-23T22:01:54Z
- **Completed:** 2026-03-23T22:06:21Z
- **Tasks:** 2
- **Files modified:** 5

## Accomplishments
- SecretManager with RwLock-based cache, TTL expiry, parse helpers, and 11 unit tests
- UnifiedLLMService #[derive(Clone)] with Arc-wrapped components, 7-step process() pipeline, and call_provider() with auth header building
- 16 service tests using mockito HTTP mocks covering auth headers (with/without prefix), custom headers, transformer headers, provider error status codes, and retryability
- Phase 1 LLM client module is complete: models, errors, transformers, secrets, service all wired together (79 total tests across module)

## Task Commits

Each task was committed atomically:

1. **Task 1: Implement SecretManager with RwLock-based cache** - `d530b78` (feat)
2. **Task 2: Implement UnifiedLLMService with Clone support and complete LLM pipeline** - `b3f3321` (feat)

## Files Created/Modified
- `examples/src/bin/mcp_agent/llm/secrets.rs` - SecretManager with get_api_key(), RwLock cache, parse helpers, 11 tests
- `examples/src/bin/mcp_agent/llm/service.rs` - UnifiedLLMService with process(), call_provider(), auth header building, 16 tests
- `examples/src/bin/mcp_agent/llm/mod.rs` - Added secrets and service modules with re-exports
- `examples/Cargo.toml` - Added mockito dev-dependency
- `examples/src/bin/mcp_agent/llm/transformers/*.rs` - cargo fmt applied (formatting only)

## Decisions Made
- **RwLock over DashMap**: Used `tokio::sync::RwLock<HashMap>` instead of `DashMap` to avoid adding the dashmap dependency. The cache access pattern (infrequent writes, frequent reads) maps well to RwLock.
- **Extracted parse helpers**: `parse_secret_json()` and `parse_secret_to_map()` extracted as standalone functions for unit testability without requiring AWS SDK mocking.
- **mockito for HTTP testing**: Added mockito as dev-dependency to test full HTTP request/response cycles in call_provider(), verifying auth headers, custom headers, and error status handling with real HTTP requests.
- **120s HTTP timeout**: Increased from 60s in original source because agent LLM calls with tool_use can involve multiple content blocks and reasoning, taking longer than simple completions.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Removed unused `warn` import in service.rs**
- **Found during:** Task 2 (compilation)
- **Issue:** `warn` imported from tracing but not used in service.rs (only debug, error, info are used)
- **Fix:** Removed `warn` from the import list
- **Files modified:** examples/src/bin/mcp_agent/llm/service.rs
- **Verification:** Compiles without warnings, clippy clean
- **Committed in:** b3f3321

**2. [Rule 1 - Bug] Applied cargo fmt to existing transformer files**
- **Found during:** Task 1 (formatting check)
- **Issue:** Existing transformer files from Plan 02 had minor formatting discrepancies caught by updated rustfmt
- **Fix:** Ran cargo fmt to normalize formatting
- **Files modified:** examples/src/bin/mcp_agent/llm/transformers/{anthropic,openai,mod}.rs
- **Verification:** cargo fmt --check passes clean
- **Committed in:** d530b78

---

**Total deviations:** 2 auto-fixed (2 bugs)
**Impact on plan:** Trivial import and formatting fixes. No scope creep. All acceptance criteria met.

## Issues Encountered
None - plan executed cleanly.

## User Setup Required
None - no external service configuration required.

## Known Stubs
None - SecretManager and UnifiedLLMService are fully implemented with complete test coverage. The mcp_agent main.rs stub (from Plan 01) remains intentionally minimal until Phase 3 wires the agent handler.

## Next Phase Readiness
- Phase 1 LLM client is COMPLETE: types, errors, transformers (Anthropic + OpenAI), secrets, and service
- UnifiedLLMService is Clone-able and ready to be moved into ctx.step() closures (Phase 3)
- Service initialization via UnifiedLLMService::new() happens outside durable steps per D-05
- 79 total unit tests across the LLM module provide confidence for integration

## Self-Check: PASSED

All files verified present. All commits verified in git log. All acceptance criteria content checks passed (22/22).

---
*Phase: 01-llm-client*
*Completed: 2026-03-23*
