# Parallel early-completion (“first successful”) example.

Demonstrates:
- `ctx.parallel()` with `CompletionConfig::with_min_successful(1)` to return once any branch succeeds.
- Collecting the first successful branch result from the batch.

Source: `../src/bin/parallel_first_successful/main.rs`

```mermaid
flowchart TD
    n_ce823421_833d_36_start([Start])
    subgraph n_243d4d4bd06262c9["first_successful_parallel"]
        subgraph n_b620026838277bb3["branch-0"]
            n_3744000bc54ac167_Step_6["task1"]
        end
        subgraph n_56d7d0778d1b50df["branch-2"]
            n_d10e5a2a51e331d0_Step_7["task3"]
        end
        subgraph n_8721d76b6fb008b7["branch-1"]
            n_d537e46b8ad2867b_Step_8["task2"]
        end
    end
    n_ce823421_833d_36_end([Success])
    n_ce823421_833d_36_start --> n_243d4d4bd06262c9 --> n_ce823421_833d_36_end
```
