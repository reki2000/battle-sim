# 01. システムアーキテクチャ

> この文書は目標アーキテクチャを示す。`sim-data`、SharedArrayBuffer、wasm threads
> など未実装の要素も含む。現在の構成は[README](../../README.md)、移行計画は
> [TODO](../../TODO.md)を参照すること。

## 1. 全体構成

```
┌─────────────────────── ブラウザ (メインスレッド) ────────────────────────┐
│                                                                          │
│  ┌────────────┐   ┌─────────────────┐   ┌──────────────────────────┐   │
│  │  UI (HTML) │   │ SVG オーバーレイ │   │  WebGL2 レンダラ          │   │
│  │  UI        │   │ 部隊枠・命令矢印 │   │ 地形 + 人物ポリゴン      │   │
│  └─────┬──────┘   └────────┬────────┘   └────────────┬─────────────┘   │
│        │                   │                          │                 │
│        └───────────────────┴──────────┬───────────────┘                 │
│                                       │ 読み取り専用                     │
│                          ┌────────────▼─────────────┐                   │
│                          │  RenderSnapshot (SoA)    │                   │
│                          │  SharedArrayBuffer or    │                   │
│                          │  transferable double buf │                   │
│                          └────────────▲─────────────┘                   │
└───────────────────────────────────────┼─────────────────────────────────┘
                             命令 (postMessage)  │ スナップショット
┌───────────────────────────────────────┼─────────────────────────────────┐
│                    Web Worker         │                                 │
│  ┌────────────────────────────────────┴──────────────────────────────┐  │
│  │                      sim-wasm (wasm-bindgen)                      │  │
│  │  World::tick() / push_order() / snapshot() / memory views         │  │
│  └────────────────────────────────┬──────────────────────────────────┘  │
│  ┌──────────────┐ ┌───────────────┴──────┐ ┌──────────────┐ ┌────────┐  │
│  │  sim-terrain │ │       sim-core       │ │  sim-data    │ │sim-math│  │
│  │  地形生成    │ │  ECS/SoA + systems   │ │  TOML ロード │ │ 固定小数│  │
│  └──────────────┘ └──────────────────────┘ └──────────────┘ └────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
```

## 2. クレート構成

Cargo workspace。依存の向きは常に下向き（循環なし）。

| クレート | 責務 | 依存 | 備考 |
|---|---|---|---|
| `sim-math` | 固定小数点 `Fx`、`Vec2Fx`、角度 `Brad`、三角関数テーブル、`isqrt`、PRNG | なし | `#![no_std]` 互換。浮動小数点を一切使わない |
| `sim-data` | TOML スキーマ定義とロード。装備・陣形・アーキタイプ・シナリオ | `serde` | ビルド時に検証、実行時は事前パース済みバイナリでも可 |
| `sim-terrain` | 地形生成パイプライン。高度・浸食・水系・植生・崖・道路 | `sim-math` | 生成は決定論的。結果は `TerrainGrids` |
| `sim-core` | ワールド状態、エンティティ、全システム、指揮ツリー、AI | `sim-math`, `sim-data`, `sim-terrain` | ここに全ロジックが入る。wasm 非依存で、ネイティブでもテスト可能 |
| `sim-render` | 状態保持型の人物モーション、体格、騎乗、カリング、全LODのポリゴン生成 | なし | Rustのみ。シミュレーション位置は変更しない |
| `sim-wasm` | `wasm-bindgen` 境界。JS に公開する API とメモリビュー | `sim-core`, `sim-render` | 薄いラッパのみ。ロジックを置かない |
| `sim-headless` | CLI。シナリオを走らせて統計を出す。ベンチとリグレッションテスト用 | `sim-core` | CI がこれで性能とバランスを回帰チェックする |

**`sim-core` を wasm 非依存に保つ**のが要。ブラウザなしで数千回のバッチ実行ができるため、
バランス調整も性能計測も回帰テストも、ネイティブの速度で回せる。

## 3. スレッドモデル

### 3.1 基本形

- **メインスレッド**: UI・入力・描画のみ。シミュレーションを絶対に実行しない。
- **Sim Worker**: `sim-wasm` をロードし、固定 20 Hz でティックを回す。

