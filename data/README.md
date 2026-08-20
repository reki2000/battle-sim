# データファイル

`data/`は、バランス値と会戦プリセットをコードから分離するための領域です。
最終的にはTOMLを検証・読込する`sim-data`を導入し、バランス調整に再コンパイルを
要求しない構成を目指します。スキーマの目標は
[データ形式仕様](../docs/spec/10-data-formats.md)、移行計画は[TODO](../TODO.md)を
参照してください。

## 現在の扱い

TOMLの実行時ローダーはまだありません。TOMLは人間が読む定義で、実行時にはRustや
TypeScript側の写しを使います。値を変える場合は、下表の対応先も同時に更新してください。

| データ | 現在の実行時の対応先 | 備考 |
|---|---|---|
| `formations.toml` | `crates/sim-core/src/organization.rs::formation_def` | 自動同期検査なし |
| `factions/medieval_western.toml` | 指揮階層・兵科の参考定義 | 汎用ファクションローダーなし |
| `scenarios/*.toml` | `crates/sim-core/src/scenario.rs`、`web/src/terrain/scenarios.ts` | 陣容と地形設定を手動同期 |
| `terrain/*.bin` | `sim-terrain::fixture` | TypeScript生成器の固定出力。RustのテストとCLIが直接読む |

地形効果は`data/`ではなく`web/src/terrain/effects.ts`を編集元とし、
`crates/sim-terrain/src/effects.rs`へ写しています。両者は
`tools/check_terrain_effects.mjs`で検査します。

`terrain/*.bin`は次のコマンドで生成・検証します。生成器やシナリオ整形を変更した
場合だけ再生成し、バイナリ差分をコミットしてください。

```bash
node tools/gen_terrain_fixtures.mjs
node tools/gen_terrain_fixtures.mjs --check
```

## まだコード内にあるデータ

- 武器、防具、射撃、士気、騎兵、工兵のバランス値
- 兵士品質と能力値分布
- 指揮官アーキタイプとAI重み
- 陣形の実行時定義
- 会戦プリセットの陣容・指揮官・障害物

これらは`sim-data`導入後に、単位と範囲を検証できるTOMLへ移します。移行中も固定小数点の
決定論を保つため、倍率は1000分率、能力値は0〜255、長さ・時間・重量はフィールド名に
`_mm`、`_ms`、`_g`などの単位を含めます。

## 規約

- シミュレーションで読む数値に浮動小数点を使わない。
- 倍率は1000分率（1000 = 等倍）。
- 長さはmm、時間はmsまたはs、重量はgを基本とする。
- 能力値・性格は0〜255。
- データ追加時は、仕様書、実行時の写し、関連fixture・テストを同じ変更で更新する。
