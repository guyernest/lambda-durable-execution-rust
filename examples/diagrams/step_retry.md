# Step retries (exponential backoff) workflow.

Demonstrates:
- `StepConfig` + `ExponentialBackoff` retry strategy for transient failures.
- Durable retries: retry decisions and scheduled delays are checkpointed.

Source: `../src/bin/step_retry/main.rs`

```mermaid
flowchart TD
    n_b1ed4064_d014_3c_start([Start])
    n_796a4745d2dd33bf_Step_2["fetch-data"]
    n_796a4745d2dd33bf_Step_3["fetch-data"]
    n_b1ed4064_d014_3c_end([Success])
    n_b1ed4064_d014_3c_start --> n_796a4745d2dd33bf_Step_2 --> n_796a4745d2dd33bf_Step_3 --> n_b1ed4064_d014_3c_end
```
