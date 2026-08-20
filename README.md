# battle-sim

中世ヨーロッパの会戦を、兵士1人ずつの状態と判断から組み立てるブラウザ向け
シミュレータです。生成済みの整数地形上で、指揮の遅延、疲労、士気、白兵・射撃、
騎兵、工兵を固定小数点の決定論的シミュレーションとしてRustで実行し、
TypeScriptとWebGL2で観戦・操作します。

- Rust / WebAssembly: シミュレーション、人物ポリゴン生成
- TypeScript / Web Worker: 地形生成、キャッシュ、シミュレーション実行制御
- WebGL2 / Canvas2D / HTML: 地形、兵士、命令、ミニマップ、UI
- 同じ整数地形、シード、命令ログから同じ状態ハッシュを再現
- 5 km〜10 mの連続ズームと、指揮官への憑依・命令・兵士追従

設計目標は[仕様書](docs/spec/README.md)、未実装事項と設計改善は
[TODO.md](TODO.md)を参照してください。仕様書のM0〜M9は目標の区切りであり、
完了済みマイルストーンの表示ではありません。

## 現在の実装

現状は、M0〜M8で定義した主要経路を一通り実装し、M9の最適化を部分的に
取り込んだプロトタイプです。各マイルストーンの受け入れ条件をすべて満たした
完成版ではありません。

| 領域 | 実装済み | 主な未実装・制約 |
|---|---|---|
| シミュレーション基盤 | 固定小数点、決定論的PRNG、兵士SoA、空間ハッシュ、衝突、経路探索、連続移動する陣形アンカー、歩行時の弾性間隔、疲労、旋回、死体障害物 | 50,000体・4倍速の性能目標は未達成、並列実行なし |
| 戦闘 | 白兵、射撃、装甲、士気伝播、敗走・追撃、圧迫、徒歩突撃、騎兵突撃・忌避・落馬、戦列交代 | 武器の持ち替え、装備重量と泥の連動など |
| 指揮とAI | N階層指揮ツリー、伝令・旗・角笛、命令遅延・損失・遵守判定、継承、Blackboard、戦況評価、会戦プラン、判断ログ、位相分散した兵士の局所迎撃・対象分散、人物追跡・区域占領・区域防衛 | 条件付き命令、個体ユーティリティAIの全候補行動、仕様上の全AI分解規則 |
| 工兵 | 杭・堀・鹿砦・土塁・馬防柵、架橋、伐採、矢の補給、負傷者回収、準備時間の比較CLI | 天候・工具、攻城兵器、パヴィス配線、Objectiveからの自動タスク生成 |
| 地形 | Worker内のシード生成、湖・海・湿度・植生・崖判定・道路・会戦地評価、シナリオ整形、固定地形、IndexedDBキャッシュ | 生成器は浮動小数点のため配布fixtureを整数で固定、河川生成は廃止、実際の崖側面ジオメトリなし、5 km生成は性能目標超過 |
| 描画とUI | WebGL2地形、Rust/Wasm人物ポリゴン、モーション・LOD・カリング、ミニマップ、憑依、命令UI、追従、観戦、日英表示、品質設定、リプレイ保存・読込 | 巻き戻しタイムライン、統計画面の拡充 |
| 配布 | 通常のViteビルド、`file://`で開ける単一HTML | 単一HTMLには地形アトラスを含めず、手続き色へフォールバック |

M9関連では、wasmの`simd128`ターゲット機能、`tickMany`、描画カリング、品質
プリセット、地形キャッシュまで実装しています。`SharedArrayBuffer`、wasm threads、
ゼロコピーのスナップショット転送は未実装で、現在は再利用する`ArrayBuffer`へ
コピーしてWorkerから転送します。

## 会戦プリセット

UI左上のプルダウン、または`?scenario=agincourt_1415`のようなURL指定で選べます。
兵数と会戦場幅はブラウザ実行向けに縮尺しています。

| ID | 会戦 | 配置兵数 | 地形・戦術上の特徴 |
|---|---|---:|---|
| `agincourt_1415` | アジンクール 1415 | 4,525 | 森に挟まれた泥濘、杭列、長弓、分裂した指揮 |
| `crecy_1346` | クレシー 1346 | 4,050 | 緩斜面、長弓と弩の射程差、落とし穴、騎兵 |
| `bannockburn_1314` | バノックバーン 1314 | 3,680 | 小川と低湿地、シルトロン、展開しにくい大軍 |

プリセットは地形・陣容・指揮官の性格を初期化しますが、戦闘の筋書きや命令は
埋め込んでいません。開始後の行動は指揮官AIの性格、認識、会戦プランから決まります。

```bash
# 一覧を表示
cargo run --release -p sim-headless -- scenario

# ブラウザなしでプリセットを実行
cargo run --release -p sim-headless -- scenario --scenario agincourt_1415
```

## 必要な環境

- Rust 1.75以上
- `wasm32-unknown-unknown`ターゲット
- `wasm-pack`
- Node.js 22以上とnpm
- WebGL2対応ブラウザ

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-pack --locked
```

環境固有の起動方法が`TOOLS.md`にある場合は、Rust・Node・npm・Wasm・Pythonの
コマンドを実行する前にそちらを優先してください。`TOOLS.md`はローカル専用で、
Gitには追加しません。

## ビルドと起動

```bash
# 1. Rustのテスト
cargo test --workspace

