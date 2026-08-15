# 02. シミュレーションコア

## 1. 時間

| 定数 | 値 | 根拠 |
|---|---|---|
| `TICK_HZ` | 20 | 1 tick = 50 ms |
| `TICK_MS` | 50 | |
| 最大移動量 / tick | 0.4 m | 全力疾走 8 m/s（騎兵）でも 0.4 m。兵士半径 0.35 m より小さいのでトンネリングしない |
| 人間の反応時間 | 200〜400 ms = 4〜8 tick | 個体差として `reflex` で表現 |

**なぜ 20 Hz か。** 人間の反応時間より細かく刻む必要はない。歩兵は 1〜1.5 m/s、
突撃騎兵でも 8 m/s 程度なので、50 ms あたりの移動は最大 40 cm。兵士の半径 35 cm より
小さいため、連続衝突判定なしで押し合いが破綻しない。一方これより粗くすると（10 Hz）
騎兵突撃が 80 cm 飛び、槍衾をすり抜ける。

**会戦の長さ。** sim 時間で 30 分〜4 時間。20 Hz なら 1 時間 = 72,000 tick。
実時間 1x で観る場合はそのまま 1 時間かかるので、通常は 4x〜16x で観戦し、
決定的局面で 1x に落とす運用を想定する。

## 2. 固定小数点

### 2.1 型

```rust
/// 位置・距離。1 単位 = 1/1024 m ≈ 0.977 mm
/// 表現範囲 ±2,097,152 m（5 km マップに対して桁あまり十分）
pub type Fx = i32;
pub const FX_ONE: Fx = 1024;
pub const FX_SHIFT: u32 = 10;

/// 角度。1 回転 = 65536。1 単位 ≈ 0.0055°
pub type Brad = u16;

/// 2D ベクトル
pub struct Vec2Fx { pub x: Fx, pub y: Fx }
```

**なぜ 1/1024 か。** 2 のべき乗なのでシフトで乗除できる。分解能 1 mm は
人間の体格（半径 35 cm）に対して十分細かく、速度（最小 1/1024 m/tick = 2 cm/s）も
表現できる。

### 2.2 演算規則

```rust
// 乗算は i64 経由で中間桁溢れを防ぐ
#[inline]
pub fn fx_mul(a: Fx, b: Fx) -> Fx {
    (((a as i64) * (b as i64)) >> FX_SHIFT) as Fx
}

// 除算も i64 経由
#[inline]
pub fn fx_div(a: Fx, b: Fx) -> Fx {
    (((a as i64) << FX_SHIFT) / (b as i64)) as Fx
}
```

**丸めは常に 0 方向切り捨て**（Rust の `>>` は算術シフトで負数は負の無限大方向に
丸まる点に注意し、必要なら明示的に補正する）。丸め方を場所によって変えない。

### 2.3 距離比較

平方根を避け、**二乗距離を i64 で比較**する。

```rust
#[inline]
pub fn dist_sq(a: Vec2Fx, b: Vec2Fx) -> i64 {
    let dx = (a.x - b.x) as i64;
    let dy = (a.y - b.y) as i64;
    dx * dx + dy * dy
}
```

実距離が必要な場面（押し戻しの正規化など）でのみ整数平方根を使う。

```rust
/// Newton 法による整数平方根。決定論的で、入力に対し一意な結果を返す
pub fn isqrt64(n: u64) -> u64;
```

### 2.4 三角関数

`Brad` を index とする事前計算テーブル。

```rust
/// 4096 エントリの sin テーブル（1/4 周期分）。値は Fx（1.0 = 1024）
static SIN_TABLE: [i16; 1024];

pub fn sin_fx(a: Brad) -> Fx;   // 対称性から全周期を導出
pub fn cos_fx(a: Brad) -> Fx;
pub fn atan2_fx(y: Fx, x: Fx) -> Brad;   // CORDIC または区分近似、テーブル参照
```

テーブルはビルド時に `build.rs` で生成し、ソースに埋め込む。実行時の浮動小数点計算は
発生しない。

### 2.5 禁止事項

**`sim-core`, `sim-math`, `sim-terrain` に `f32` / `f64` を一切入れない。**
CI で以下を検査する。

```bash
# シミュレーション系クレートに浮動小数点型が現れたら失敗
! grep -rn --include='*.rs' -E '\bf32\b|\bf64\b' crates/sim-{math,core,terrain}/src
```

例外は「描画用スナップショットの書き出し」のみで、これは `sim-wasm` 側に置く。

## 3. 乱数

### 3.1 方針

グローバルな RNG 状態を持たない。**「誰が」「何の目的で」「いつ」引くかから決定的に
導出**する。これによりシステムの実行順序やスレッド数が変わっても結果が一致する。

