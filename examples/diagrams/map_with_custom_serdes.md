# Map fan-out with custom Serdes example.

Demonstrates:
- Supplying an `item_serdes` to `ctx.map()` so each item result is serialized/deserialized with custom logic.
- Returning a JSON summary of per-item processing.

Source: `../src/bin/map_with_custom_serdes/main.rs`

```mermaid
flowchart TD
    n_8f9de01d_18a1_30_start([Start])
    subgraph n_0cb55f59d9c4363b["map_with_custom_serdes"]
    end
    subgraph n_9a1d37f873a99c0f["map_with_custom_serdes-item-0"]
        n_ddc24981e2680935_Step_6["process_0"]
    end
    subgraph n_ac9e7eccc397aa7b["map_with_custom_serdes-item-2"]
        n_690b459ad942b80b_Step_7["process_2"]
    end
    subgraph n_59caf0641f1019b4["map_with_custom_serdes-item-1"]
        n_3288a5c8edaa70a8_Step_8["process_1"]
    end
    n_8f9de01d_18a1_30_end([Success])
    n_8f9de01d_18a1_30_start --> n_0cb55f59d9c4363b --> n_9a1d37f873a99c0f --> n_ac9e7eccc397aa7b --> n_59caf0641f1019b4 --> n_8f9de01d_18a1_30_end
```
