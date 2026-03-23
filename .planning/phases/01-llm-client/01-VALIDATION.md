---
phase: 1
slug: llm-client
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-03-23
---

# Phase 1 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (tokio::test for async) |
| **Config file** | examples/Cargo.toml |
| **Quick run command** | `cargo test --manifest-path examples/Cargo.toml --all-targets` |
| **Full suite command** | `cargo test --manifest-path examples/Cargo.toml --all-targets && cargo clippy --manifest-path examples/Cargo.toml --all-targets -- -D warnings` |
| **Estimated runtime** | ~10 seconds |

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
| 1-01-01 | 01 | 1 | LLM-01 | unit | `cargo test --manifest-path examples/Cargo.toml` | ❌ W0 | ⬜ pending |
| 1-01-02 | 01 | 1 | LLM-02 | unit | `cargo test --manifest-path examples/Cargo.toml` | ❌ W0 | ⬜ pending |
| 1-01-03 | 01 | 1 | LLM-03 | unit | `cargo test --manifest-path examples/Cargo.toml` | ❌ W0 | ⬜ pending |
| 1-02-01 | 02 | 1 | LLM-04 | unit | `cargo test --manifest-path examples/Cargo.toml` | ❌ W0 | ⬜ pending |
| 1-02-02 | 02 | 1 | LLM-05 | unit | `cargo test --manifest-path examples/Cargo.toml` | ❌ W0 | ⬜ pending |
| 1-02-03 | 02 | 1 | LLM-06 | unit | `cargo test --manifest-path examples/Cargo.toml` | ❌ W0 | ⬜ pending |
| 1-02-04 | 02 | 1 | LLM-07 | unit | `cargo test --manifest-path examples/Cargo.toml` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `examples/src/bin/mcp_agent/` — binary directory structure created
- [ ] `examples/src/bin/mcp_agent/main.rs` — minimal binary entry point
- [ ] Test infrastructure verified: `cargo test --manifest-path examples/Cargo.toml` runs successfully with the new binary

*Existing test infrastructure (examples/Cargo.toml, cargo test) covers framework needs. Wave 0 creates the binary structure.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| API key retrieval from Secrets Manager | LLM-05 | Requires AWS credentials and actual secret | Deploy to AWS, invoke with valid secret_path |
| End-to-end LLM API call | LLM-01 | Requires live API endpoint and key | Invoke with real Anthropic/OpenAI API key |

*Unit tests mock HTTP responses and Secrets Manager. Integration tests require AWS deployment.*

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 15s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
