# Phase 7: pmcp-run Agents Tab - Context

**Gathered:** 2026-03-24
**Status:** Ready for planning

<domain>
## Phase Boundary

Add an Agents tab to the pmcp-run web UI that enables users to create, configure, execute, and monitor durable MCP agents. Includes agent CRUD (instructions, model, MCP servers, parameters), on-demand execution with status tracking, execution history with conversation detail view, and metrics/cost dashboards. All without touching DynamoDB directly, SAM templates, or the command line.

</domain>

<decisions>
## Implementation Decisions

### Agent Lambda deployment
- **D-01:** The Durable Agent Lambda is deployed via CDK escape hatch within Amplify Gen 2's `amplify/backend.ts`. pmcp-run already uses this pattern for its MCP Tester Lambda (Rust binary).
- **D-02:** One shared agent Lambda per pmcp-run environment. pmcp-run is designed as a private per-organization deployment, not a multi-tenant SaaS. If multi-tenant support is needed later, Lambda's tenant separation feature can be used.
- **D-03:** Agent behavior is determined by config passed in the invocation payload (agent name, model, MCP servers, instructions, parameters). The Lambda binary is the same for all agents.

### Data model
- **D-04:** Fresh Amplify Gen 2 model (`AgentConfig`) in `amplify/data/resource.ts`, following pmcp-run's existing patterns (like McpServer). No backward compatibility with the Step Functions AgentRegistry schema.
- **D-05:** MCP server selection in the agent form pulls from pmcp-run's existing server registry only. No arbitrary external URLs — ensures agents only use trusted, deployed servers.
- **D-06:** `AgentExecution` model tracks execution state: started, iteration count, status (running/completed/failed), final response, token counts, tools called. The agent Lambda writes to this table during and after execution.
- **D-07:** `AgentExecution` includes the durable execution ID so the UI can query the Durable Execution API for detailed checkpoint/message history when needed.

### Execution and status tracking
- **D-08:** Agent execution is triggered via async Lambda invocation (`InvocationType: 'Event'`) from an AppSync mutation. Returns immediately with an execution ID.
- **D-09:** UI polls the `AgentExecution` DynamoDB table via GraphQL for status updates (running → completed/failed). This avoids the heavier `GetDurableExecution` API for routine status checks.
- **D-10:** For execution detail view (full conversation history, tool call inputs/outputs), the UI calls a backend function that queries the Durable Execution API using the stored execution ID.

### Metrics and cost tracking
- **D-11:** Token counts (input/output) are aggregated in the agent Lambda and stored in the `AgentExecution` DynamoDB row as part of the execution summary. UI reads from DynamoDB for per-execution cost display.
- **D-12:** Time-series metrics (token usage trends, execution counts, success rates) are emitted via CloudWatch Embedded Metric Format (EMF) — structured metric data in log lines. No direct CloudWatch PutMetricData API calls (too expensive). Metrics do not need to be real-time.
- **D-13:** Cost tracking uses the model registry pricing data multiplied by actual token counts from `AgentExecution` records. Aggregated in the UI, not pre-computed.

### Model registry and API keys
- **D-14:** Model registry is a new Amplify Gen 2 model (`LLMModel`) with provider, model_id, display_name, input_price_per_1k, output_price_per_1k, max_tokens, supports_tools, is_active. Seeded with common models (Claude, GPT-4).
- **D-15:** API keys stored in Secrets Manager using pmcp-run's existing org-scoped path pattern: `pmcp/orgs/{orgId}/agents/llm-keys`. Managed via the existing `manage-secrets` function pattern.

