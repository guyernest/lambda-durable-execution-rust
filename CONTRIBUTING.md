# Contributing

Thank you for your interest in improving the Lambda Durable Execution Rust SDK. This repository is experimental. Contributions should favor clarity, determinism, and reproducibility.

## Code of conduct

This project follows the AWS Open Source Code of Conduct.

## Development setup

- Rust 1.88 or newer (edition 2021).
- `cargo fmt` and `cargo clippy` available.

## Build and test

```bash
cargo fmt
cargo test
cargo clippy --all-targets --all-features -D warnings

# Examples package
cargo test --manifest-path examples/Cargo.toml --all-targets
```

## Design constraints

- Step bodies should be deterministic and side-effect-free.
- Use durable operations to express external side effects.
- Keep replay logic and execute logic separate when adding new operations.

## Documentation

- Public APIs require rustdoc.
- Rustdoc examples should compile with `no_run` and minimal scaffolding.
- Keep README and ARCHITECTURE updates consistent with code changes.

## Licensing

By submitting a contribution, you agree to license it under Apache-2.0.
