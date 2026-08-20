# 04. 軍の編成と指揮系統

## 1. 汎用 N 階層エンジン

編成は特定の時代に固定せず、**任意の深さのツリー**として扱う。中世の編成は
このエンジン上のデータとして定義する。

```rust
pub struct CommandNode {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,       // 空なら葉 = Unit
    pub echelon: u8,                 // 0 = 軍。下位ほど大きい
    pub faction: FactionId,

    // 指揮
    pub commander: SoldierId,        // 指揮官も戦場にいる一兵士
    pub deputies: Vec<SoldierId>,    // 継承順。指揮官が倒れたら繰り上がる
    pub command_state: CommandState, // Commanded / Leaderless / Succeeding

    // 意思
    pub objective: Objective,        // 自分が今やろうとしていること
    pub received_order: Option<Order>,  // 上位から届いた最新の命令
    pub pending_orders: Vec<InFlightOrder>, // 下位へ送信中（伝令が移動中）

    // 認識（fog of war の実体）
    pub blackboard: Blackboard,

    // 集計（毎 tick 更新、AI と UI が読む）
    pub stats: NodeStats,

    // 葉ノードのみ
    pub unit: Option<Unit>,
}

pub struct NodeStats {
    pub centroid: Vec2Fx,
    pub facing: Brad,
    pub frontage_m: Fx,
    pub alive: u32,
    pub downed: u32,
    pub dead: u32,
    pub broken: u32,           // 敗走中の人数
    pub avg_morale: u16,
    pub avg_fatigue: u16,
    pub cohesion: u16,         // 陣形の維持度 0..1000
    pub engaged_ratio: u16,    // 交戦中の割合
}
```

葉ノードの `Unit`:

```rust
pub struct Unit {
    pub soldiers: Vec<SoldierId>,   // 生成後は不変（死んでも残す）
    pub troop_type: TroopTypeId,    // 兵科
    pub formation: FormationId,
    pub formation_origin: Vec2Fx,   // 陣形の基準点
    pub formation_facing: Brad,
    pub ranks: u16,                 // 列数
    pub file_spacing: Fx,           // 横間隔
    pub rank_spacing: Fx,           // 縦間隔
    pub banner: Option<BannerId>,
    pub engagement: EngagementState,
}
```

### 1.1 指揮官も兵士である

`commander` は `SoldierId` であり、戦場に実体を持ち、移動し、戦い、死ぬ。
これは表現上の飾りではなく機能的に重要:

- 指揮官の**位置**が命令の伝達遅延を決める（伝令の走行距離）
- 指揮官の**視界**が `Blackboard` の精度を決める（高地にいれば見える）
- 指揮官の**存命**が指揮下の士気を左右し、死ねば指揮系統が切れる
- 指揮官が前線に出れば士気は上がるが、死ぬ確率も上がる。この**トレードオフを
  性格パラメータが判断する**（`boldness` が高い指揮官は前に出る）

### 1.2 指揮の継承

指揮官が `Downed` / `Dead` になると:

1. ノードは `Leaderless` になる。この間、上位からの命令は受理されず、
   `objective` は最後のものを継続（ただし新しい状況に適応できない）。
2. 指揮下の全兵士に士気ペナルティ（`commander_loss`）。近くにいた者ほど大きい。
3. `deputies` の先頭で生存している者が `Succeeding` に入る。
   継承には時間がかかる（既定 20〜60 秒、`Blackboard` の混乱度と距離に依存）。
4. 継承完了で `Commanded` に戻る。ただし新指揮官は自分の性格で判断するので、
   方針が変わりうる。
5. `deputies` が全滅した場合、上位ノードが直接指揮するか（自分の負荷が増える）、
   隣接ノードの指揮官が兼任する。どちらも上位ノードの `flexibility` で決まる。

## 2. 中世プリセット（14 世紀西欧封建軍）

`data/factions/medieval_western.toml` で定義。

```
Echelon 0: Army           総大将（王・大公）             全軍 3,000〜20,000
Echelon 1: Battle         バタイユ（前衛・主力・後衛）    1,000〜6,000
Echelon 2: Banner         バナー部隊（旗本）              100〜400
Echelon 3: Company        コンパニー（歩兵）100〜150
           Conroi         コンロワ（騎兵）20〜40 騎
Echelon 4: Vintaine       ヴァンテーヌ（20 人組）
           Lance          ランス（騎士 1 + 従士 2〜4）
Echelon 5: Soldier
```

