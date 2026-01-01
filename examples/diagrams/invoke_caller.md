# Durable invoke caller example.

Demonstrates:
- `ctx.invoke()` to call another Lambda function as a durable operation.
- Passing a typed payload to the target and using the typed result in a follow-up step.

Source: `../src/bin/invoke_caller/main.rs`

```mermaid
flowchart TD
    n_d4f1f0d7_10b8_3b_start([Start])
    n_f8d3064885e25fbe_Chai_2["invoke: invoke-target"]
    n_f8d3064885e25fbe_Chai_4["invoke: invoke-target"]
    n_abdb54874f407baf_Step_5["plus-one"]
    n_d4f1f0d7_10b8_3b_end([Success])
    n_d4f1f0d7_10b8_3b_start --> n_f8d3064885e25fbe_Chai_2 --> n_f8d3064885e25fbe_Chai_4 --> n_abdb54874f407baf_Step_5 --> n_d4f1f0d7_10b8_3b_end
```
