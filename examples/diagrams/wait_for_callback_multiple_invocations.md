# Multiple callbacks in one workflow example.

Demonstrates:
- Two sequential `ctx.wait_for_callback()` operations, separated by other durable ops.
- Durable waits between invocations to make the state machine visible in history.

Source: `../src/bin/wait_for_callback_multiple_invocations/main.rs`

```mermaid
flowchart TD
    n_63f3f5ef_6f5e_3b_start([Start])
    n_4af3ebeabc5509e9_Wait_2[/"wait-invocation-1"/]
    n_4af3ebeabc5509e9_Wait_4[/"wait-invocation-1"/]
    subgraph n_195d29b790850523["first-callback"]
        n_0f9c6eb7b3e52438_Call_6{{"0f9c6eb7"}}
        n_58d9c0035e4bc5a0_Step_7["submitter"]
        n_0f9c6eb7b3e52438_Call_9{{"0f9c6eb7"}}
    end
    n_bbe77925d78dab54_Step_11["process-callback-data"]
    n_a72098fec7002003_Wait_12[/"wait-invocation-2"/]
    n_a72098fec7002003_Wait_14[/"wait-invocation-2"/]
    n_da989984c00dd1c1_Step_15["process-callback-data"]
    n_38e8d1e1cd459e4f_Wait_16[/"wait-invocation-2"/]
    n_38e8d1e1cd459e4f_Wait_18[/"wait-invocation-2"/]
    subgraph n_1d74ad97bfb8508d["second-callback"]
        n_753f4c6485e13f7d_Call_20{{"753f4c64"}}
        n_9f0543c0995e2dfc_Step_21["submitter"]
        n_753f4c6485e13f7d_Call_23{{"753f4c64"}}
    end
    n_63f3f5ef_6f5e_3b_end([Success])
    n_63f3f5ef_6f5e_3b_start --> n_4af3ebeabc5509e9_Wait_2 --> n_4af3ebeabc5509e9_Wait_4 --> n_195d29b790850523 --> n_bbe77925d78dab54_Step_11 --> n_a72098fec7002003_Wait_12 --> n_a72098fec7002003_Wait_14 --> n_da989984c00dd1c1_Step_15 --> n_38e8d1e1cd459e4f_Wait_16 --> n_38e8d1e1cd459e4f_Wait_18 --> n_1d74ad97bfb8508d --> n_63f3f5ef_6f5e_3b_end
```
