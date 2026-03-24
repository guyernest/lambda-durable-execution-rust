---
phase: 05-deployment-and-validation
plan: 01
subsystem: infra
tags: [sam, dynamodb, lambda, deployment, validation, python, boto3]

requires:
  - phase: 03-agent-loop
    provides: mcp_agent binary with agent loop, types, and config loader
  - phase: 04-observability
    provides: AgentMetadata in AgentResponse for validation checks
provides:
  - McpAgentFunction SAM resource with DurableConfig, DynamoDB, and SecretsManager IAM
  - AgentRegistryTable DynamoDB resource with agent_name/version key schema
  - End-to-end validation script for deployed mcp_agent Lambda
affects: []

tech-stack:
  added: []
  patterns:
    - SAM template pattern for agent Lambda with DynamoDB and SecretsManager IAM
    - PEP 723 validation script with DynamoDB seeding and cleanup

key-files:
  created:
    - examples/scripts/validate_agent.py
  modified:
    - examples/template.yaml

key-decisions:
  - "Timeout 900s for McpAgentFunction (15 min max for multi-iteration LLM calls)"
  - "MemorySize 256MB for McpAgentFunction (LLM response parsing needs more than 128MB)"
  - "secretsmanager:GetSecretValue scoped to Resource * (secret ARN not known at deploy time)"
  - "Validation user message asks for tool listing to exercise config and MCP discovery paths"

patterns-established:
  - "Agent Lambda pattern: extended IAM (DynamoDB + SecretsManager) beyond standard example pattern"
  - "Validation script pattern: seed-invoke-validate-cleanup lifecycle for agent testing"

requirements-completed: [DEPL-01, DEPL-02, DEPL-03]

duration: 3min
completed: 2026-03-24
---

# Phase 5 Plan 1: SAM Deployment and Validation Summary

**McpAgentFunction + AgentRegistryTable in SAM template with end-to-end Python validation script**

## Performance

- **Duration:** 3 min
- **Started:** 2026-03-24T03:26:51Z
- **Completed:** 2026-03-24T03:29:51Z
- **Tasks:** 2
- **Files modified:** 2

## Accomplishments
- SAM template extended with McpAgentFunction (900s timeout, 256MB, DynamoDB/SecretsManager IAM) and AgentRegistryTable (agent_name PK, version SK)
- End-to-end validation script that seeds AgentRegistry, invokes deployed Lambda, validates AgentResponse JSON structure, and cleans up
- Stack outputs for McpAgentFunctionArn and AgentRegistryTableName enable script-driven validation

## Task Commits

Each task was committed atomically:

1. **Task 1: Add McpAgentFunction and AgentRegistryTable to SAM template** - `4c10896` (feat)
2. **Task 2: Create end-to-end agent validation script** - `2959d07` (feat)

## Files Created/Modified
- `examples/template.yaml` - Added AgentRegistryTable, McpAgentFunction resources, AgentRegistryTableName parameter, and new stack outputs
- `examples/scripts/validate_agent.py` - Standalone validation script with DynamoDB seeding, async Lambda invocation, durable execution polling, AgentResponse structure validation, and cleanup

## Decisions Made
- Timeout set to 900s (15 min) for McpAgentFunction because multi-iteration agent loops with LLM calls and tool execution can be long-running
- MemorySize 256MB because LLM response parsing and MCP tool schema handling require more than the 128MB default
- secretsmanager:GetSecretValue scoped to `*` rather than a specific secret ARN because the secret path is configured at runtime via AgentRegistry
- Validation message "What tools do you have available? List them briefly." chosen to exercise config loading and MCP tool discovery without requiring actual tool execution

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

None.

## User Setup Required

None - no external service configuration required.

## Known Stubs

None - both files are complete and functional.

## Next Phase Readiness
- SAM template ready for `sam build && sam deploy`
- Validation script ready for `uv run examples/scripts/validate_agent.py --mcp-server-url <url>`
- This is the final plan in the project; all phases complete

## Self-Check: PASSED

All files and commits verified:
- examples/template.yaml: FOUND
- examples/scripts/validate_agent.py: FOUND
- .planning/phases/05-deployment-and-validation/05-01-SUMMARY.md: FOUND
- Commit 4c10896: FOUND
- Commit 2959d07: FOUND

---
*Phase: 05-deployment-and-validation*
*Completed: 2026-03-24*
