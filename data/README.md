# データファイル

シミュレーションの数値はすべてここに置く。バランス調整に再コンパイルを要求しない
（仕様 `docs/spec/00-overview.md` 非交渉事項 5）。

スキーマは `docs/spec/10-data-formats.md` を参照。

## 現状（M3）

M3 では編成・陣形のプリセットを追加した。読み込み機構（`sim-data` クレート）は
まだ未実装のため、シミュレーション実行時の既定値は Rust 側にも保持している。
人物描画はRust/Wasmの状態保持型ポリゴンエンジンが担当する。`troop_type` は
兵科別の配色・装備種別indexとしてスナップショットへ載せる。

| 仕様上のファイル | 現在の置き場所 |
|---|---|
| `terrain_surfaces.toml` | `crates/sim-terrain/src/lib.rs` の `SURFACE_EFFECTS` |
| `qualities.toml` | `crates/sim-core/src/lib.rs` の `deploy_block` |
| `formations.toml` | `crates/sim-core/src/organization.rs` の `formation_def` |
| `factions/medieval_western.toml` | M3 のプリセットデータ |
| `morale.toml` | `crates/sim-core/src/soldiers.rs` の初期士気 |
| `scenarios/*.toml` | `crates/sim-core/src/scenario.rs` の会戦プリセット定数 |

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
├── factions/
└── scenarios/
```

会戦プリセット（`scenarios/`）も同じ扱いで、TOML が人間の読む正本、実際に読まれる
のは Rust 側の写し。数値を変えるときは両方を直す。解釈できる範囲は
`docs/spec/10-data-formats.md` 10 節「実装済みの範囲」を参照。

## 規約

- 倍率はすべて 1000 分率の整数（1000 = 等倍）。浮動小数点は書かない。
- 長さは mm、時間は ms または s、重量は g。単位をフィールド名に含める
  （`reach_mm`, `weight_g`, `reload_ms`）。
- 能力値・性格は 0..=255。分布は `[平均, 標準偏差]` の 2 要素配列。
- 追加した値は必ず仕様書の該当章の表にも反映する。
