# BUILD.md

このドキュメントは、`battle-sim`の**現在の**アーキテクチャとビルドパイプラインを
記す。目標アーキテクチャ（`sim-data`、SharedArrayBuffer、wasm threadsなど未実装の
要素を含む）は[docs/spec/01-architecture.md](docs/spec/01-architecture.md)を、
未実装事項・移行計画は[TODO.md](TODO.md)を参照。

コマンドを実行する前に、リポジトリ直下に`TOOLS.md`があれば必ず読むこと
（`AGENTS.md`参照）。このファイルはローカル専用でGit管理外。

## 1. 全体構成

```
┌───────────────── ブラウザ (メインスレッド) ─────────────────┐
│  UI (HTML/SVG) ── main.ts ── WebGL2レンダラ                  │
│        ▲                              ▲                      │
│        │ postMessage (JSON/構造化クローン)                    │
└────────┼──────────────────────────────┼──────────────────────┘
         │                              │
┌────────▼──────────────────────────────▼──────────────────────┐
│                        Web Worker (worker.ts)                 │
│   ┌───────────────────────────────────────────────────────┐  │
│   │              sim-wasm (wasm-bindgen境界)                │  │
│   │   World::tick() / push_order() / メモリビュー           │  │
│   └───────────────────────────┬───────────────────────────┘  │
│   ┌──────────────┐ ┌──────────┴─────────┐ ┌────────────────┐ │
│   │  sim-terrain │ │      sim-core       │ │   sim-math     │ │
│   │  地形生成    │ │  ECS/SoA + systems  │ │  固定小数点     │ │
│   └──────────────┘ └─────────────────────┘ └────────────────┘ │
└──────────────────────────────────────────────────────────────┘
```

`sim-render`（人物モーション・LOD・ポリゴン生成）は`sim-wasm`から呼ばれ、
Worker内でwasmとして実行される。地形の**生成**自体はTypeScript側
（`web/src/terrain/`）にもあり、Rust版と二重実装になっている
（詳細は6節）。

## 2. Cargo workspace

`Cargo.toml`がワークスペースルート。メンバーは6クレートで、依存は常に
下向き（循環なし）。

| クレート | 責務 | 依存 |
|---|---|---|
| `sim-math` | 固定小数点`Fx`、`Vec2Fx`、角度`Brad`、三角関数テーブル、`isqrt`、決定論的PRNG | なし |
| `sim-terrain` | 地形グリッドの保持・問い合わせ・導出値の計算、固定地形（fixture）の読込 | `sim-math` |
| `sim-core` | ワールド状態、兵士、指揮ツリー、AI、戦闘、騎兵、工兵、シナリオ、全システム | `sim-math`, `sim-terrain` |
| `sim-render` | 状態保持型の人物モーション、体格、騎乗、カリング、全LODのポリゴン生成 | なし |
| `sim-wasm` | `wasm-bindgen`境界。JSに公開するAPIとメモリビュー。ロジックを持たない | `sim-core`, `sim-math`, `sim-render`, `sim-terrain`, `wasm-bindgen` |
| `sim-headless` | CLI。シナリオ実行、決定論検証、ベンチ、バランス回帰、固定地形統計 | `sim-core`, `sim-math`, `sim-terrain` |

仕様上の目標にある`sim-data`（TOMLスキーマの検証・読込クレート）は
**未実装**。`data/*.toml`は人間が読む正本で、実行時にはRust/TypeScript側の
手書きの写しを使う（6節参照）。

### 2.1 非交渉事項（CIが機械的に検査する）

- `sim-math` / `sim-core` / `sim-terrain`に浮動小数点を混入させない。
  `f32`/`f64`が1つでも入ると同じシードで異なる結果になりうるため。
  描画用変換のみ`sim-core/src/snapshot.rs`に例外として許可。
  → `tools/check_no_float.sh`が正規表現で検査（コメント行は除外）。
- 同じシード・命令列から同じ結果（`state_hash()`）を出す。
  → CIの`determinism`ジョブが5,000体×5,000tickで検証。
