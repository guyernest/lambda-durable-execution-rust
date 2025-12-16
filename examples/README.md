# Durable Execution Examples (Rust)

This directory is a standalone Cargo package (`examples/Cargo.toml`) containing deployable AWS Lambda functions that exercise `lambda-durable-execution-rust`. Each example is also validated end-to-end by `examples/scripts/validate.py`, which invokes the deployed functions and generates diagrams from the returned durable execution history.

## Deploy

Prereqs:
- AWS credentials configured (e.g. `aws configure`) and permission to deploy Lambda/SAM + Durable Execution.
- AWS SAM CLI with the Rust `rust-cargolambda` build method enabled (preview).
- `uv` (for running the validator script); for SVG rendering: `playwright install chromium`.

Build and deploy to `us-east-1`:
```bash
sam build -t examples/template.yaml --beta-features
sam deploy -t examples/template.yaml --guided --region us-east-1 --stack-name durable-rust
```

## Validate & regenerate diagrams

Run all deployed examples, save artifacts under `examples/.durable-validation`, and regenerate Mermaid + SVG diagrams under `examples/diagrams/`:
```bash
playwright install chromium
uv run examples/scripts/validate.py \
  --region us-east-1 \
  --stack durable-rust \
  --out examples/.durable-validation \
  --diagrams-out examples/diagrams \
  --mermaid --svg \
  --timeout-seconds 240
```

Notes:
- The Mermaid is generated from the real durable execution history, so node ids may change between runs.
- The SVG files are derived from the Mermaid using `mermaid-cli` (via Playwright).

## Examples

### Hello World (`hello_world`)

A minimal typed workflow: a deterministic `ctx.step()` computes the input name length and then `ctx.wait()` suspends for 10 seconds (so the suspend/resume behavior is easy to see in history). The handler returns a typed JSON response.

- Source: [`src/bin/hello_world/main.rs`](src/bin/hello_world/main.rs)
- Diagram (SVG): [`diagrams/hello_world.svg`](diagrams/hello_world.svg)

```mermaid
flowchart TD
    n_271a1d35_b455_34_start([Start])
    n_1ecf7cb48651031e_Step_2["calculate-length"]
    n_1ecf7cb48651031e_Step_3["calculate-length"]
    n_eff7f71b69df6b6e_Wait_4[/"wait-10s"/]
    n_eff7f71b69df6b6e_Wait_6[/"wait-10s"/]
    n_271a1d35_b455_34_end([Success])
    n_271a1d35_b455_34_start --> n_1ecf7cb48651031e_Step_2 --> n_1ecf7cb48651031e_Step_3 --> n_eff7f71b69df6b6e_Wait_4 --> n_eff7f71b69df6b6e_Wait_6 --> n_271a1d35_b455_34_end
```
---
### Callback / external approval (`callback_example`)

Shows `ctx.wait_for_callback()` for external coordination. The workflow submits an approval request (the “submitter” step) and then suspends until the callback is completed; `examples/scripts/validate.py` completes it using the Lambda callback APIs.

- Source: [`src/bin/callback_example/main.rs`](src/bin/callback_example/main.rs)
- Diagram (SVG): [`diagrams/callback_example.svg`](diagrams/callback_example.svg)

```mermaid
flowchart TD
    n_6975be56_d218_34_start([Start])
    subgraph n_fe6725a4628f543a["wait-for-approval"]
        n_bf55974300301ed0_Call_3{{"bf559743"}}
        n_79060d944e077c96_Step_4["submitter"]
        n_bf55974300301ed0_Call_6{{"bf559743"}}
    end
    n_3846c1f1d539f384_Step_8["execute-approved-action"]
    n_6975be56_d218_34_end([Success])
    n_6975be56_d218_34_start --> n_fe6725a4628f543a --> n_3846c1f1d539f384_Step_8 --> n_6975be56_d218_34_end
```
---
### Step retry (`step_retry`)

Demonstrates a `StepConfig` with an `ExponentialBackoff` retry strategy. Retry decisions and the scheduled delays are durable (checkpointed), so a replay resumes with the same retry schedule.

- Source: [`src/bin/step_retry/main.rs`](src/bin/step_retry/main.rs)
- Diagram (SVG): [`diagrams/step_retry.svg`](diagrams/step_retry.svg)

```mermaid
flowchart TD
    n_5a10c6e4_cd1d_38_start([Start])
    n_57fce957e0952f4e_Step_2["fetch-data"]
    n_57fce957e0952f4e_Step_3["fetch-data"]
    n_5a10c6e4_cd1d_38_end([Success])
    n_5a10c6e4_cd1d_38_start --> n_57fce957e0952f4e_Step_2 --> n_57fce957e0952f4e_Step_3 --> n_5a10c6e4_cd1d_38_end
```
---
### Child context (“scoped workflow”) (`child_context`)

