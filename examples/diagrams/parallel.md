# Parallel fan-out (bounded concurrency) example.

Demonstrates:
- `ctx.parallel()` to run multiple branches concurrently (each branch uses durable ops).
- `ParallelConfig::with_max_concurrency()` to bound in-flight branches.

Source: `../src/bin/parallel/main.rs`

```mermaid
flowchart TD
    n_54b813bc_14b9_38_start([Start])
    subgraph n_b9ee7d87a96639b5["parallel_operation"]
        subgraph n_aec3030be12cd730["branch-0"]
            n_c5219dbfbf2f7ca3_Step_5["task1"]
        end
        subgraph n_427432c97bd7ef44["branch-1"]
            n_f4c74e9ee029700b_Step_6["task2"]
        end
        subgraph n_672275512ae66e93["branch-2"]
            n_949492a84e971edf_Wait_10[/"wait_in_task3"/]
            n_949492a84e971edf_Wait_12[/"wait_in_task3"/]
        end
        subgraph n_633f13506eae5b00["branch-2"]
            n_a51b20638e748020_Wait_14[/"wait_in_task3"/]
            n_a51b20638e748020_Wait_16[/"wait_in_task3"/]
        end
    end
    n_54b813bc_14b9_38_end([Success])
    n_54b813bc_14b9_38_start --> n_b9ee7d87a96639b5 --> n_54b813bc_14b9_38_end
```