- 描画はシミュレーション状態を変更しない。
- 各システムは前フェーズの確定値を読み、自分の出力へ書く
  （将来のマルチスレッド化に備えたダブルバッファ規約）。

### 2.2 ビルドプロファイル

`Cargo.toml`の`[profile.release]`は`opt-level = 3` / `lto = true` /
`codegen-units = 1` / `panic = "abort"`。wasm向けには`[profile.release-wasm]`
（`opt-level = "s"`を継承）があるが、実際に使われるのは`wasm-pack`が
`sim-wasm/Cargo.toml`の`[package.metadata.wasm-pack.profile.release]`で
`wasm-opt = false`に固定しているため、**`wasm-opt -Oz`はデフォルトでは
走らない**（wasm-packがGitHubからbinaryenを取得しようとしてネットワーク
制限環境で失敗するため）。サイズ最適化（目標: 800 KB未満）を効かせるには
別途binaryenを入れて手動で`wasm-opt -Oz`を実行する必要がある。

`.cargo/config.toml`は`wasm32-unknown-unknown`ターゲットに`simd128`を
`target-feature`として付与する。手書きSIMD命令は実測の上で見送り済み
（`sim-core`が`#![forbid(unsafe_code)]`のため）。LLVMの自動ベクトル化に
賭けているだけで、有効化自体はフラグのみ。

## 3. wasm ↔ JS 境界

境界を跨ぐ呼び出しはフレームあたり定数回に抑える設計。`sim-wasm`が公開する
主なAPI（`World`）:

- 生成: `new(scenario_json)`, `seed()`
- 実行: `tick()`, `tick_count()`, `state_hash()`
- 命令: `push_order(order_json)`, `cancel_order(order_id)`
- 描画用メモリビュー: `soldiers_ptr/len`, `terrain_surface_ptr`,
  `terrain_height_ptr`, `structures_ptr`（ポインタと長さのみ、コピーなし）
- 問い合わせ（低頻度）: `inspect_soldier`, `inspect_node`, `command_tree`, `pick`

JS側は`new Uint8Array(wasm.memory.buffer, ptr, len)`でビューを作る。wasm
メモリがgrowするとビューがdetachするため、毎tick`buffer`の同一性を確認して
必要ならビューを作り直す（`web/src/sim/snapshot.ts`）。

スレッドモデルは単一Worker・単一スレッド。`SharedArrayBuffer`によるゼロコピー
転送とwasm threadsは未実装で、現在は再利用する`ArrayBuffer`を
`postMessage`でtransferしてWorkerとメイン間をピンポンする。

## 4. Webフロントエンド構成

```
web/
├── index.html          エントリHTML
├── vite.config.ts       ビルド設定（Workerをclassicへ、wasmをdata URIへ畳み込む設定含む）
├── src/
│   ├── main.ts          エントリ。Worker起動とレンダループ
│   ├── sim/              worker.ts, protocol.ts, snapshot.ts, terrain-cache.ts, terrain-data.ts, detail.ts
│   ├── render/           gl.ts, iso.ts, terrain-gl.ts, terrain-tile.ts, soldiers.ts, minimap.ts, command-overlay.ts, generated-assets.ts
│   ├── terrain/          generate.ts, shaping.ts, hydrology.ts, roads.ts, battle-site.ts, scenarios.ts, serialize.ts, rng.ts, effects.ts, types.ts
│   ├── ui/               orders.ts, detail-panel.ts, scenario-panel.ts, session-panel.ts
│   ├── i18n.ts, quality.ts
│   └── wasm/             生成物（Git管理外）。wasm-pack build の出力先
└── tools/                Playwright疎通確認、単一HTML梱包（web/tools/*.mjs、7節）
```

地形の**生成**はTypeScript側（`web/src/terrain/`）にオリジナル実装があり、
Rust側（`sim-terrain`）には配布用に固定化した整数fixtureを読み込む経路だけ
がある（生成器自体の浮動小数点実装はRustへ移植されていない）。詳細は6節。

## 5. ローカルビルド手順

