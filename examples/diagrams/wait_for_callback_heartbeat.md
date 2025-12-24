# Wait-for-callback with heartbeat timeout example.

Demonstrates:
- `ctx.wait_for_callback()` with both a total timeout and a heartbeat timeout.
- Suspending until the callback is completed (or times out).

Source: `../src/bin/wait_for_callback_heartbeat/main.rs`

```mermaid
flowchart TD
    n_a2f1c95e_666c_3f_start([Start])
    subgraph n_4ca9bead5a86eddd["WaitForCallback"]
        n_f90b19374af6e392_Call_3{{"f90b1937"}}
        n_f3c00e074eae4944_Step_4["submitter"]
        n_f3c00e074eae4944_Step_5["submitter"]
        n_f90b19374af6e392_Call_7{{"f90b1937"}}
    end
    n_a2f1c95e_666c_3f_end([Success])
    n_a2f1c95e_666c_3f_start --> n_4ca9bead5a86eddd --> n_a2f1c95e_666c_3f_end
```