Uses `ctx.run_in_child_context()` to group a set of related steps under a single parent operation (“batch-processing-context”). This is useful for structuring workflows and keeping operation names scoped in the execution history.

- Source: [`src/bin/child_context/main.rs`](src/bin/child_context/main.rs)
- Diagram (SVG): [`diagrams/child_context.svg`](diagrams/child_context.svg)

```mermaid
flowchart TD
    n_217d24b8_a17d_3b_start([Start])
    subgraph n_f97d08f38581da33["batch-processing-context"]
        n_ff050f37daf31dc9_Step_3["process-item-0"]
        n_ff050f37daf31dc9_Step_4["process-item-0"]
        n_4119b1c844bd9e83_Step_5["process-item-1"]
        n_4119b1c844bd9e83_Step_6["process-item-1"]
        n_ff4599e5db8e5937_Step_7["process-item-2"]
        n_ff4599e5db8e5937_Step_8["process-item-2"]
    end
    n_217d24b8_a17d_3b_end([Success])
    n_217d24b8_a17d_3b_start --> n_f97d08f38581da33 --> n_217d24b8_a17d_3b_end
```
---
### Map (bounded concurrency) (`map_operations`)

Illustrates `ctx.map()` for fan-out over a list of items with per-item durable steps. The example uses `MapConfig::with_max_concurrency(2)` to limit in-flight work.

- Source: [`src/bin/map_operations/main.rs`](src/bin/map_operations/main.rs)
- Diagram (SVG): [`diagrams/map_operations.svg`](diagrams/map_operations.svg)

```mermaid
flowchart TD
    n_a8e2ad52_e334_3a_start([Start])
    subgraph n_30850af7bf9484b0["map_operation"]
    end
    subgraph n_71b8c7102674e28a["map_operation-item-0"]
        n_1b4fb9f8c202c684_Step_5["map_item_0"]
    end
    subgraph n_8f4e2373cabbe052["map_operation-item-1"]
        n_3f744a147629986e_Step_6["map_item_1"]
    end
    subgraph n_47a91e323ebe6590["map_operation-item-2"]
        n_304b6a2541e4b69c_Step_11["map_item_2"]
    end
    subgraph n_9ad25a83b32a3d8f["map_operation-item-3"]
        n_19cbd2c80ea9b215_Step_12["map_item_3"]
    end
    subgraph n_2a984ba5ce8ed0f1["map_operation-item-4"]
        n_92f8ebff32714023_Step_16["map_item_4"]
    end
    n_a8e2ad52_e334_3a_end([Success])
    n_a8e2ad52_e334_3a_start --> n_30850af7bf9484b0 --> n_71b8c7102674e28a --> n_8f4e2373cabbe052 --> n_47a91e323ebe6590 --> n_9ad25a83b32a3d8f --> n_2a984ba5ce8ed0f1 --> n_a8e2ad52_e334_3a_end
```
---
### Parallel fan-out (`parallel`)

Uses `ctx.parallel()` to fan out into multiple concurrent branches and gather their results. `ParallelConfig::with_max_concurrency(2)` bounds concurrency, and one branch includes a durable wait to show that waits are replay-safe across suspends.

- Source: [`src/bin/parallel/main.rs`](src/bin/parallel/main.rs)
- Diagram (SVG): [`diagrams/parallel.svg`](diagrams/parallel.svg)

```mermaid
flowchart TD
    n_6891e497_02aa_32_start([Start])
    subgraph n_11ba799566fab7b8["parallel_operation"]
        subgraph n_7cb3f381e6de48ca["branch-1"]
            n_26cd535e5af6bee9_Step_5["task2"]
        end
        subgraph n_2494cda0afe5160f["branch-0"]
            n_0e8b1cd08dbf9483_Step_6["task1"]
        end
        subgraph n_71d72888caa40b24["branch-2"]
            n_5c8206872db3a495_Wait_10[/"wait_in_task3"/]
            n_5c8206872db3a495_Wait_12[/"wait_in_task3"/]
        end
        subgraph n_1c5899bb6f93a838["branch-2"]
            n_7bd41824208d7503_Wait_14[/"wait_in_task3"/]
            n_7bd41824208d7503_Wait_16[/"wait_in_task3"/]
        end
    end
    n_6891e497_02aa_32_end([Success])
    n_6891e497_02aa_32_start --> n_11ba799566fab7b8 --> n_6891e497_02aa_32_end
```
--
### Parallel "first successful" (`parallel_first_successful`)

