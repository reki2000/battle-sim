# battle-sim

中世ヨーロッパの会戦を、**兵士 1 人単位・最大数万人**でブラウザ上でシミュレートする。

地形はシードから自動生成され、司令官・隊長・兵士がそれぞれ独立した思考ルーチンを
持って行動する。会戦の勝敗は上から与えられた戦力比ではなく、**個々の兵士の士気が
崩れて敗走が伝染する過程**から創発する。

- Rust → wasm（シミュレーション）＋ TypeScript（UI・描画）
- 固定小数点による完全決定論。シードと命令ログだけで会戦全体を再現できる
- クォータービュー。5 km 四方から 10 m 四方まで連続してズームできる
- 観戦するだけでなく、任意の指揮官に憑依して命令を出せる（命令は伝令で遅れて届く）

## 仕様書

設計はすべて [`docs/spec/`](docs/spec/) にある。実装より仕様が先。

| # | 文書 |
|---|---|
| — | [目次と設計方針](docs/spec/README.md) |
| 00 | [概要とスコープ](docs/spec/00-overview.md) |
| 01 | [システムアーキテクチャ](docs/spec/01-architecture.md) |
| 02 | [シミュレーションコア](docs/spec/02-simulation-core.md) |
| 03 | [地形生成](docs/spec/03-terrain.md) |
| 04 | [軍の編成と指揮系統](docs/spec/04-organization.md) |
| 05 | [AI と思考ルーチン](docs/spec/05-ai.md) |
| 06 | [戦闘と士気](docs/spec/06-combat.md) |
| 07 | [工兵](docs/spec/07-engineers.md) |
| 08 | [レンダリング](docs/spec/08-rendering.md) |
| 09 | [UI と命令系](docs/spec/09-ui.md) |
| 10 | [データ形式](docs/spec/10-data-formats.md) |
| 11 | [性能目標と予算](docs/spec/11-performance.md) |
| 12 | [ロードマップ](docs/spec/12-roadmap.md) |

## 現在の状態: M0（基盤）

ビルドが通る雛形まで。実装済みなのは以下。

- 固定小数点数学（`Fx`、`Vec2Fx`、`Brad`、整数平方根、事前生成の三角関数テーブル）
- 決定論的な PRNG（エンティティ・目的・ティックからストリームを導出）
- 地形生成の骨格（ノイズ、熱浸食、地表分類、通行コスト）
- 兵士の SoA レイアウト、空間ハッシュ、移動積分、衝突解決（押し合い）
- wasm 境界と描画スナップショット
- クォータービューのカメラと Canvas2D の疎通確認版レンダラ
- CLI（ベンチ・決定論検証・地形統計）と CI

**まだ無い**もの: 指揮ツリー、AI、白兵戦、射撃、士気、騎兵、工兵、命令 UI。
順序は [ロードマップ](docs/spec/12-roadmap.md) を参照。

### 実測値（M0 時点、開発機での参考値）

移動と衝突のみ（AI・戦闘は未実装）。

| 兵数 | ms / tick | 実時間 20 Hz に対する余裕 |
|---|---|---|
| 20,000 | 5.1 | 9.8x |
| 50,000 | 11.5 | 4.4x |

## セットアップ

```bash
# 前提: Rust 1.75+, Node 22+
rustup target add wasm32-unknown-unknown
cargo install wasm-pack --locked
```

## ビルドと実行

```bash
# 1. ネイティブのテスト（ブラウザなしで全ロジックを検証できる）
cargo test --workspace

# 2. wasm をビルドして web/src/wasm/ に出す
wasm-pack build crates/sim-wasm --target web \
  --out-dir ../../web/src/wasm --out-name sim

# 3. 開発サーバ
cd web && npm install && npm run dev
```

`web/package.json` の `npm run wasm` は手順 2 のショートカット。

### 操作

| 入力 | 動作 |
|---|---|
| ドラッグ | パン |
| ホイール | ズーム（対数、カーソル位置を固定） |
| Space | 一時停止 / 再開 |
| 1〜5 | 速度 1x / 2x / 4x / 8x / 16x |
| Q W E R T | 視野 5 km / 1 km / 200 m / 40 m / 10 m |

## CLI

`sim-headless` はブラウザなしでシミュレーションを回す。バランス調整・性能計測・
決定論の回帰テストはこれで行う。

```bash
# 性能を測る
cargo run --release -p sim-headless -- bench --soldiers 20000 --ticks 500

# 同じシードで結果が一致するか検証する
cargo run --release -p sim-headless -- verify --soldiers 5000 --ticks 5000

# 地形の統計を見る
cargo run --release -p sim-headless -- terrain --size 2000 --relief 450
```

## ブラウザでの疎通確認

全ズーム域（5 km 〜 10 m）でエラーなく描けることを確認し、
各 LOD のスクリーンショットを `web/smoke-out/` に出す。

```bash
cd web
npm run build
npx vite preview --port 4173 &
npm run smoke
```

## リポジトリ構成

```
crates/
├── sim-math/       固定小数点数学。浮動小数点を一切使わない
├── sim-terrain/    地形生成
├── sim-core/       ワールド状態と全システム。wasm 非依存
├── sim-wasm/       wasm-bindgen の境界。ロジックは持たない
└── sim-headless/   CLI（ベンチ・検証）
web/
├── src/
│   ├── sim/        Worker、wasm ブリッジ、スナップショットの読み取り
│   ├── render/     クォータービュー変換、地形、兵士
│   └── main.ts     エントリ
└── tools/          ブラウザ疎通確認
docs/spec/          仕様書
data/               バランス調整用の数値（M3 以降）
tools/              三角関数テーブル生成、浮動小数点の混入チェック
```

## 開発上の規約

仕様書の [非交渉事項](docs/spec/00-overview.md#5-設計上の非交渉事項) がすべてに優先する。
特に次の 2 つは CI が機械的に検査する。

1. **`sim-math` / `sim-core` / `sim-terrain` に浮動小数点を書かない。**
   決定論はプラットフォームに依らない整数演算だけで保たれる。
   例外は `sim-core/src/snapshot.rs`（描画用の変換）のみ。
   `tools/check_no_float.sh` が検査する。

2. **同じシードからは常に同じ結果が出る。**
   `sim-headless verify` が全ティックの状態ハッシュを突き合わせる。

その他:

- `HashMap` をシミュレーション状態の走査に使わない（イテレーション順が不定）。
- 乱数は必ず `Rng::stream(seed, entity, purpose, tick)` から引く。
  グローバルな RNG 状態を持たない。
- 各システムは「前フェーズの確定値を読み、自分の出力配列に書く」。
  同一フェーズ内で他エンティティの更新後の値を読まない（並列化の前提）。
- 描画はシミュレーションを一切変更しない。レンダラは読み取り専用。

## ライセンス

MIT
