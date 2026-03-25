---
phase: 07-pmcp-run-agents-tab
plan: 01
subsystem: database
tags: [amplify-gen2, dynamodb, graphql, appsync, data-model]

# Dependency graph
requires: []
provides:
  - AgentConfig model with LLM config, instructions, MCP server refs, parameters
  - AgentExecution model with status tracking, token counts, durable execution bridge
  - LLMModel model with provider pricing, capabilities, active flag
  - Custom mutations (executeAgent, setAgentSecret, deleteAgentSecret)
  - Custom queries (getAgentExecutionDetail, listAgentSecrets)
  - Placeholder function resources for execute-agent, manage-agent-secrets, get-execution-detail
affects: [07-02, 07-03, 07-04, 07-05, 07-06]

# Tech tracking
tech-stack:
  added: []
  patterns: [agent-config-model, agent-execution-model, llm-model-registry]

key-files:
  created:
    - ~/Development/mcp/sdk/pmcp-run/amplify/functions/execute-agent/resource.ts
    - ~/Development/mcp/sdk/pmcp-run/amplify/functions/execute-agent/handler.ts
    - ~/Development/mcp/sdk/pmcp-run/amplify/functions/manage-agent-secrets/resource.ts
    - ~/Development/mcp/sdk/pmcp-run/amplify/functions/manage-agent-secrets/handler.ts
    - ~/Development/mcp/sdk/pmcp-run/amplify/functions/get-execution-detail/resource.ts
    - ~/Development/mcp/sdk/pmcp-run/amplify/functions/get-execution-detail/handler.ts
  modified:
    - ~/Development/mcp/sdk/pmcp-run/amplify/data/resource.ts

key-decisions:
  - "Placed AGENTS section between BUILT-IN SERVER BUILDER and USAGE & BILLING sections in resource.ts"
  - "Placed AGENT OPERATIONS custom mutations/queries before existing SECRETS MANAGEMENT API section"
  - "Added belongsTo relationships on AgentConfig and AgentExecution back to Organization"

patterns-established:
  - "Agent model pattern: AgentConfig with denormalized agentName on AgentExecution for list display"
  - "Agent secrets pattern: org-scoped LLM API key management via manageAgentSecrets function (mirrors manageSecrets pattern)"

requirements-completed: [PMCP-01, PMCP-02, PMCP-03, PMCP-04, PMCP-05, PMCP-06, PMCP-07, PMCP-08, PMCP-09]

# Metrics
duration: 2min
completed: 2026-03-25
---

# Phase 7 Plan 1: Data Models Summary

**AgentConfig, AgentExecution, and LLMModel Amplify Gen 2 models with secondary indexes, authorization rules, and custom GraphQL mutations/queries for agent operations**

## Performance

- **Duration:** 2 min
- **Started:** 2026-03-25T00:25:28Z
- **Completed:** 2026-03-25T00:28:13Z
- **Tasks:** 2
- **Files modified:** 7

## Accomplishments
- Three new Amplify Gen 2 data models (AgentConfig, AgentExecution, LLMModel) with full field definitions, secondary indexes, and authorization rules
- Five custom GraphQL operations (executeAgent, getAgentExecutionDetail, listAgentSecrets, setAgentSecret, deleteAgentSecret) wired to placeholder function handlers
- Organization model extended with hasMany relationships to AgentConfig and AgentExecution

## Task Commits

Each task was committed atomically:

1. **Task 1: Add AgentConfig, AgentExecution, and LLMModel data models** - `ed8e4189` (feat)
2. **Task 2: Add custom mutations and queries for agent operations** - `56993274` (feat)

## Files Created/Modified
- `amplify/data/resource.ts` - Added three models (AgentConfig, AgentExecution, LLMModel), Organization hasMany relationships, import statements, and AGENT OPERATIONS custom mutations/queries section
- `amplify/functions/execute-agent/resource.ts` - Placeholder function resource for async agent execution
- `amplify/functions/execute-agent/handler.ts` - Placeholder handler (Plan 02)
- `amplify/functions/manage-agent-secrets/resource.ts` - Placeholder function resource for LLM API key management
- `amplify/functions/manage-agent-secrets/handler.ts` - Placeholder handler (Plan 02)
- `amplify/functions/get-execution-detail/resource.ts` - Placeholder function resource for execution detail query
- `amplify/functions/get-execution-detail/handler.ts` - Placeholder handler (Plan 02)

## Decisions Made
- Placed AGENTS section between BUILT-IN SERVER BUILDER and USAGE & BILLING sections to group with other data models
- Placed AGENT OPERATIONS custom mutations/queries before SECRETS MANAGEMENT API section in the custom operations area
- Added `belongsTo` relationships on AgentConfig and AgentExecution back to Organization for bidirectional navigation

## Deviations from Plan

None - plan executed exactly as written.

## Known Stubs

These are intentional placeholders, documented in the plan as being implemented in Plan 02:

| File | Line | Stub | Resolving Plan |
|------|------|------|----------------|
| amplify/functions/execute-agent/handler.ts | 4 | `throw new Error('Not implemented - placeholder for Plan 02')` | 07-02 |
| amplify/functions/manage-agent-secrets/handler.ts | 4 | `throw new Error('Not implemented - placeholder for Plan 02')` | 07-02 |
| amplify/functions/get-execution-detail/handler.ts | 4 | `throw new Error('Not implemented - placeholder for Plan 02')` | 07-02 |

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Data layer complete: AgentConfig, AgentExecution, LLMModel models ready for CRUD operations
- Custom mutations/queries defined and wired to placeholder functions
- Plan 02 can implement the actual function handlers (execute-agent, manage-agent-secrets, get-execution-detail)
- Plan 03+ can build UI components against the GraphQL API

## Self-Check: PASSED

All files exist, all commits verified, SUMMARY.md created.

---
*Phase: 07-pmcp-run-agents-tab*
*Completed: 2026-03-25*