Runs several branches concurrently but returns as soon as any branch succeeds using `CompletionConfig::with_min_successful(1)`. This pattern is useful for racing alternative strategies and keeping the fastest successful result.

- Source: [`src/bin/parallel_first_successful/main.rs`](src/bin/parallel_first_successful/main.rs)
- Diagram (SVG): [`diagrams/parallel_first_successful.svg`](diagrams/parallel_first_successful.svg)

```mermaid
flowchart TD
    n_07774e8a_ab0b_31_start([Start])
    subgraph n_7eda5b43e4068571["first_successful_parallel"]
        subgraph n_8c4851454e6bdcac["branch-0"]
            n_0e8b1cd08dbf9483_Step_6["task1"]
        end
        subgraph n_61c224cf19b9d2e7["branch-2"]
            n_56b739e81e6ad520_Step_7["task3"]
        end
        subgraph n_7c6ab0009b4bda80["branch-1"]
            n_9934e94bdfa9a1bc_Step_8["task2"]
        end
    end
    n_07774e8a_ab0b_31_end([Success])
    n_07774e8a_ab0b_31_start --> n_7eda5b43e4068571 --> n_07774e8a_ab0b_31_end
```
---
### Map with failure tolerance (`map_with_failure_tolerance`)

Shows `ctx.map()` with `CompletionConfig::with_tolerated_failures(3)` so the batch can complete even if a few items fail. In this example, items divisible by 3 fail and are recorded in the batch result instead of aborting the workflow.

- Source: [`src/bin/map_with_failure_tolerance/main.rs`](src/bin/map_with_failure_tolerance/main.rs)
- Diagram (SVG): [`diagrams/map_with_failure_tolerance.svg`](diagrams/map_with_failure_tolerance.svg)

```mermaid
flowchart TD
    n_46b16856_df65_3f_start([Start])
    subgraph n_93472deddf9066f9["map_with_tolerance"]
    end
    subgraph n_a900ee0ac6c58756["map_with_tolerance-item-0"]
        n_236eca427148f2df_Step_8["item_0"]
    end
    subgraph n_6e39ce714e6dbf50["map_with_tolerance-item-3"]
        n_58dcf0d91ecf5848_Step_9["item_3"]
    end
    subgraph n_27ec189e482d9946["map_with_tolerance-item-4"]
        n_f8e280267e20521e_Step_10["item_4"]
    end
    subgraph n_bda33116c97db9be["map_with_tolerance-item-1"]
        n_cd10f8b76ba8eb9b_Step_11["item_1"]
    end
    subgraph n_91d91fd2f4d7654f["map_with_tolerance-item-2"]
        n_b8fdde29fcc63e7b_Step_12["item_2"]
    end
    subgraph n_9ba8d1a73d5afb19["map_with_tolerance-item-5"]
        n_5b4bdd4b632c03fa_Step_23["item_5"]
    end
    subgraph n_84ba9b450c25a759["map_with_tolerance-item-6"]
        n_7c80051c41d4ae47_Step_24["item_6"]
    end
    subgraph n_91c789da0a3e2798["map_with_tolerance-item-7"]
        n_7b8907a1ad2cf9a0_Step_25["item_7"]
    end
    subgraph n_2694e95ed13905a6["map_with_tolerance-item-8"]
        n_1bf94bf2f8778b16_Step_26["item_8"]
    end
    subgraph n_f8dfeaeed0c96d54["map_with_tolerance-item-9"]
        n_026756bd0bde5d4b_Step_27["item_9"]
    end
    n_46b16856_df65_3f_end([Success])
    n_46b16856_df65_3f_start --> n_93472deddf9066f9 --> n_a900ee0ac6c58756 --> n_6e39ce714e6dbf50 --> n_27ec189e482d9946 --> n_bda33116c97db9be --> n_91d91fd2f4d7654f --> n_9ba8d1a73d5afb19 --> n_84ba9b450c25a759 --> n_91c789da0a3e2798 --> n_2694e95ed13905a6 --> n_f8dfeaeed0c96d54 --> n_46b16856_df65_3f_end
```
---
### Wait-for-condition (polling) (`wait_for_condition`)

Demonstrates `ctx.wait_for_condition()` to poll state until a stop condition is reached. The step increments a counter, and the decision function returns `Continue { delay }` to suspend between polls.

