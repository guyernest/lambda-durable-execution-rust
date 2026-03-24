# Phase 4: Observability - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.

**Date:** 2026-03-24
**Phase:** 04-observability
**Areas discussed:** Metadata in response

---

## Metadata in Response

| Option | Description | Selected |
|--------|-------------|----------|
| Separate metadata field | agent_metadata alongside flattened LLMResponse, SF callers ignore it | ✓ |
| Inside LLMResponse.metadata | Extend ResponseMetadata — changes shape | |
| Log-only | Metadata in CloudWatch only, response unchanged | |

**User's choice:** Separate metadata field
**Notes:** Option with skip_serializing_if for backward compatibility.

## Claude's Discretion

- Token accumulation pattern
- Step naming convention
- Elapsed time tracking
- Tools called list collection
- SDK logger vs tracing choice