Worker は自分のペースで `tick()` を呼び、実時間との差分を蓄積して
可変ステップ数を消化する（accumulator パターン）。ただし 1 フレームで消化する
ティック数には上限を設け（既定 8）、スパイラルを防ぐ。倍速時はこの上限を引き上げる。

```
loop {
    accumulator += elapsed_real_ms * speed_multiplier;
    let mut n = 0;
    while accumulator >= TICK_MS && n < max_ticks_per_frame {
        world.tick();
        accumulator -= TICK_MS;
        n += 1;
    }
    world.write_snapshot(back_buffer);
    publish(back_buffer);
}
```

### 3.2 スナップショットの受け渡し

描画に必要なのは全状態のごく一部なので、専用の SoA スナップショットを吐く。

```rust
// 兵士 1 体あたり 20 バイト
struct RenderSoldier {
    x: f32,        // メートル（描画専用に固定小数点から変換）
    y: f32,
    z_cm: i16,
    facing: u16,
    unit_id: u16,
    troop_type: u16,
    state: u8,
    flags: u8,
    faction: u8,
    padding: u8,
}
```

50,000 体で 1 MB / スナップショット。

- **SharedArrayBuffer が使える場合**（COOP/COEP ヘッダを配信できる場合）:
  ダブルバッファをリング状に持ち、`Atomics` で「今読んでよいバッファの index」だけを
  やり取りする。コピーゼロ。
- **使えない場合**: `postMessage` で `ArrayBuffer` を transfer する。バッファは
  2 枚を Worker とメイン間でピンポンする（毎回の確保を避ける）。30 fps 更新で 24 MB/s、
  実測上問題ない範囲。

描画は60 fps、スナップショットは20 Hz（sim tickと同期）なので、`sim-render`は
2つの連続スナップショット間を位置と向きについて補間する。モーション位相と
クロスフェードも描画エンジン内で完結し、シミュレーションには一切戻さない。

### 3.3 将来のマルチスレッド化

wasm threads（`SharedArrayBuffer` + `Atomics` + `wasm-bindgen-rayon`）は
M9 で検討する。そのために今から守る規約:

- 各システムを **読み取りフェーズ → 書き込みフェーズ** に分ける（ダブルバッファ）。
  同一ティック内で他のエンティティの「更新後の値」を読まない。
- 乱数はエンティティ ID から導出したストリームを使い、グローバルな RNG 状態を持たない。
- 空間ハッシュはカウントソートで再構築するので並列化が容易。

これにより「単一スレッドと複数スレッドで結果が一致する」ことを保てる。

## 4. wasm ↔ JS 境界

境界を跨ぐ呼び出しは**フレームあたり定数回**に抑える。エンティティごとの呼び出しは禁止。

### 4.1 公開 API（`sim-wasm`）

```rust
#[wasm_bindgen]
impl World {
    // --- 生成 ---
    pub fn new(scenario_json: &str) -> World;
    pub fn seed(&self) -> u64;

    // --- 実行 ---
    pub fn tick(&mut self);
    pub fn tick_count(&self) -> u32;
    pub fn state_hash(&self) -> u64;      // 決定論の検証用

    // --- 命令 ---
    pub fn push_order(&mut self, order_json: &str) -> u32;  // 命令 ID を返す
    pub fn cancel_order(&mut self, order_id: u32);

    // --- 描画用メモリビュー（ポインタと長さのみ、コピーなし） ---
    pub fn soldiers_ptr(&self) -> *const u8;
    pub fn soldiers_len(&self) -> u32;
    pub fn terrain_surface_ptr(&self) -> *const u8;
    pub fn terrain_height_ptr(&self) -> *const u8;
    pub fn structures_ptr(&self) -> *const u8;

    // --- 問い合わせ（UI 用、低頻度） ---
    pub fn inspect_soldier(&self, id: u32) -> JsValue;   // 全パラメータ + 現在の思考
    pub fn inspect_node(&self, id: u32) -> JsValue;      // 指揮官の目的・命令・戦況認識
    pub fn command_tree(&self) -> JsValue;               // 指揮ツリー全体（変化時のみ）
    pub fn pick(&self, x: f32, y: f32, radius: f32) -> Uint32Array;  // 矩形/円内の兵士 ID
}

#[wasm_bindgen]
impl ArmyRenderer {
    pub fn apply_snapshot(&mut self, bytes: &[u8], stride: u32, tick: u32);
    pub fn set_camera(&mut self, center_x: f32, center_y: f32, px_per_m: f32,
                      width: f32, height: f32, world_size: f32);
    pub fn step(&mut self, dt: f32, alpha: f32, lod: u8);
    pub fn vertices_ptr(&self) -> *const f32;
    pub fn vertices_float_len(&self) -> u32;
}
```