- Source: [`src/bin/wait_for_condition/main.rs`](src/bin/wait_for_condition/main.rs)
- Diagram (SVG): [`diagrams/wait_for_condition.svg`](diagrams/wait_for_condition.svg)

```mermaid
flowchart TD
    n_b71faad3_07aa_3b_start([Start])
    n_d00717593d6f0cd4_Step_2["wait_for_condition"]
    n_d00717593d6f0cd4_Step_3["wait_for_condition"]
    n_d00717593d6f0cd4_Step_5["wait_for_condition"]
    n_d00717593d6f0cd4_Step_7["wait_for_condition"]
    n_b71faad3_07aa_3b_end([Success])
    n_b71faad3_07aa_3b_start --> n_d00717593d6f0cd4_Step_2 --> n_d00717593d6f0cd4_Step_3 --> n_d00717593d6f0cd4_Step_5 --> n_d00717593d6f0cd4_Step_7 --> n_b71faad3_07aa_3b_end
```
---
### Map with custom item Serdes (`map_with_custom_serdes`)

Shows how to attach an item-level `Serdes` to `ctx.map()` so each per-item result uses custom serialization/deserialization logic (useful for versioning or interop with existing payload formats). The example returns a JSON summary of the processed items.

- Source: [`src/bin/map_with_custom_serdes/main.rs`](src/bin/map_with_custom_serdes/main.rs)
- Diagram (SVG): [`diagrams/map_with_custom_serdes.svg`](diagrams/map_with_custom_serdes.svg)

```mermaid
flowchart TD
    n_eb3bab03_a06d_38_start([Start])
    subgraph n_bb06303c671e8dfa["map_with_custom_serdes"]
    end
    subgraph n_854884afb82c9f52["map_with_custom_serdes-item-0"]
        n_f1a910c1de6193aa_Step_6["process_0"]
    end
    subgraph n_5a6489d72c7e1cb5["map_with_custom_serdes-item-2"]
        n_aadf1ef32ebcbc6f_Step_7["process_2"]
    end
    subgraph n_fcd4eddceb034fd3["map_with_custom_serdes-item-1"]
        n_e2fe10d2ffa3acc2_Step_8["process_1"]
    end
    n_eb3bab03_a06d_38_end([Success])
    n_eb3bab03_a06d_38_start --> n_bb06303c671e8dfa --> n_854884afb82c9f52 --> n_5a6489d72c7e1cb5 --> n_fcd4eddceb034fd3 --> n_eb3bab03_a06d_38_end
```
---
### Wait-for-callback with heartbeat timeout (`wait_for_callback_heartbeat`)

Demonstrates configuring `CallbackConfig` with both a total timeout and a heartbeat timeout. The workflow suspends in `ctx.wait_for_callback()` until the callback is completed (the validator completes it), and the timeouts protect against forgotten callbacks.

- Source: [`src/bin/wait_for_callback_heartbeat/main.rs`](src/bin/wait_for_callback_heartbeat/main.rs)
- Diagram (SVG): [`diagrams/wait_for_callback_heartbeat.svg`](diagrams/wait_for_callback_heartbeat.svg)

```mermaid
flowchart TD
    n_ccb90602_a7e2_30_start([Start])
    subgraph n_27a519ae665a6f0a["WaitForCallback"]
        n_bf55974300301ed0_Call_3{{"bf559743"}}
        n_79060d944e077c96_Step_4["submitter"]
        n_79060d944e077c96_Step_5["submitter"]
        n_bf55974300301ed0_Call_7{{"bf559743"}}
    end
    n_ccb90602_a7e2_30_end([Success])
    n_ccb90602_a7e2_30_start --> n_27a519ae665a6f0a --> n_ccb90602_a7e2_30_end
```

---
### Multiple callbacks in one workflow (`wait_for_callback_multiple_invocations`)

Executes two `ctx.wait_for_callback()` operations sequentially, with durable waits and a step in between. This illustrates how callback ids and completions are scoped and recorded across multiple external interactions.

- Source: [`src/bin/wait_for_callback_multiple_invocations/main.rs`](src/bin/wait_for_callback_multiple_invocations/main.rs)
- Diagram (SVG): [`diagrams/wait_for_callback_multiple_invocations.svg`](diagrams/wait_for_callback_multiple_invocations.svg)