```rust
pub struct Rng(u64);

impl Rng {
    /// エンティティ・目的・ティックから独立ストリームを導出
    #[inline]
    pub fn stream(world_seed: u64, entity: u32, purpose: Purpose, tick: u32) -> Rng {
        let mut h = world_seed;
        h = splitmix64(h ^ (entity as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        h = splitmix64(h ^ (purpose as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9));
        h = splitmix64(h ^ (tick as u64).wrapping_mul(0x94D0_49BB_1331_11EB));
        Rng(h)
    }

    pub fn next_u32(&mut self) -> u32;
    pub fn range(&mut self, lo: i32, hi: i32) -> i32;      // [lo, hi)
    pub fn chance(&mut self, numerator: u32, denom: u32) -> bool;
    pub fn normal_fx(&mut self, mean: Fx, stddev: Fx) -> Fx;  // Irwin–Hall 近似
}

#[repr(u16)]
pub enum Purpose {
    HitRoll, DamageRoll, MoraleCheck, TargetSelect, PathJitter,
    ArrowSpread, HorseRefusal, RallyCheck, DecisionNoise, Spawn, /* ... */
}
```

### 3.2 生成時の分散

兵士の能力値は生成時に一度だけ引く。年齢・訓練度・出身によって平均と分散が変わる
正規分布からサンプルし、以降は固定。

## 4. エンティティとデータレイアウト

### 4.1 SoA（Structure of Arrays）

50,000 体を毎 tick 触るので、キャッシュ効率がそのまま性能になる。
**ホット（毎 tick 読む）とコールド（稀にしか読まない）を別配列に分ける。**

```rust
pub struct Soldiers {
    // ── ホット: 移動・衝突で毎 tick 触る ─────────────────
    pub pos_x:    Vec<Fx>,      // 4 B
    pub pos_y:    Vec<Fx>,      // 4 B
    pub vel_x:    Vec<i16>,     // 2 B  Fx の下位のみ（1 tick の移動は小さい）
    pub vel_y:    Vec<i16>,     // 2 B
    pub facing:   Vec<Brad>,    // 2 B
    pub state:    Vec<State>,   // 1 B
    pub flags:    Vec<u8>,      // 1 B  mounted / downed / broken / engaged ...
                                //      計 16 B —— キャッシュライン 4 体分

    // ── ウォーム: 思考で読む ────────────────────────────
    pub hp:       Vec<u16>,
    pub fatigue:  Vec<u16>,     // 0..10000
    pub morale:   Vec<u16>,     // 0..1000
    pub unit_id:  Vec<u16>,
    pub target:   Vec<u32>,     // 交戦相手の soldier id（なければ NONE）
    pub slot:     Vec<u16>,     // 陣形内スロット番号
    pub think_at: Vec<u32>,     // 次に思考するティック
    pub z:        Vec<i16>,     // 高度 cm（地形から導出、キャッシュ）

    // ── コールド: 生成時に決まり、ほぼ変わらない ─────────
    pub attrs:    Vec<Attrs>,   // 性格・運動能力 16 B
    pub equip:    Vec<EquipId>, // 装備セット参照
    pub archetype:Vec<u16>,
    pub horse:    Vec<u32>,     // 騎乗中の馬 id（なければ NONE）
    pub name_seed:Vec<u32>,     // 名前生成用（UI 表示のみ）

    // ── 弾薬・工兵など、持つ者だけの疎データ ────────────
    pub ammo:     HashMap<u32, u16>,
}
```

エンティティ ID は配列の index。削除はしない（死体も `Downed` / `Dead` 状態として
残り、地形の障害物になり士気に影響するため）。生成順は決定論的。

### 4.2 状態機械

```rust
#[repr(u8)]
pub enum State {
    Idle,        // 待機
    Marching,    // 隊列を組んで移動
    Repositioning, // 陣形スロットへの復帰
    Advancing,   // 敵に向けて前進
    Charging,    // 突撃（速度と衝撃を蓄積）
    Engaged,     // 白兵戦中
    Shooting,    // 射撃中
    Reloading,   // 装填中
    Wavering,    // 動揺（命令に従うが逃走判定が近い）
    Broken,      // 敗走中
    Rallying,    // 再結集中
    Working,     // 工兵作業中
    Downed,      // 戦闘不能（回収されうる）
    Dead,
}
```

### 4.3 能力・性格パラメータ

```rust
/// 16 バイト。各値は 0..=255 で、実効値はデータ定義のスケールで解釈する
#[repr(C)]
pub struct Attrs {
    // 運動能力
    pub speed:      u8,  // 最大速度
    pub accel:      u8,  // 加速力・方向転換
    pub endurance:  u8,  // 疲労耐性
    pub strength:   u8,  // 打撃力・押し合いでの質量寄与
    pub reflex:     u8,  // 反応時間（思考の遅延）
    pub skill:      u8,  // 武技（命中・受け）

    // 性格
    pub bravery:          u8,  // 士気の初期値と回復
    pub discipline:       u8,  // 隊列維持・命令遵守
    pub aggression:       u8,  // 追撃・深追い
    pub self_preservation:u8,  // 危機回避性向（危険な位置から逃げる強さ）
    pub loyalty:          u8,  // 仲間を助けに行くか
    pub composure:        u8,  // パニック伝播への耐性

    pub _pad: [u8; 4],
}
```

