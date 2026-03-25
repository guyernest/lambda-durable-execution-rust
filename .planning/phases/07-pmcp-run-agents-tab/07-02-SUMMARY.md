---
phase: 07-pmcp-run-agents-tab
plan: 02
subsystem: api
tags: [lambda, dynamodb, secrets-manager, appsync, typescript]

# Dependency graph
requires:
  - phase: 07-01
    provides: "Data models, custom types, placeholder handler files"
provides:
  - "execute-agent handler: creates AgentExecution record and async-invokes Durable Agent Lambda"
  - "manage-agent-secrets handler: Secrets Manager CRUD for LLM API keys at org-scoped path"
  - "get-execution-detail handler: queries execution record and returns conversation history"
affects: [07-03, 07-04, 07-05, 07-06]

# Tech tracking
tech-stack:
  added: []
  patterns: [AppSync resolver with fieldName switching, async Lambda invocation, org-scoped Secrets Manager path]

key-files:
  created: []
  modified:
    - "amplify/functions/execute-agent/handler.ts"
    - "amplify/functions/execute-agent/resource.ts"
    - "amplify/functions/manage-agent-secrets/handler.ts"
    - "amplify/functions/manage-agent-secrets/resource.ts"
    - "amplify/functions/get-execution-detail/handler.ts"
    - "amplify/functions/get-execution-detail/resource.ts"

key-decisions:
  - "Used randomUUID from crypto instead of uuid package for execution ID generation"
  - "Conversation history initially built from execution input/output fields; full checkpoint inspection deferred"
  - "Provider name validation uses lowercase alphanumeric only (e.g., anthropic, openai)"

patterns-established:
  - "Agent secret path: pmcp/orgs/{orgId}/agents/llm-keys with provider-keyed JSON"
  - "Agent execution flow: create record -> async invoke -> update to running"
  - "Execution detail: GetCommand lookup with structured response matching GraphQL customType"

requirements-completed: [PMCP-06, PMCP-07, PMCP-08, PMCP-10]

# Metrics
duration: 2min
completed: 2026-03-25
---

# Phase 07 Plan 02: Backend Lambda Functions Summary

**Three AppSync resolver handlers for agent execution (async Lambda invoke), LLM API key management (Secrets Manager CRUD), and execution detail queries (DynamoDB lookup with conversation history)**

## Performance

- **Duration:** 2 min
- **Started:** 2026-03-25T00:31:39Z
- **Completed:** 2026-03-25T00:33:59Z
- **Tasks:** 2
- **Files modified:** 6

## Accomplishments
- execute-agent handler creates AgentExecution record in DynamoDB, looks up agent config/model/MCP servers, async-invokes the Durable Agent Lambda via InvocationType Event
- manage-agent-secrets handler provides CRUD operations (list/set/delete) for LLM provider API keys in Secrets Manager at pmcp/orgs/{orgId}/agents/llm-keys
- get-execution-detail handler queries execution records and builds conversation history from input/output fields
- All resource.ts files updated with appropriate environment variables and memory settings

## Task Commits

Each task was committed atomically:

1. **Task 1: Implement execute-agent and manage-agent-secrets handlers** - `e9ca8073` (feat)
2. **Task 2: Implement get-execution-detail handler** - `c0a3421a` (feat)

## Files Created/Modified
- `amplify/functions/execute-agent/handler.ts` - AppSync resolver that creates execution record and invokes agent Lambda
- `amplify/functions/execute-agent/resource.ts` - defineFunction with AGENT_LAMBDA_FUNCTION_NAME, table env vars
- `amplify/functions/manage-agent-secrets/handler.ts` - Secrets Manager CRUD for LLM API keys
- `amplify/functions/manage-agent-secrets/resource.ts` - defineFunction with permissions comment
- `amplify/functions/get-execution-detail/handler.ts` - Execution detail query with conversation history
- `amplify/functions/get-execution-detail/resource.ts` - defineFunction with AGENT_EXECUTIONS_TABLE, 512MB memory

## Decisions Made
- Used `randomUUID` from Node.js crypto module instead of uuid package for execution ID generation (avoids adding a dependency)
- Conversation history is initially built from execution input/output fields; full Durable Execution checkpoint inspection is a documented stretch goal
- Provider name validation enforces lowercase alphanumeric only to keep secret JSON keys clean
- manage-agent-secrets follows the ensureOrgSecret pattern from manage-secrets (create if not exists)

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- All three backend handlers are complete and ready for frontend integration (Plan 03+)
- Resource files declare placeholder environment variables that must be wired in backend.ts (Plan 03 or later)
- Secrets Manager and Lambda invoke permissions need to be granted in backend.ts CDK escape hatch

## Self-Check: PASSED

All 6 files exist. Both commits (e9ca8073, c0a3421a) verified in git log. SUMMARY.md created.

---
*Phase: 07-pmcp-run-agents-tab*
*Completed: 2026-03-25*