```mermaid
flowchart TD
    n_8098264a_445b_31_start([Start])
    n_17c274e64229e5be_Wait_2[/"wait-invocation-1"/]
    n_17c274e64229e5be_Wait_4[/"wait-invocation-1"/]
    subgraph n_ff4512d1af6d286c["first-callback"]
        n_762f74c01672d4b5_Call_6{{"762f74c0"}}
        n_6b399a48552e0750_Step_7["submitter"]
        n_762f74c01672d4b5_Call_9{{"762f74c0"}}
    end
    n_4b3cf5cac5326de4_Step_11["process-callback-data"]
    n_4b96114fdcef8408_Wait_12[/"wait-invocation-2"/]
    n_4b96114fdcef8408_Wait_14[/"wait-invocation-2"/]
    n_3b97458d6e28ab19_Step_15["process-callback-data"]
    n_730cfa75006c3705_Wait_16[/"wait-invocation-2"/]
    n_730cfa75006c3705_Wait_18[/"wait-invocation-2"/]
    subgraph n_ccbfb1c4cb16c10e["second-callback"]
        n_56ae2f4d7b143e34_Call_20{{"56ae2f4d"}}
        n_447ec07d7a602c60_Step_21["submitter"]
        n_56ae2f4d7b143e34_Call_23{{"56ae2f4d"}}
    end
    n_8098264a_445b_31_end([Success])
    n_8098264a_445b_31_start --> n_17c274e64229e5be_Wait_2 --> n_17c274e64229e5be_Wait_4 --> n_ff4512d1af6d286c --> n_4b3cf5cac5326de4_Step_11 --> n_4b96114fdcef8408_Wait_12 --> n_4b96114fdcef8408_Wait_14 --> n_3b97458d6e28ab19_Step_15 --> n_730cfa75006c3705_Wait_16 --> n_730cfa75006c3705_Wait_18 --> n_ccbfb1c4cb16c10e --> n_8098264a_445b_31_end
```

---
### Nested child contexts ("blocks") (`block_example`)

Shows nested `ctx.run_in_child_context()` calls to build a parent/child hierarchy in the workflow history. This pattern is useful for grouping operations (and their steps/waits) under meaningful names.

- Source: [`src/bin/block_example/main.rs`](src/bin/block_example/main.rs)
- Diagram (SVG): [`diagrams/block_example.svg`](diagrams/block_example.svg)

```mermaid
flowchart TD
    n_4e2c46bb_bab6_38_start([Start])
    subgraph n_a4009b439a71f8ca["parent_block"]
        n_90a91fb3a13f1bed_Step_3["nested_step"]
        subgraph n_9b9dcaf2ae8e1bb5["nested_block"]
            n_478786cb75e45f14_Wait_5[/"478786cb"/]
            n_478786cb75e45f14_Wait_7[/"478786cb"/]
        end
    end
    n_4e2c46bb_bab6_38_end([Success])
    n_4e2c46bb_bab6_38_start --> n_a4009b439a71f8ca --> n_4e2c46bb_bab6_38_end
```
---
### Durable invoke target (`invoke_target`)

A small durable workflow intended to be called by another workflow via `ctx.invoke()`. It performs work inside a checkpointed `ctx.step()` and returns a typed response.

- Source: [`src/bin/invoke_target/main.rs`](src/bin/invoke_target/main.rs)
- Diagram (SVG): [`diagrams/invoke_target.svg`](diagrams/invoke_target.svg)

```mermaid
flowchart TD
    n_11bde4ac_ba55_3b_start([Start])
    n_508ad35a7ed0dc26_Step_2["double"]
    n_11bde4ac_ba55_3b_end([Success])
    n_11bde4ac_ba55_3b_start --> n_508ad35a7ed0dc26_Step_2 --> n_11bde4ac_ba55_3b_end
```

---
### Durable invoke caller (`invoke_caller`)

Demonstrates `ctx.invoke()` as a durable operation: the workflow invokes `invoke_target` (configured via `INVOKE_TARGET_FUNCTION` in the SAM template), then uses the returned value in a follow-up step.

- Source: [`src/bin/invoke_caller/main.rs`](src/bin/invoke_caller/main.rs)
- Diagram (SVG): [`diagrams/invoke_caller.svg`](diagrams/invoke_caller.svg)

```mermaid
flowchart TD
    n_28ae61e7_d4db_3c_start([Start])
    n_951709eef4ac0152_Chai_2["invoke: invoke-target"]
    n_951709eef4ac0152_Chai_4["invoke: invoke-target"]
    n_0fcc02defa36533c_Step_5["plus-one"]
    n_28ae61e7_d4db_3c_end([Success])
    n_28ae61e7_d4db_3c_start --> n_951709eef4ac0152_Chai_2 --> n_951709eef4ac0152_Chai_4 --> n_0fcc02defa36533c_Step_5 --> n_28ae61e7_d4db_3c_end
```
