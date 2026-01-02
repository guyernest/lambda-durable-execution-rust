# Repository Guidelines

## Project Structure & Module Organization

- Root `Cargo.toml` defines the SDK crate `lambda-durable-execution-rust` and a workspace that includes `examples/`. Source lives in `src/`.
- `examples/`: a separate Cargo package (`examples/Cargo.toml`) with deployable Lambda binaries under `examples/src/bin/`. Deployment config is in `examples/template.yaml`; generated diagrams live in `examples/diagrams/`; validation tooling lives in `examples/scripts/`.
- Tests live alongside code in `#[cfg(test)] mod tests { … }` blocks (no top-level integration tests yet). Rustdoc examples are compiled (`no_run`), so keep them buildable.

## Build, Test, and Development Commands

- `cargo build` — build the SDK crate.
- `cargo check` — fast typecheck without producing binaries.
- `cargo test` — run SDK unit tests + doc-tests.
- `cargo test --doc` — run only doc-tests (useful when editing rustdoc examples).
- `cargo test --manifest-path examples/Cargo.toml --all-targets` — compile/test the examples package (binaries have no unit tests).
- `cargo build --manifest-path examples/Cargo.toml --all-targets` — compile the examples locally (they’re intended to run on AWS Lambda).
- `cargo fmt` — format with rustfmt (standard config).
- `cargo fmt --check` — CI-friendly formatting check.
- `cargo clippy --all-targets --all-features -D warnings` — lint; keep warnings at zero.
- `sam build -t examples/template.yaml --beta-features` — build the example Lambdas via the `rust-cargolambda` build method.
- `sam deploy -t examples/template.yaml` — deploy the example stack.
- `uv run examples/scripts/validate.py ...` — invoke the deployed examples and regenerate `examples/diagrams/*.mermaid` + `*.md` from real execution history.

## Coding Style & Naming Conventions

- Rust edition 2021, MSRV 1.88 (see `Cargo.toml`); keep new code compatible.
- Use rustfmt defaults (4‑space indent). Run `cargo fmt` before committing.
- Public APIs require rustdoc (`missing_docs` is warned); prefer clear module‑level docs.
- Naming: modules/functions `snake_case`, types/traits `CamelCase`, constants `SCREAMING_SNAKE_CASE`.
- Favor deterministic, side‑effect‑free step bodies to preserve replay correctness.

## Testing Guidelines

- Add unit tests in the same module file; name tests `test_*`.
- For async paths, use `#[tokio::test]` or `tokio-test` helpers.
- Keep rustdoc examples compiling: prefer ` ```rust,no_run` over `ignore`, and include minimal `#` scaffolding when needed.

## Commit & Pull Request Guidelines

- History is minimal, so no strict convention yet; follow Conventional Commits when possible (`feat(retry): add jitter`, `fix(runtime): handle empty input`).
- PRs should include: problem/solution summary, linked issue, updated docs/examples if user‑facing, and a clean `cargo fmt --check`, `cargo clippy ... -D warnings`, `cargo test`, and `cargo test --manifest-path examples/Cargo.toml --all-targets` run.
