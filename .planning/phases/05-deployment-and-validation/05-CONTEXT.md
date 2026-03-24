# Phase 5: Deployment and Validation - Context

**Gathered:** 2026-03-24
**Status:** Ready for planning

<domain>
## Phase Boundary

Deploy the durable MCP agent via SAM template and validate end-to-end with a real MCP server. Includes IAM permissions, DurableConfig, environment variables, and a validation script that seeds AgentRegistry and invokes the agent.

</domain>

<decisions>
## Implementation Decisions

### Validation MCP server
- **D-01:** Use an existing deployed MCP server from the step-functions-agent project (Calculator or similar). No new server deployment needed. The MCP server URL is configured in the AgentRegistry seed data.

### AgentRegistry seeding
- **D-02:** Create a validation script (Python, similar to existing `examples/scripts/validate.py`) that:
  1. Inserts the test agent config into the AgentRegistry DynamoDB table
  2. Invokes the durable agent Lambda with a test message
  3. Checks the response for expected structure (AgentResponse with LLM response + agent_metadata)
  4. Reports pass/fail
- **D-03:** The script takes command-line args for region, stack name, and MCP server URL. The MCP server URL is the operator's choice — they point it at whatever MCP server they want to validate against.

### Claude's Discretion
- SAM template structure (follow existing `examples/template.yaml` pattern exactly)
- IAM policy statements (DynamoDB read, Secrets Manager read, Lambda checkpoint — follow existing patterns)
- DurableConfig settings (ExecutionTimeout, RetentionPeriodInDays — match existing examples)
- Environment variables (AGENT_REGISTRY_TABLE, RUST_LOG)
- Validation script implementation details (Python with boto3, similar to validate.py)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Existing SAM template (pattern to follow)
- `examples/template.yaml` — Existing SAM template with DurableConfig, nodejs24.x runtime, EXEC_WRAPPER, IAM policies. The mcp_agent entry should follow this exact pattern.

### Existing validation script (pattern to follow)
- `examples/scripts/validate.py` — Existing validation script that invokes deployed examples and checks results. The agent validation script should follow this pattern.

### Agent handler (what's being deployed)
- `examples/src/bin/mcp_agent/main.rs` — Entry point
- `examples/src/bin/mcp_agent/handler.rs` — Handler function
- `examples/src/bin/mcp_agent/types.rs` — AgentRequest (input), AgentResponse (output)

### AgentRegistry schema (for seed data)
- `~/projects/step-functions-agent/lambda/shared/agent_registry.py` — DynamoDB schema
- `examples/src/bin/mcp_agent/config/loader.rs` — Fields the agent reads (system_prompt, llm_provider, llm_model, parameters, mcp_servers)
- `examples/src/bin/mcp_agent/config/types.rs` — AgentConfig, AgentParameters types

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `examples/template.yaml`: 16 existing Lambda function entries with identical DurableConfig/IAM pattern — copy and adapt for mcp_agent
- `examples/scripts/validate.py`: Existing validation framework with `--region`, `--stack`, `--out` args, colored output, Lambda invocation via boto3
- `examples/Cargo.toml`: Already has `[[bin]] name = "mcp_agent"` entry — SAM just needs to reference it

### Established Patterns
- `BuildMethod: rust-cargolambda` with `Binary: mcp_agent` in SAM Metadata
- `Runtime: nodejs24.x` + `AWS_LAMBDA_EXEC_WRAPPER: /var/task/bootstrap`
- `DurableConfig: { ExecutionTimeout: 300, RetentionPeriodInDays: 7 }`
- IAM: `AWSLambdaBasicExecutionRole` + `AWSLambdaBasicDurableExecutionRolePolicy` + inline `lambda:CheckpointDurableExecution`

### Integration Points
- SAM template additions go in `examples/template.yaml` (new Resource + Output)
- Validation script goes in `examples/scripts/validate_agent.py` (new file)
- The agent Lambda needs additional IAM for DynamoDB and Secrets Manager (beyond what existing examples have)

</code_context>

<specifics>
## Specific Ideas

- The validation script should be self-contained — an operator can run it after `sam deploy` to verify the agent works
- MCP server URL is a parameter, not hardcoded — different operators use different MCP servers
- The script should create and clean up its own test data in AgentRegistry

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 05-deployment-and-validation*
*Context gathered: 2026-03-24*
