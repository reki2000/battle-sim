# データファイル

シミュレーションの数値はすべてここに置く。バランス調整に再コンパイルを要求しない
（仕様 `docs/spec/00-overview.md` 非交渉事項 5）。

スキーマは `docs/spec/10-data-formats.md` を参照。

## 現状（M0）

M0 時点では読み込み機構（`sim-data` クレート）が未実装で、値は Rust のコード内に
既定値として持っている。

| 仕様上のファイル | M0 での置き場所 |
|---|---|
| `terrain_surfaces.toml` | `crates/sim-terrain/src/lib.rs` の `SURFACE_EFFECTS` |
| `qualities.toml` | `crates/sim-core/src/lib.rs` の `deploy_block` |
| `morale.toml` | `crates/sim-core/src/soldiers.rs` の初期士気 |

M3（編成）で `sim-data` を追加し、以下を順に外へ出す。

```
data/
├── weapons.toml
├── armor.toml
├── shields.toml
├── troop_types.toml
├── formations.toml
├── qualities.toml
├── archetypes.toml
├── ai_weights.toml
├── morale.toml
├── terrain_surfaces.toml
├── engineer_tasks.toml
├── sprite_sets.toml
├── factions/
└── scenarios/
```

## 規約

- 倍率はすべて 1000 分率の整数（1000 = 等倍）。浮動小数点は書かない。
- 長さは mm、時間は ms または s、重量は g。単位をフィールド名に含める
  （`reach_mm`, `weight_g`, `reload_ms`）。
- 能力値・性格は 0..=255。分布は `[平均, 標準偏差]` の 2 要素配列。
- 追加した値は必ず仕様書の該当章の表にも反映する。