| 階層 | 指揮官の呼称 | 典型人数 | 備考 |
|---|---|---|---|
| Army | Commander-in-Chief / 総大将 | — | 王、大公、あるいは元帥 |
| Battle | Marshal / Constable | 1,000〜6,000 | 前衛 (vanguard)・主力 (main)・後衛 (rearguard) の 3 列が標準 |
| Banner | Banneret（旗騎士） | 100〜400 | 旗を持てる階級。旗の喪失は大きな士気打撃 |
| Company | Centenar（百人隊長） | 100 | 歩兵の実務単位 |
| Conroi | Knight-Commander | 20〜40 騎 | 騎兵の突撃単位。一体となって突撃する |
| Vintaine | Vintenar（二十人長） | 20 | 歩兵の最小単位。顔見知りの集団 |
| Lance | 騎士本人 | 3〜5 | 騎士 + 従士 + 従者 |

### 2.1 兵科（`data/troop_types.toml`）

| 兵科 | 装備 | 陣形 | 特性 |
|---|---|---|---|
| `MenAtArms` | プレート/ブリガンダイン + ロングソード/ポールアーム + シールド | Line, Shieldwall | 中核。防御高、疲労大 |
| `Spearmen` | ギャンベゾン/メイル + 槍 + シールド | Shieldwall, Schiltron | 対騎兵。密集で真価 |
| `Pikemen` | ギャンベゾン + パイク(5m) | PikeSquare | 正面極強、側面極弱、転換が遅い |
| `Billmen` | メイル + ハルバード/ビル | Line | 対騎兵と対重装の中間 |
| `Longbowmen` | ギャンベゾン + 長弓 + ダガー + 杭 | Skirmish, Stakes | 射程 250m、自前で杭を打つ |
| `Crossbowmen` | メイル + 弩 + パヴィス（大盾） | Pavise Line | 貫通高、装填遅、パヴィスに隠れる |
| `LightInfantry` | 布 + 槍/斧 | Skirmish | 徴募兵。安いが脆い |
| `HeavyCavalry` | プレート + ランス + 剣、軍馬 | Wedge, Line | 突撃で決着をつける。停止すると価値半減 |
| `LightCavalry` | メイル + 槍/剣、軽馬 | Skirmish, Column | 偵察・側面・追撃 |
| `MountedArchers` | 布/メイル + 弓、馬 | Skirmish | 一撃離脱 |
| `Engineers` | 布 + 工具 + ダガー | Work | 07 章 |

### 2.2 データによる定義

エンジンは階層数も名称も知らない。ファクションデータが与える。

```toml
[[echelon]]
name = "Army"
commander_title = "総大将"
child_count = { min = 3, max = 5 }

[[echelon]]
name = "Battle"
commander_title = "元帥"
child_count = { min = 3, max = 8 }
roles = ["Vanguard", "Main", "Rearguard"]

[[echelon]]
name = "Banner"
commander_title = "旗騎士"
child_count = { min = 2, max = 4 }
has_banner = true

# ...

[[echelon]]
name = "Vintaine"
commander_title = "二十人長"
leaf = true
size = { min = 18, max = 24 }
```

古代ローマ（レギオ→コホルス→ケントゥリア→コントゥベルニウム）やナポレオン期
（軍団→師団→旅団→連隊→大隊→中隊）も同じ形式で書ける。エンジン側の変更は不要。

## 3. 命令の伝達

### 3.1 命令の構造

```rust
pub struct Order {
    pub id: OrderId,
    pub issued_tick: u32,
    pub issuer: NodeId,
    pub target: NodeId,
    pub intent: Intent,
    pub priority: Priority,       // Routine | Urgent | Absolute
    pub conditions: Vec<Condition>, // 「敵が橋を渡り始めたら」等のトリガ
    pub expires_tick: Option<u32>,
}

pub enum Intent {
    MoveTo { pos: Vec2Fx, facing: Brad, speed: MoveSpeed, formation: FormationId },
    Hold { pos: Vec2Fx, facing: Brad, allow_pursuit: bool },
    Attack { target: NodeId, approach: ApproachStyle },
    Charge { target: NodeId },
    Flank { target: NodeId, side: Side },
    Envelop { target: NodeId },
    Screen { protect: NodeId, side: Side },
    Reserve { rally_pos: Vec2Fx },     // 予備。投入命令を待つ
    Withdraw { to: Vec2Fx, fighting: bool },  // 戦闘退却か脱出か
    Pursue { target: NodeId, max_distance_m: u16 },
    HuntPerson { target: SoldierId }, // 指揮官など特定個人を追跡
    OccupyArea { center: Vec2Fx, radius_m: u16 },
    GuardArea { center: Vec2Fx, radius_m: u16, intercept_radius_m: u16 },
    SeizeTerrain { area: AreaId },     // 高地・橋・渡渉点
    Feint { target: NodeId, break_at_range_m: u16 },  // 偽装退却
    ShootAt { target: NodeId, mode: ShootMode },      // Volley | AtWill | Hold
    Engineer { task: EngineerTask },
}

pub enum MoveSpeed { Cautious, Walk, Quick, Run }
pub enum ApproachStyle { Deliberate, Aggressive, Cautious }
```