```bash
# 0. 環境固有の起動方法があれば TOOLS.md を優先する（このファイルには書けない）

# 1. Rustのネイティブテスト
cargo test --workspace

# 2. wasmを web/src/wasm/ へ生成
cd web
npm ci
npm run wasm
# 実体: wasm-pack build ../crates/sim-wasm --target web --out-dir ../../web/src/wasm --out-name sim

# 3. 開発サーバ
npm run dev
```

`web/src/wasm/`は生成物でGit管理外。`sim-wasm`の公開APIを変更したら、
`npm run wasm`を再実行してから型検査・ビルドを行うこと。

### 5.1 主要npmスクリプト（`web/package.json`）

| スクリプト | 内容 |
|---|---|
| `dev` | `vite`開発サーバ |
| `build` | `tsc --noEmit && vite build`（型検査してからバンドル） |
| `build:single` | `build`の後`tools/bundle-single-file.mjs`で単一HTML化 |
| `typecheck` | `tsc --noEmit`のみ |
| `wasm` | `wasm-pack build`で`sim-wasm`をビルドし`web/src/wasm/`へ出力 |
| `smoke` / `smoke:scenario` / `smoke:ui` / `smoke:performance` / `smoke:cache` / `smoke:single` | Playwrightによるブラウザ疎通確認（7節） |

### 5.2 ヘッドレスCLI（`sim-headless`）

ブラウザなしで決定論・バランス・性能・固定地形を検証する。主なサブコマンド:
`verify`（決定論）、`battle`/`winrate`（対称会戦）、`prep`（準備時間比較）、
`bench`（性能、`--metrics`でフェーズ別内訳）、`terrain`（固定地形統計）、
`regress`（戦場挙動の固定シード回帰）、`scenario`（プリセット一覧・実行）。
性能値は必ず`--release`で測る（devプロファイルの固定小数点演算は実運用の
指標にならない）。

## 6. データの二重管理

`data/`はTOMLの人間向け正本だが、**実行時ローダーはまだ存在しない**
（`sim-data`クレート未実装）。値を変更する場合は下表の実行時側コピーも
同じ変更で更新する必要があり、自動同期検査はない（地形効果テーブルを除く）。

| データ | 実行時の対応先 | 同期検査 |
|---|---|---|
| `formations.toml` | `crates/sim-core/src/organization.rs::formation_def` | なし |
| `factions/medieval_western.toml` | 指揮階層・兵科の参考定義のみ | なし |
| `scenarios/*.toml` | `crates/sim-core/src/scenario.rs`, `web/src/terrain/scenarios.ts` | なし（手動同期） |
| `terrain/*.bin` | `sim-terrain::fixture`（RustのテストとCLIが直接読む） | `node tools/gen_terrain_fixtures.mjs --check`（CI） |
| 地形効果（`web/src/terrain/effects.ts`が編集元） | `crates/sim-terrain/src/effects.rs`への写し | `node tools/check_terrain_effects.mjs`（CI） |

`terrain/*.bin`はTypeScriptの地形生成器（浮動小数点）の出力を固定化した
ものなので、生成器かシナリオ整形を変更したときだけ
`node tools/gen_terrain_fixtures.mjs`で再生成し、バイナリ差分をコミットする。

武器・防具・射撃・士気・騎兵・工兵のバランス値、兵士能力値分布、指揮官
アーキタイプとAI重み、陣形の実行時定義、会戦プリセットの陣容・指揮官・
障害物は、まだTOML化されておらずRustコード内に直書きされている。

## 7. tools/ と web/tools/ のスクリプト

リポジトリ直下`tools/`（生成・整合性検査、Rust/CI向け）:

| スクリプト | 役割 |
|---|---|
| `check_no_float.sh` | シミュレーション系クレートへの浮動小数点混入を検査（2.1節） |
| `check_terrain_effects.mjs` | 地形効果テーブルがJS/Rustで一致しているか検査 |
| `gen_terrain_fixtures.mjs` | `data/terrain/*.bin`固定地形を生成・検証（`--check`） |
| `gen_trig.mjs` | `crates/sim-math/src/trig_table.rs`の三角関数テーブルを再生成 |

