# Durable invoke target example.

Demonstrates:
- A simple durable function designed to be called via `ctx.invoke()` from another workflow.
- Performing work inside a checkpointed `ctx.step()`.

Source: `../src/bin/invoke_target/main.rs`

```mermaid
flowchart TD
    n_b5d429d8_cc62_36_start([Start])
    n_7b1d6c47bf0205bd_Step_2["double"]
    n_b5d429d8_cc62_36_end([Success])
    n_b5d429d8_cc62_36_start --> n_7b1d6c47bf0205bd_Step_2 --> n_b5d429d8_cc62_36_end
```
