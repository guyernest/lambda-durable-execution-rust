---
phase: 07-pmcp-run-agents-tab
plan: 03
subsystem: infra
tags: [cdk, lambda, durable-execution, iam, dynamodb, secrets-manager]

# Dependency graph
requires:
  - phase: 07-01
    provides: "AgentConfig, AgentExecution, LLMModel DynamoDB tables in Amplify data model"
provides:
  - "DurableAgentFunction Lambda via CDK escape hatch with PROVIDED_AL2023 runtime"
  - "IAM permissions for DynamoDB, Secrets Manager, Lambda invoke, durable checkpoint APIs"
  - "Environment variable wiring between agent Lambda and executeAgent/getExecutionDetail/manageAgentSecrets functions"
  - "lambda-target directory for cross-compiled Rust binary"
affects: [07-05, 07-06, deploy]

# Tech tracking
tech-stack:
  added: []
  patterns: ["CDK escape hatch for Rust Lambda in Amplify Gen 2 backend"]

key-files:
  created:
    - "amplify/functions/durable-agent-lambda/lambda-target/.gitkeep"
  modified:
    - "amplify/backend.ts"

key-decisions:
  - "15-minute Lambda timeout for long-running agent executions"
  - "1024MB memory for LLM response processing"
  - "Broad lambda:InvokeFunction permission (arn:aws:lambda:*:*:function:*) for MCP server invocation since server names are user-defined"
  - "Secrets Manager path pattern pmcp/orgs/*/agents/llm-keys for org-scoped LLM API keys"

patterns-established:
  - "CDK escape hatch for custom Rust Lambda alongside Amplify-managed functions"
  - "Environment variable wiring between CDK-created Lambda and Amplify defineFunction resources"

requirements-completed: [PMCP-07, PMCP-08]

# Metrics
duration: 3min
completed: 2026-03-24
---

# Phase 07 Plan 03: CDK Infrastructure Wiring Summary

**Durable Agent Lambda wired into Amplify CDK via escape hatch with DynamoDB, Secrets Manager, Lambda invoke, and durable checkpoint IAM permissions**

## Performance

- **Duration:** 3 min
- **Started:** 2026-03-25T00:31:29Z
- **Completed:** 2026-03-25T00:34:56Z
- **Tasks:** 1
- **Files modified:** 2

## Accomplishments
- DurableAgentFunction created via CDK escape hatch with PROVIDED_AL2023 runtime, ARM_64, 15-min timeout, 1024MB memory
- Agent Lambda granted DynamoDB read/write (AgentExecution) and read (AgentConfig, LLMModel, McpServer) permissions
- Agent Lambda granted Secrets Manager, Lambda invoke, and durable execution checkpoint API permissions
- executeAgent function wired with AGENT_LAMBDA_FUNCTION_NAME env var and invoke permission plus DynamoDB table env vars
- getExecutionDetail function wired with AGENT_EXECUTIONS_TABLE env var and read permission
- manageAgentSecrets function wired with Secrets Manager CRUD permissions
- lambda-target directory placeholder created for cross-compiled Rust binary

## Task Commits

Each task was committed atomically:

1. **Task 1: Add Durable Agent Lambda via CDK escape hatch** - `b66c2a78` (feat) backend.ts changes, `fee41893` (feat) .gitkeep placeholder

**Plan metadata:** (pending)

## Files Created/Modified
- `amplify/backend.ts` - Added DurableAgentFunction CDK escape hatch, IAM permissions, env var wiring for executeAgent/getExecutionDetail/manageAgentSecrets
- `amplify/functions/durable-agent-lambda/lambda-target/.gitkeep` - Placeholder for cross-compiled Rust binary directory

## Decisions Made
- Used 15-minute timeout (vs 5-min for MCP Tester) since agent executions involve multiple LLM call + tool execution iterations
- Used 1024MB memory (vs 512MB for MCP Tester) for LLM response processing
- Broad Lambda invoke permission pattern matches existing MCP Tester pattern for user-defined function names
- Durable checkpoint API permissions use wildcard resource since checkpoint APIs are self-scoped to the calling function

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
- Parallel agent (07-04) committed backend.ts changes before this agent's commit, since both agents edited the same file on the same worktree. The backend.ts changes are correctly captured in commit b66c2a78. The .gitkeep placeholder was committed separately in fee41893.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- CDK infrastructure ready for agent UI components (07-04, 07-05, 07-06)
- lambda-target directory ready for cross-compiled Rust binary from the agent crate build
- All three backend functions (executeAgent, manageAgentSecrets, getExecutionDetail) have correct permissions and env vars

## Self-Check: PASSED

- FOUND: amplify/functions/durable-agent-lambda/lambda-target/.gitkeep
- FOUND: DurableAgentFunction in backend.ts
- FOUND: AGENT_LAMBDA_FUNCTION_NAME in backend.ts
- FOUND: CheckpointDurableExecution in backend.ts
- FOUND: commit b66c2a78
- FOUND: commit fee41893

---
*Phase: 07-pmcp-run-agents-tab*
*Completed: 2026-03-24*
