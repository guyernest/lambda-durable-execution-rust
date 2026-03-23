# Deferred Items - Phase 01

## Pre-existing Issues (Out of Scope)

1. **clippy::io_other_error in map_with_failure_tolerance** - `examples/src/bin/map_with_failure_tolerance/main.rs:54` uses `std::io::Error::new(ErrorKind::Other, ...)` which clippy now recommends replacing with `std::io::Error::other(...)`. Not related to LLM client work.