JS 側は `new Uint8Array(wasm.memory.buffer, ptr, len)` でビューを作る。
wasm のメモリが grow するとビューが detach するので、**ティックごとに `buffer` の
同一性を確認し、変わっていたらビューを作り直す**。

### 4.2 JSON を使う場所・使わない場所

| 用途 | 形式 | 頻度 |
|---|---|---|
| シナリオ投入 | JSON 文字列 | 起動時 1 回 |
| 命令の投入 | JSON 文字列 | ユーザー操作時のみ（秒に数回） |
| 兵士・ノードの詳細 | `JsValue`（serde-wasm-bindgen） | UI 選択時のみ |
| 描画データ | 生バイト（メモリビュー） | 毎フレーム |
| ピック結果 | `Uint32Array` | クリック時のみ |

## 5. Web フロントエンド構成

```
web/
├── index.html
├── vite.config.ts
├── src/
│   ├── main.ts              エントリ。Worker 起動とレンダループ
│   ├── sim/
│   │   ├── worker.ts        Sim Worker 本体（wasm をロードして tick を回す）
│   │   ├── bridge.ts        メイン側から Worker を叩く型付きラッパ
│   │   └── snapshot.ts      SoA ビューの読み取りと補間
│   ├── render/
│   │   ├── iso.ts           クォータービューの座標変換とカメラ
│   │   ├── gl.ts            WebGL2 コンテキスト・シェーダ管理
│   │   ├── terrain.ts       タイルマップのチャンク描画
│   │   ├── soldiers.ts      Wasm生成ポリゴンをWebGL2へ転送
│   │   ├── effects.ts       矢の軌跡・血・砂塵
│   │   └── lod.ts           ズームレベルと LOD の決定
│   ├── ui/
│   │   ├── overlay.ts       SVG オーバーレイ（部隊枠・命令矢印・前線）
│   │   ├── inspector.ts     兵士・指揮官の詳細パネル
│   │   ├── orders.ts        命令入力の UI
│   │   ├── timeline.ts      時間制御とリプレイスクラブ
│   │   └── minimap.ts       ミニマップ
└── tools/
    └── *.mjs                スモークテスト・単一HTML梱包
```

## 6. ビルドパイプライン

```
1. cargo test --workspace                    ネイティブでロジックテスト
2. wasm-pack build crates/sim-wasm           wasm + JS グルーを web/src/wasm/ に出力
   --target web --out-dir ../../web/src/wasm
3. npm run build (Vite)                      バンドル
```

wasm はリリースビルドで `opt-level = "s"` + `lto = true` + `wasm-opt -Oz`。
目標サイズは 800 KB 未満（gzip 後 300 KB 前後）。

## 7. テスト戦略

| 層 | 手段 |
|---|---|
| 固定小数点数学 | 単体テスト。既知の値と、`isqrt`/三角関数の誤差上限 |
| 決定論 | 同一シードで 10,000 tick 走らせ `state_hash()` が一致するか。CI で毎回 |
| 地形生成 | シードごとのスナップショットハッシュ。生成物の統計的性質（すべての排水が海または地図外へ到達するか、通行不能領域が孤立していないか） |
| 戦闘バランス | `sim-headless` でシナリオを 100 回走らせ、勝率・死傷率・会戦時間の分布を検証。史実の会戦を模したシナリオで妥当な範囲に入るか |
| 性能 | `sim-headless` のベンチ。50,000 体の 1 tick あたり所要時間を CI で追跡し、回帰したら失敗 |
| 描画 | Playwright でスクリーンショット比較（LOD 各段階） |
