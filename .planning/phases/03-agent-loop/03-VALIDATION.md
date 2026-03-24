---
phase: 3
slug: agent-loop
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-03-23
---

# Phase 3 — Validation Strategy

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
| 3-01-01 | 01 | 1 | LOOP-01..07, MCP-04, MCP-05 | unit+integration | `cargo test --manifest-path examples/Cargo.toml` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `examples/src/bin/mcp_agent/handler.rs` — handler module created
- [ ] Handler compiles with durable SDK integration

*Existing test infrastructure covers framework needs.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Full agent loop with real LLM + MCP | LOOP-01 | Requires live LLM API key and MCP server | Deploy, configure agent, invoke with test messages |
| Durable replay across suspension | LOOP-04 | Requires Lambda suspension/resumption | Deploy with DurableConfig, trigger wait, verify replay |

*Unit tests mock LLM responses and MCP tools. Integration requires AWS deployment (Phase 5).*

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 15s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