`OccupyArea` / `GuardArea` は地点へ全員を重ねず、到着後に安定スロットを円盤状へ
割り当てる。`GuardArea` の兵士は各持ち場を起点に `intercept_radius_m` 内の敵を
個別に迎撃し、離れすぎれば持ち場へ戻る。`HuntPerson` は対象の移動に合わせて
経路を更新するが、本人へ直接殺到する人数には上限を設け、残りは護衛や周辺の敵へ
対応する。

### 3.2 伝達の物理

命令は瞬時には届かない。3 つの経路がある。

| 経路 | 条件 | 遅延 | 情報量 |
|---|---|---|---|
| **伝令 (Messenger)** | 常に可能 | 距離 / 伝令速度 + 発令 5〜15 秒 + 受領 5〜15 秒 | 完全な `Intent` |
| **視覚信号（旗）** | 見通しがあり、距離 < 800 m、視界条件が良い | 3〜10 秒 | 限定語彙のみ |
| **音響信号（角笛・太鼓）** | 距離 < 400 m、戦闘の騒音で減衰 | 2〜5 秒 | 極めて限定的な語彙 |

**伝令エンティティ**は実際に戦場を走る。

```rust
pub struct Messenger {
    pub soldier: SoldierId,     // 実体。撃たれれば死ぬ
    pub order: Order,
    pub from: NodeId,
    pub to: NodeId,
    pub state: MessengerState,  // Riding | Delivering | Returning | Lost
}
```

- 騎馬伝令の速度 8〜10 m/s（地形の影響を受ける）。500 m 離れた部隊なら片道 50〜60 秒。
- 伝令は**敵に殺されうる**。射線を横切れば矢が当たり、敵騎兵に捕捉されれば失われる。
  命令が届かなければ、発令側は届いたと思い込んだまま戦況が進む。
- 発令側は一定時間で応答がなければ再送を検討する（`patience` パラメータ）。

**限定語彙**（旗・角笛で送れる信号）:
`Advance` / `Halt` / `Charge` / `Withdraw` / `Wheel Left` / `Wheel Right` /
`Volley` / `Cease Fire` / `Rally to Banner` / `General Retreat`

複雑な命令（「森の裏を回って側面に出よ」）は伝令でしか送れない。ここが
中世の指揮の限界であり、シミュレーションの面白さの源になる。

### 3.3 命令の解釈と歪み

命令は受け手の指揮官が**解釈**する。そのまま実行されない。

```
受領した Intent
  ↓
1. 遵守判定: obedience と priority と自分の Objective の衝突度から
             「従う / 部分的に従う / 無視する」を決める
  ↓
2. 解釈ノイズ: tactical_skill が低いと目標位置がずれる、
             タイミングを誤る、Approach スタイルを取り違える
  ↓
3. 現実との突合: 受領時点で状況が変わっていれば（目標がもういない、
             経路が塞がっている）、自分の判断で修正する
  ↓
4. 自分の Objective として採用、下位への命令に分解
```

**命令無視の例**（歴史的に頻出）:
- `ambition` の高い騎士が `Reserve` を無視して突撃する（`initiative` × `ambition` が
  一定を超え、かつ目の前に「名誉ある敵」がいるとき）
- `caution` の高い隊長が `Charge` を渋り、`Advance` 止まりで実行する
- `Withdraw` を受けた部隊が交戦中で離脱できず、`fighting withdrawal` になる

無視や歪みは必ず**イベントログに出す**。「なぜ左翼が突出したのか」を後から
追えなければ意味がない。

## 4. 陣形

```rust
pub struct FormationDef {
    pub id: FormationId,
    pub name: &'static str,
    pub file_spacing_mm: u16,     // 横間隔
    pub rank_spacing_mm: u16,     // 縦間隔
    pub default_ranks: u8,
    pub shape: FormationShape,    // Rect | Wedge | Circle | Loose | Line
    pub def_front: u16,           // 防御倍率（前面）1000 = 等倍
    pub def_flank: u16,
    pub def_rear: u16,
    pub move_mult: u16,           // 移動速度倍率
    pub turn_mult: u16,           // 方向転換の速さ
    pub cohesion_req: u16,        // 維持に必要な discipline
    pub anti_cavalry: u16,        // 対騎兵補正
    pub allow_shoot: bool,
}
```