# 2. Web依存を固定バージョンで導入
cd web
npm ci

# 3. wasmを web/src/wasm/ へ生成
npm run wasm

# 4. 開発サーバ
npm run dev
```

`web/src/wasm/`は生成物で、Git管理外です。Rust側のWasm公開APIを変更した場合は、
再生成してから型検査・Webビルドを実行してください。

### 操作

| 入力 | 動作 |
|---|---|
| ドラッグ | カメラをパン |
| ピンチ / ホイール | カーソル位置を保ってズーム |
| Space | 一時停止 / 再開 |
| 1〜5 | 速度 1x / 2x / 4x / 8x / 16x |
| Q / W / E / R / T | 視野幅 5 km / 1 km / 200 m / 40 m / 10 m |

指揮ツリーのノードを選ぶと憑依し、直属部隊へ移動・攻撃・突撃・側面・退却・
射撃・人物追跡・区域占領・区域防衛・築城などを命令できます。兵士を選ぶと
詳細表示と追従ができます。

## ヘッドレスCLI

`sim-headless`は、ブラウザなしで決定論、バランス、性能、固定地形を検証します。

```bash
# 決定論: 同じシードの全tickで状態ハッシュが一致するか
cargo run --release -p sim-headless -- verify --soldiers 5000 --ticks 5000

# 対称会戦と勝率
cargo run --release -p sim-headless -- battle --soldiers 4000 --seed 1
cargo run --release -p sim-headless -- winrate --soldiers 2000 --runs 200

# 準備時間の長短による防御側勝率の比較
cargo run --release -p sim-headless -- prep --soldiers 2000 --runs 100

# 性能と固定地形の統計
cargo run --release -p sim-headless -- bench --soldiers 20000 --ticks 500
cargo run --release -p sim-headless -- terrain --scenario agincourt_1415
```

性能値は必ず`--release`で測ってください。devプロファイルの固定小数点演算は、
実運用の性能指標になりません。

## 検証

```bash
# Rust
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./tools/check_no_float.sh

# Rust/TypeScript間の地形効果テーブル
node tools/check_terrain_effects.mjs

# Web
cd web
npm run typecheck
npm run build
```

ブラウザ疎通確認は、別ターミナルで`npm run preview -- --port 4173`を起動してから
実行します。スクリーンショットは`web/smoke-out/`へ出力され、Git管理外です。

```bash
npm run smoke               # 全ズーム域
npm run smoke:scenario      # 全会戦プリセット
npm run smoke:ui            # 憑依・命令・追従・観戦・リプレイ
npm run smoke:cache         # IndexedDB地形キャッシュ
npm run smoke:performance   # 50,000体の手動性能疎通
```

## 単一HTML

wasmとclassic Workerを埋め込んだ`web/dist/battle-sim.html`を生成できます。

```bash
cd web
npm run build:single
npm run smoke:single
```

単一HTMLは`file://`から起動できます。地形アトラスPNGは埋め込まないため、
地形色は`TILE_COLORS`から生成したフォールバックになります。人物ポリゴンはWasmに
含まれるため通常ビルドと同じです。

## リポジトリ構成

```text
crates/
├── sim-math/       固定小数点、角度、三角関数、決定論的PRNG
├── sim-terrain/    地形グリッド、効果、導出値、固定地形の読込
├── sim-core/       ワールド、兵士、指揮、戦闘、騎兵、工兵、AI、シナリオ
├── sim-render/     人物モーション、LOD、ポリゴン生成
├── sim-wasm/       wasm-bindgen境界
└── sim-headless/   CLI、決定論・バランス・性能検証
web/
├── src/terrain/    地形生成、整形、道路、水系、シリアライズ
├── src/sim/        Worker、プロトコル、スナップショット、キャッシュ
├── src/render/     地形・人物・命令・ミニマップ描画
├── src/ui/         命令、詳細、セッション、会戦選択UI
└── tools/          Playwright疎通確認、単一HTML梱包
data/               TOMLの人間向け正本と固定地形
docs/spec/          目標仕様
tools/              生成・整合性検査
```

TOMLの実行時ローダーはまだなく、`data/`の値にはRust/TypeScript側の写しが存在します。
現在の二重管理と移行方針は[data/README.md](data/README.md)と
[TODO.md](TODO.md)に記載しています。

## 開発上の規約

仕様書の[非交渉事項](docs/spec/00-overview.md#5-設計上の非交渉事項)を優先します。

1. `sim-math` / `sim-core` / `sim-terrain`のシミュレーション計算に浮動小数点を
   入れない。描画用変換は`sim-core/src/snapshot.rs`へ閉じ込める。
2. 同じシードと命令列から同じ結果を出す。走査順が不定なコレクションや
   グローバルRNG状態を使わない。
3. 描画はシミュレーション状態を変更しない。
4. システムは前フェーズの確定値を読み、自分の出力へ書く。将来の並列化で
   単一スレッド版との状態ハッシュ一致を保てる構造にする。

## ライセンス

[MIT](LICENSE)
