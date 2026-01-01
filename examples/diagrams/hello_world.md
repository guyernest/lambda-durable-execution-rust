# Hello World durable workflow.

Demonstrates:
- `ctx.step()` for deterministic, checkpointed work.
- `ctx.wait()` to suspend/resume without paying for idle compute.

Source: `../src/bin/hello_world/main.rs`

```mermaid
flowchart TD
    n_12f49d03_2094_3c_start([Start])
    n_05461beee7e26b4a_Step_2["calculate-length"]
    n_05461beee7e26b4a_Step_3["calculate-length"]
    n_3d452b3799674612_Wait_4[/"wait-10s"/]
    n_3d452b3799674612_Wait_6[/"wait-10s"/]
    n_12f49d03_2094_3c_end([Success])
    n_12f49d03_2094_3c_start --> n_05461beee7e26b4a_Step_2 --> n_05461beee7e26b4a_Step_3 --> n_3d452b3799674612_Wait_4 --> n_3d452b3799674612_Wait_6 --> n_12f49d03_2094_3c_end
```