指揮官はこれに加えて `CommanderAttrs` を別配列で持つ（05 章参照）。

### 4.4 その他のエンティティ

| 種類 | 保持形式 | 備考 |
|---|---|---|
| `Horse` | SoA（数千体） | 体力・疲労・恐怖。騎手と独立に判定する |
| `Projectile` | SoA（数千体、リングバッファ） | 矢・弩矢・投石。放物線を固定小数点で積分 |
| `Structure` | Vec（数百） | 杭列・堀・橋・土塁・攻城兵器。地形グリッドにも書き込む |
| `Banner` | Vec（数百） | 旗。位置と保持者。喪失が士気に大きく効く |
| `Corpse` | 兵士の `Dead` 状態で表現 | 別配列にしない |
| `CommandNode` | Vec（数百） | 指揮ツリー |

## 5. ティックの実行順序

順序は固定。各システムは「前フェーズの確定値を読み、自分の出力配列に書く」。
同一フェーズ内で他エンティティの更新後の値を読まない。

```
tick(t):
  0. 命令キューの処理       届いた命令を CommandNode に適用
  1. 空間ハッシュ再構築     カウントソートで O(n)
  2. 知覚                   各兵士の近傍リスト・脅威評価を更新（LOD 頻度で間引き）
  3. 指揮官 AI              期限が来たノードのみ。目的の再評価と命令の発行
  4. Unit AI                葉ノード。陣形スロットの再計算・経路・交戦判断
  5. 兵士 AI                期限が来た兵士のみ。行動の決定と操舵ベクトル
  6. 射撃・装填             発射判定、Projectile の生成
  7. 弾道更新               Projectile の積分と着弾判定
  8. 白兵戦解決             engagement の攻撃サイクル、ダメージ適用
  9. 移動積分               操舵 → 速度 → 位置（地形の速度修正込み）
 10. 衝突解決               分離の反復（2 回）、押し合い、圧迫判定
 11. 高度追従               地形高度を位置から引いて z を更新
 12. 士気                   個人士気の増減、近傍伝播、閾値判定、敗走・再結集
 13. 疲労・出血             継続効果
 14. 工兵作業               建設進捗、地形グリッドへの書き込み
 15. 伝令                   伝令エンティティの移動、到着した命令のキュー投入
 16. 集計                   Unit・ノードごとの統計（死傷・平均士気・重心）
 17. イベント発行           UI 向けログ（指揮官の決断、部隊の崩壊など）
```

**8 → 9 → 10 の順序が重要。** ダメージを与えてから動かし、最後に重なりを解消する。
これにより「押されて後退する」「崩れた列に隙間ができる」が自然に出る。

## 6. 決定論の担保

### 6.1 状態ハッシュ

毎 tick、あるいは N tick ごとにワールド全体のハッシュを取れるようにする。

```rust
impl World {
    pub fn state_hash(&self) -> u64 {
        // 兵士の pos/vel/hp/morale/state、Projectile、Structure、
        // CommandNode の目的と命令キューを固定順で FNV-1a に流し込む
    }
}
```

### 6.2 CI での検証

- 同一シードで 10,000 tick を 2 回走らせ、全 tick のハッシュ列が一致すること。
- ネイティブ（x86_64）と wasm でハッシュ列が一致すること。整数演算のみなので
  一致するはずで、しなければバグ。
- リプレイ（シード + 命令ログ）から再生した結果が元の実行と一致すること。

### 6.3 陥りやすい罠と対策

| 罠 | 対策 |
|---|---|
| `HashMap` のイテレーション順 | シミュレーション状態の走査に `HashMap` を使わない。必要なら `BTreeMap` か、ID ソート済み `Vec` |
| 浮動小数点の混入 | CI の grep で機械的に禁止（本章 2.5） |
| 並列化での順序依存 | 読み書きフェーズ分離 + エンティティ由来の RNG ストリーム |
| `sort_unstable` の不安定性 | 比較キーに必ず ID を含め、全順序にする |
| 時刻・乱数の外部取得 | `sim-core` から `std::time` と `rand` を依存から外す |
| wasm メモリ grow での挙動差 | 状態に影響しない。ただし JS 側のビュー再取得は必要 |

## 7. スナップショットと巻き戻し

- **リプレイ**は「シード + シナリオ + 命令ログ」のみで完全再現できる（数 KB）。
  共有・保存の既定形式はこれ。
- **巻き戻し**は再生の高速化のため、N tick（既定 6,000 = 5 分）ごとに完全な状態
  スナップショットをメモリに保持する。巻き戻し先の直前スナップショットから
  早送りで到達する。50,000 体で 1 スナップショット約 6 MB、10 個保持で 60 MB。
- スナップショットは可逆圧縮（位置の差分 + LZ）で 1/3 程度に落とせる想定。
