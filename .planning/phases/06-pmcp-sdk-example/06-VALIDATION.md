---
phase: 6
slug: pmcp-sdk-example
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-03-24
---

# Phase 6 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (Rust native) |
| **Config file** | Cargo.toml in PMCP SDK repo examples/ |
| **Quick run command** | `cargo test --manifest-path examples/Cargo.toml -p durable-agent-example` |
| **Full suite command** | `cargo test --manifest-path examples/Cargo.toml --all-targets` |
| **Estimated runtime** | ~15 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo check --manifest-path examples/Cargo.toml`
- **After every plan wave:** Run full suite command
- **Before `/gsd:verify-work`:** Full suite must be green
- **Max feedback latency:** 15 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|-----------|-------------------|-------------|--------|
| 06-01-01 | 01 | 1 | SDK-01 | compile | `cargo check` | ❌ W0 | ⬜ pending |
| 06-01-02 | 01 | 1 | SDK-02 | unit | `cargo test test_task_status_mapping` | ❌ W0 | ⬜ pending |
| 06-01-03 | 01 | 1 | SDK-03 | unit | `cargo test test_progress_reporting` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] Example project scaffold with Cargo.toml and dependencies
- [ ] Basic compilation test (cargo check passes)

*Existing PMCP SDK test infrastructure covers compilation; example-specific tests need scaffolding.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Agent loop completes with real LLM | SDK-01 | Requires API key + MCP server | Deploy to Lambda, invoke with test prompt |
| Task polling suspends Lambda | SDK-02 | Requires durable execution environment | Observe checkpoint logs during task wait |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 15s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
