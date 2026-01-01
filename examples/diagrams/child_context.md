# Child context (“scoped workflow”) example.

Demonstrates:
- `ctx.run_in_child_context()` to group a set of steps under a single parent operation.
- A simple sequential “batch processing” loop inside a child context.

Source: `../src/bin/child_context/main.rs`

```mermaid
flowchart TD
    n_ef821840_1aae_35_start([Start])
    subgraph n_a2cb261984cb89d6["batch-processing-context"]
        n_9064db722a7d5a6c_Step_3["process-item-0"]
        n_9064db722a7d5a6c_Step_4["process-item-0"]
        n_005ac2a074d44344_Step_5["process-item-1"]
        n_005ac2a074d44344_Step_6["process-item-1"]
        n_751f6a019eb9c14d_Step_7["process-item-2"]
        n_751f6a019eb9c14d_Step_8["process-item-2"]
    end
    n_ef821840_1aae_35_end([Success])
    n_ef821840_1aae_35_start --> n_a2cb261984cb89d6 --> n_ef821840_1aae_35_end
```
