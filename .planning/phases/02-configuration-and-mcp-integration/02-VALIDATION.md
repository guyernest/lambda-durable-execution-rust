---
phase: 2
slug: configuration-and-mcp-integration
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-03-23
---

# Phase 2 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (tokio::test for async) |
| **Config file** | examples/Cargo.toml |
| **Quick run command** | `cargo test --manifest-path examples/Cargo.toml --all-targets` |
| **Full suite command** | `cargo test --manifest-path examples/Cargo.toml --all-targets && cargo clippy --manifest-path examples/Cargo.toml -p lambda-durable-execution-rust-examples --bin mcp_agent -- -D warnings` |
| **Estimated runtime** | ~15 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test --manifest-path examples/Cargo.toml --all-targets`
- **After every plan wave:** Run full suite command
- **Before `/gsd:verify-work`:** Full suite must be green
- **Max feedback latency:** 15 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|-----------|-------------------|-------------|--------|
| 2-01-01 | 01 | 1 | CONF-01..04 | unit | `cargo test --manifest-path examples/Cargo.toml` | ❌ W0 | ⬜ pending |
| 2-02-01 | 02 | 2 | MCP-01..03, MCP-06 | unit | `cargo test --manifest-path examples/Cargo.toml` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `examples/src/bin/mcp_agent/config/` — config module directory created
- [ ] `examples/src/bin/mcp_agent/mcp/` — mcp module directory created
- [ ] pmcp dependency added to examples/Cargo.toml
- [ ] aws-sdk-dynamodb dependency added to examples/Cargo.toml

*Existing test infrastructure (cargo test) covers framework needs. Wave 0 creates module structures and dependencies.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| DynamoDB AgentRegistry read | CONF-01 | Requires live DynamoDB table | Deploy, insert test agent config, invoke |
| MCP server connection + list_tools | MCP-01, MCP-02 | Requires live MCP server endpoint | Deploy MCP server, configure agent, invoke |

*Unit tests mock DynamoDB and MCP responses. Integration requires AWS deployment.*

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 15s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
