# Nested child contexts (“blocks”) example.

Demonstrates:
- `ctx.run_in_child_context()` nesting and parent/child operation hierarchy.
- Mixing steps and waits inside nested contexts.

Source: `../src/bin/block_example/main.rs`

```mermaid
flowchart TD
    n_9fc00198_3291_3c_start([Start])
    subgraph n_2fc18fa04b04d421["parent_block"]
        n_51354675028409e1_Step_3["nested_step"]
        subgraph n_054451e2809a33d6["nested_block"]
            n_247e18f62ba9feda_Wait_5[/"247e18f6"/]
            n_247e18f62ba9feda_Wait_7[/"247e18f6"/]
        end
    end
    n_9fc00198_3291_3c_end([Success])
    n_9fc00198_3291_3c_start --> n_2fc18fa04b04d421 --> n_9fc00198_3291_3c_end
```