| 陣形 | 間隔 | 列 | 前面防御 | 側面 | 移動 | 転換 | 対騎兵 | 特徴 |
|---|---|---|---|---|---|---|---|---|
| `Line` 横隊 | 0.8 m | 3〜4 | 1000 | 700 | 1000 | 1000 | 1000 | 標準 |
| `Shieldwall` 盾壁 | 0.5 m | 4〜6 | 1500 | 750 | 550 | 500 | 1300 | 密集。前進は遅い |
| `Column` 縦隊 | 0.9 m | 多 | 900 | 900 | 1200 | 1300 | 800 | 移動用。戦闘には不向き |
| `Wedge` 楔 | 1.2 m | — | 1300 | 800 | 1000 | 700 | — | 騎兵の突撃隊形。突破力 |
| `Schiltron` 円陣 | 0.5 m | 4 | 1200 | 1200 | 300 | 200 | 1800 | 全周対騎兵。動けない |
| `PikeSquare` | 0.6 m | 6〜8 | 1600 | 500 | 500 | 250 | 2000 | 正面最強、側面最弱 |
| `Skirmish` 散兵 | 2.5 m | 2 | 600 | 600 | 1150 | 1400 | 500 | 射撃と偵察。白兵に弱い |
| `PaviseLine` | 1.0 m | 2 | 1400(射撃) | 600 | 400 | 600 | 700 | 弩兵が大盾に隠れる |
| `Echelon` 梯形 | 0.8 m | 3 | 1000 | 850 | 950 | 900 | 1000 | 斜行。片翼を先行させる |

### 4.1 スロット割り当て

Unit の陣形は `formation_origin` と `formation_facing` から、各兵士の
**目標位置（スロット）**を決定論的に計算する。

```
files = ceil(slot_capacity / ranks)
slot(i) -> (file, rank)
local = ( (file - files/2) * file_spacing, rank * rank_spacing )
world = origin + rotate(local, formation_facing)
```

- スロット番号は生成時に固定し、死者を除いた全体再パックはしない。一人の死で
  それ以降の全員が一斉に別位置へ向かうのを防ぐためである。
- 前列に穴ができると、同じ file の直後にいる一人だけが一定間隔で前へ詰める。
  空いたスロットは後方へ一段ずつ伝わり、瞬時に列全体を圧縮しない。これが
  「厚みが持続力になる」と「個人が空きを見て進む」を両立させる。
- 将来はこの局所再割り当ての遅延・成功率へ `discipline` と周囲の混雑を反映する。
  規律の低い部隊は穴が埋まりにくく、そこから崩れる。
- 兵科ごとにスロットの優先順位がある（重装が前、弓が後ろ）。

### 4.2 陣形変更

陣形の変更は時間がかかり、その間**極めて脆弱**になる。

```
変更時間 = base_time × (人数 / 100) × (2000 - discipline平均) / 1000 × 地形の隊列維持係数
```

`Line` → `Schiltron` は 30〜90 秒。この間に騎兵に突かれれば崩壊する。
「騎兵が来る前に円陣を組めるか」という判断が、隊長 AI の重要な意思決定になる。

## 5. 部隊の生成

シナリオは編成ツリーを宣言的に書き、生成器が兵士を実体化する。

```toml
[[army]]
faction = "english"
commander = { archetype = "cautious_veteran", name = "Edward" }

  [[army.battle]]
  role = "Main"
  commander = { archetype = "stubborn_baron" }
    [[army.battle.unit]]
    troop = "MenAtArms"
    count = 800
    quality = "veteran"       # 能力値分布のプリセット
    formation = "Line"
    [[army.battle.unit]]
    troop = "Longbowmen"
    count = 1500
    quality = "professional"
    formation = "Stakes"
```

中間階層（Banner, Company, Vintaine）は `count` から自動生成される。
明示的に書くこともできる。

生成される兵士の能力値:

```
attrs.X = clamp(normal(quality.X.mean, quality.X.stddev), 0, 255)
```

`quality` のプリセット（`data/qualities.toml`）:

| 品質 | skill 平均 | bravery 平均 | discipline 平均 | 分散 | 例 |
|---|---|---|---|---|---|
| `levy` 徴募 | 60 | 70 | 55 | 大 | 農民徴募兵 |
| `militia` 民兵 | 90 | 95 | 85 | 大 | 都市民兵 |
| `professional` | 140 | 130 | 145 | 中 | 傭兵・熟練弓兵 |
| `veteran` 古参 | 175 | 165 | 170 | 小 | 歴戦の従士 |
| `elite` 精鋭 | 205 | 195 | 190 | 小 | 王の親衛隊・騎士 |
