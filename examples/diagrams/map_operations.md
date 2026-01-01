# Map fan-out (bounded concurrency) example.

Demonstrates:
- `ctx.map()` to process a list of items with per-item durable steps.
- `MapConfig::with_max_concurrency()` to limit in-flight work.

Source: `../src/bin/map_operations/main.rs`

```mermaid
flowchart TD
    n_2c92aa32_8b03_3e_start([Start])
    subgraph n_7bdd6441b72db58d["map_operation"]
    end
    subgraph n_47a1a23a26aa4b33["map_operation-item-0"]
        n_8567f8107737fa2a_Step_5["map_item_0"]
    end
    subgraph n_de1e0ab9cdf321ce["map_operation-item-1"]
        n_8ee9dfdec16985fb_Step_6["map_item_1"]
    end
    subgraph n_485143bc380459a2["map_operation-item-2"]
        n_f0b39b6362762aff_Step_11["map_item_2"]
    end
    subgraph n_6231d9a89ea147d7["map_operation-item-3"]
        n_1a0aedfe53209250_Step_12["map_item_3"]
    end
    subgraph n_23a461cac058646b["map_operation-item-4"]
        n_aa4478b92bf38dab_Step_16["map_item_4"]
    end
    n_2c92aa32_8b03_3e_end([Success])
    n_2c92aa32_8b03_3e_start --> n_7bdd6441b72db58d --> n_47a1a23a26aa4b33 --> n_de1e0ab9cdf321ce --> n_485143bc380459a2 --> n_6231d9a89ea147d7 --> n_23a461cac058646b --> n_2c92aa32_8b03_3e_end
```