### Claude's Discretion
- Exact LCARS component selection for agent list, form, and detail views
- GraphQL query/mutation naming conventions (follow pmcp-run's existing style)
- Pagination strategy for execution history (cursor-based like existing pmcp-run patterns)
- Agent form field ordering and grouping
- Metrics dashboard chart library choice (follow existing pmcp-run patterns)
- How the agent Lambda reports progress updates to DynamoDB during execution (callback, periodic write, or only at completion)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### pmcp-run Amplify Gen 2 backend
- `~/Development/mcp/sdk/pmcp-run/amplify/data/resource.ts` — Existing data models (McpServer, Deployment, Organization), authorization patterns, custom types, mutation/query definitions
- `~/Development/mcp/sdk/pmcp-run/amplify/backend.ts` — CDK escape hatch patterns for Rust Lambda deployment
- `~/Development/mcp/sdk/pmcp-run/amplify/functions/` — Existing function patterns (manage-secrets, execute-test-scenario, update-server-metadata)

### pmcp-run frontend
- `~/Development/mcp/sdk/pmcp-run/components/dashboard/authentic-navigation.tsx` — Navigation array for adding Agents tab
- `~/Development/mcp/sdk/pmcp-run/app/(authenticated)/` — Existing page structure and routing patterns
- `~/Development/mcp/sdk/pmcp-run/components/ui/` — LCARS UI components available for reuse

### Agent Lambda (source code to adapt)
- `examples/src/bin/mcp_agent/handler.rs` — Production agent handler with AgentResponse metadata
- `examples/src/bin/mcp_agent/types.rs` — AgentRequest, AgentResponse, AgentMetadata types
- `examples/src/bin/mcp_agent/config/` — AgentConfig types and DynamoDB loader

### Step Functions Agent (patterns to port)
- `~/projects/step-functions-agent/ui_amplify/src/pages/` — 14 UI pages including Registries, History, Metrics, ModelCosts, AgentExecution, ExecutionDetail
- `~/projects/step-functions-agent/ui_amplify/amplify/data/resource.ts` — LLMModels table schema for model registry seeding

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `manage-secrets/handler.ts`: Secrets Manager CRUD pattern with org-scoped paths — reuse for LLM API key management
- `execute-test-scenario/handler.ts`: Async Lambda invocation pattern (`InvocationType: 'Event'`) — reuse for agent execution trigger
- `update-server-metadata/handler.ts`: DynamoDB update pattern with `DynamoDBDocumentClient` — reuse for execution status updates
- `authentic-navigation.tsx`: Navigation array — add `{ name: 'Agents', href: '/agents', color: 'cyan' }` entry
- McpServer model in `resource.ts`: Pattern for multi-field model with relationships, authorization, secondary indexes

### Established Patterns
- Amplify Gen 2 `defineFunction()` for backend Lambda functions with `resourceGroupName: 'data'`
- `AppSyncResolverHandler<Input, Output>` for GraphQL mutation/query handlers
- `{ success: boolean, error?: string }` return pattern for mutations
- CDK escape hatch in `backend.ts` for non-standard Lambda deployments (Rust binaries)
- Org-scoped Secrets Manager paths: `pmcp/orgs/{orgId}/...`

### Integration Points
- Navigation: `authentic-navigation.tsx` line 7-15 (navigation array)
- Data model: `amplify/data/resource.ts` (add AgentConfig, AgentExecution, LLMModel models)
- Backend functions: `amplify/functions/` (add manage-agents, execute-agent, get-execution-detail)
- CDK: `amplify/backend.ts` (add Durable Agent Lambda via escape hatch)
- Frontend: `app/(authenticated)/agents/` (new page directory)

</code_context>

<specifics>
## Specific Ideas

- pmcp-run is a private per-organization deployment, not multi-tenant SaaS — simplifies the data model and security model
- Token metrics via EMF format (embedded in CloudWatch logs), not direct PutMetricData calls — cost-effective for non-real-time metrics
- Agent execution DynamoDB row holds the durable execution ID as a bridge to the detailed checkpoint/message API — lightweight status polling for the UI, deep inspection available on demand
- MCP server selection restricted to pmcp-run's deployed servers — agents only use trusted, managed tools

</specifics>

<deferred>
## Deferred Ideas

- Scheduled/triggered agent execution (EventBridge Scheduler) — v3 (PMCP-13)
- Approval dashboard for pending human approvals — Phase 8 (Channels)
- Live execution streaming via WebSocket — v3 (PMCP-18)
- Agent version management with draft/active/archived promotion — v3 (PMCP-17)
- Test prompt library — v3 (PMCP-16)
- Multi-tenant Lambda isolation — use Lambda tenant separation when needed

</deferred>

---

*Phase: 07-pmcp-run-agents-tab*
*Context gathered: 2026-03-24*
