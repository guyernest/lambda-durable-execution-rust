# Durable callback (external approval) workflow.

Demonstrates:
- `ctx.wait_for_callback()` to suspend until an external system completes a callback.
- A “submitter” step that would normally notify a human/system with the callback id.

Source: `../src/bin/callback_example/main.rs`

```mermaid
flowchart TD
    n_7e9f1431_56d1_35_start([Start])
    subgraph n_84e6db23691ed51a["wait-for-approval"]
        n_f90b19374af6e392_Call_3{{"f90b1937"}}
        n_f3c00e074eae4944_Step_4["submitter"]
        n_f90b19374af6e392_Call_6{{"f90b1937"}}
    end
    n_6319c6143baf4582_Step_8["execute-approved-action"]
    n_7e9f1431_56d1_35_end([Success])
    n_7e9f1431_56d1_35_start --> n_84e6db23691ed51a --> n_6319c6143baf4582_Step_8 --> n_7e9f1431_56d1_35_end
```
