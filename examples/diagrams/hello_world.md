# Hello World durable workflow.

Demonstrates:
- `ctx.step()` for deterministic, checkpointed work.
- `ctx.wait()` to suspend/resume without paying for idle compute.

Source: `../src/bin/hello_world/main.rs`

```mermaid
flowchart TD
    n_51318409_5c52_36_start([Start])
    n_05461beee7e26b4a_Step_2["calculate-length"]
    n_3d452b3799674612_Wait_3[/"wait-10s"/]
    n_3d452b3799674612_Wait_5[/"wait-10s"/]
    n_51318409_5c52_36_end([Success])
    n_51318409_5c52_36_start --> n_05461beee7e26b4a_Step_2 --> n_3d452b3799674612_Wait_3 --> n_3d452b3799674612_Wait_5 --> n_51318409_5c52_36_end
```
