# Wait-for-condition (polling) example.

Demonstrates:
- `ctx.wait_for_condition()` to repeatedly run a step until a stop condition is reached.
- Returning `WaitConditionDecision::Continue { delay }` to suspend between polls.

Source: `../src/bin/wait_for_condition/main.rs`

```mermaid
flowchart TD
    n_2bedd511_1903_39_start([Start])
    n_c1a0793e95bf18a4_Step_2["wait_for_condition"]
    n_c1a0793e95bf18a4_Step_3["wait_for_condition"]
    n_c1a0793e95bf18a4_Step_5["wait_for_condition"]
    n_c1a0793e95bf18a4_Step_7["wait_for_condition"]
    n_2bedd511_1903_39_end([Success])
    n_2bedd511_1903_39_start --> n_c1a0793e95bf18a4_Step_2 --> n_c1a0793e95bf18a4_Step_3 --> n_c1a0793e95bf18a4_Step_5 --> n_c1a0793e95bf18a4_Step_7 --> n_2bedd511_1903_39_end
```
