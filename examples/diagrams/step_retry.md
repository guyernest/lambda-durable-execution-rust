# Step retries (exponential backoff) workflow.

Demonstrates:
- `StepConfig` + `ExponentialBackoff` retry strategy for transient failures.
- Durable retries: retry decisions and scheduled delays are checkpointed.

Source: `../src/bin/step_retry/main.rs`

```mermaid
flowchart TD
    n_2af7c57b_7804_3f_start([Start])
    n_796a4745d2dd33bf_Step_2["fetch-data"]
    n_796a4745d2dd33bf_Step_3["fetch-data"]
    n_2af7c57b_7804_3f_end([Success])
    n_2af7c57b_7804_3f_start --> n_796a4745d2dd33bf_Step_2 --> n_796a4745d2dd33bf_Step_3 --> n_2af7c57b_7804_3f_end
```