`web/tools/`（Node、Playwright、Web向け）:

| スクリプト | 役割 |
|---|---|
| `bundle-single-file.mjs` | `web/dist/`を単一HTML（`battle-sim.html`）へ梱包。Worker（classic化済み）とwasm（data URI化済み）は`vite.config.ts`側で事前に畳み込まれているので、ここではJSをHTMLに埋め込むだけ。Workerをclassicにしているのは`file://`（origin `null`）からmodule workerを作るとブラウザに拒否されるため |
| `smoke.mjs` / `scenario-smoke.mjs` / `ui-smoke.mjs` / `performance-smoke.mjs` / `terrain-cache-smoke.mjs` / `single-file-smoke.mjs` | Playwrightスモークテスト本体 |

## 8. 単一HTML配布

```bash
cd web
npm run build:single   # dist/battle-sim.html を生成
npm run smoke:single   # file:// で疎通確認
```

wasmとclassic Workerを1ファイルへ埋め込むため`file://`から直接開ける。
地形アトラスPNGは埋め込まないので、地形色は`TILE_COLORS`由来の手続き
フォールバックになる。人物ポリゴンはwasm内で生成されるため通常ビルドと同じ。

## 9. CI（`.github/workflows/ci.yml`）

`push`（`main`）と全`pull_request`で実行。5ジョブ、依存関係は
`smoke`が`web`ジョブ完了を待つのみで他は並列。

1. **`rust`** — `cargo fmt --all --check` → `cargo clippy --workspace
   --all-targets -- -D warnings` → `./tools/check_no_float.sh` →
   `node tools/check_terrain_effects.mjs` → `npm --prefix web ci` +
   `node tools/gen_terrain_fixtures.mjs --check` → `cargo test --workspace`
   → `cargo check -p sim-wasm --target wasm32-unknown-unknown`
2. **`determinism`** — `sim-headless verify --soldiers 5000 --ticks 5000
   --size 1500`で状態ハッシュ一致を確認。`node tools/gen_trig.mjs`を実行後
   `git diff --exit-code`で三角関数テーブルが再生成しても変わらないことを検査
3. **`bench`** — `sim-headless bench --soldiers 20000 --ticks 500
   --size 2000`。閾値25 ms/tick（20 Hzの実時間予算50 msの半分）を超えたら
   失敗。GitHub Actionsランナーの性能揺れがあるため「明らかな回帰」だけを
   捕まえる閾値
4. **`web`** — `wasm-pack build`→`npm ci`→`npm run build`（型検査＋バンドル）
5. **`smoke`**（`web`完了後）— wasmビルド→`npm ci`→
   `npx playwright install --with-deps chromium`→`npm run build`→
   `vite preview`起動→`npm run smoke`/`smoke:scenario`→
   `bundle-single-file.mjs`→`smoke:single`（`file://`疎通）。
   `battle-sim.html`と`smoke-out/`のスクリーンショットをartifactとして
   常時（`if: always()`）アップロード

## 10. デプロイ（`.github/workflows/deploy-pages.yml`）

`main`へのpushまたは手動実行（`workflow_dispatch`）で、GitHub Pagesへ
自動デプロイする。`build`ジョブが`wasm-pack build`→`npm ci`→
`npm run build -- --base="<pages base path>/"`でVite出力を作り
（`actions/upload-pages-artifact@v3`でアップロード）、`deploy`ジョブが
`actions/deploy-pages@v4`で公開する。同時実行は`concurrency: group: pages,
cancel-in-progress: false`でキューイングされる。

## 11. 必要な環境

- Rust 1.75以上、`wasm32-unknown-unknown`ターゲット、`wasm-pack`
- Node.js 22以上とnpm
- WebGL2対応ブラウザ（スモークテストにはPlaywright + Chromium）

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-pack --locked
```

`mise.toml`はclangとrustの最新版を指定（SIMDビルドや将来のネイティブ拡張向け）。
