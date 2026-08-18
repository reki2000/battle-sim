# 10. データ形式

すべての数値は `data/` 以下の TOML に置く。バランス調整に再コンパイルを要求しない
（00 章 非交渉事項 5）。

## 1. ファイル構成

```
data/
├── weapons.toml           武器
├── armor.toml             防具
├── shields.toml           盾
├── troop_types.toml       兵科（装備セット + 既定陣形 + AI 傾向）
├── formations.toml        陣形
├── qualities.toml         兵の品質（能力値の分布）
├── archetypes.toml        性格アーキタイプ
├── ai_weights.toml        AI の評価関数の係数
├── morale.toml            士気の増減量
├── terrain/               会戦ごとの固定地形（生成器が書き出したバイナリ）
├── engineer_tasks.toml    工兵タスクの工数と効果
├── sprite_sets.toml       スプライト生成の定義
├── visual-profiles/       時代・地域・兵科・行動の描画契約（シミュレーター正本）
├── factions/
│   ├── english_1415.toml
│   ├── french_1415.toml
│   └── generic_medieval.toml
└── scenarios/
    ├── open_field.toml
    ├── river_crossing.toml
    └── hill_defense.toml
```

ビルド時に検証し（未定義の参照・範囲外の値を検出）、
実行時にはバイナリ化した形式（`bincode`）を wasm に埋め込む。
開発時は TOML を直接読める `dev` フィーチャを用意する。

## 2. 武器

```toml
[[weapon]]
id = "longsword"
name_ja = "ロングソード"
name_en = "Longsword"
reach_mm = 1200
weight_g = 1500
base_swing_ms = 1400
accuracy = 20                  # 命中ロールへの加算
power = 85
damage_type = "Cut"            # Cut | Pierce | Blunt
secondary_type = "Pierce"      # 刺突にも切り替えられる
secondary_ratio = 400          # 1000 分率で、AI が刺突を選ぶ割合
two_handed = true
hands = 2
can_shield = false
cavalry_ok = false

[[weapon]]
id = "spear"
name_ja = "槍"
reach_mm = 2200
weight_g = 2000
base_swing_ms = 1100
accuracy = 10
power = 70
damage_type = "Pierce"
two_handed = false
hands = 1
can_shield = true
anti_cavalry = 1600            # 対騎兵倍率
brace_bonus = 900              # 構えて待ち受けたときの加算（突撃を受け止める）

[[weapon]]
id = "longbow"
name_ja = "長弓"
kind = "Missile"
range_m = 250
effective_range_m = 180
reload_ms = 6000
power = 55
pen_base = 60                  # 装甲貫通の基準値
missile = "arrow"
ammo_capacity = 72
draw_fatigue = 12              # 1 射あたりの疲労
min_strength = 140             # 引くのに必要な膂力。足りないと射程低下
arc = "High"                   # High = 曲射（味方頭越し可）| Flat = 直射
```

## 3. 防具

部位ごとにカバー率と防護値を持つ。

```toml
[[armor]]
id = "plate_harness"
name_ja = "プレートアーマー"
class = "Plate"                # Cloth | Mail | Plate
weight_g = 25000
# 部位ごとの [カバー率(1000分率), 防護値]
head  = [1000, 190]
torso = [1000, 200]
arms  = [ 900, 170]
legs  = [ 850, 160]
mobility_mult = 900            # 移動速度倍率
fatigue_mult = 1600            # 疲労消費倍率
mud_penalty = 1900             # 泥での追加ペナルティ

[[armor]]
id = "mail_hauberk"
name_ja = "チェインメイル（ホーバーク）"
class = "Mail"
weight_g = 11000
head  = [   0,   0]            # 別途ヘルムが必要
torso = [1000, 120]
arms  = [ 800, 100]
legs  = [ 500,  90]
mobility_mult = 960
fatigue_mult = 1250
mud_penalty = 1350

[[armor]]
id = "gambeson"
name_ja = "ギャンベゾン"
class = "Cloth"
weight_g = 4000
head  = [   0,   0]
torso = [1000,  45]
arms  = [ 900,  40]
legs  = [ 400,  30]
mobility_mult = 1000
fatigue_mult = 1050
mud_penalty = 1100
```

ダメージタイプと装甲クラスの相性表:

```toml
[damage_matrix]
# [Cloth, Mail, Plate] への倍率（1000 = 等倍）
Cut    = [1000, 350, 100]
Pierce = [ 900, 650, 300]
Blunt  = [ 700, 850, 700]
```

## 4. 兵科

```toml
[[troop_type]]
id = "longbowmen"
name_ja = "長弓兵"
weapons = ["longbow", "dagger"]
armor = "gambeson"
helmet = "kettle_hat"
shield = "none"
mounted = false
carries_stakes = true
default_formation = "skirmish"
alt_formations = ["stakes_line", "line"]
ammo = { arrow = 72 }
# AI の傾向（この兵科の兵士の性格に加算される偏り）
attr_bias = { aggression = -20, discipline = +10 }
# 隊長 AI の傾向
tactics = { prefer_range = true, avoid_melee = true, self_fortify = true }
sprite_set = "longbowman"

[[troop_type]]
id = "heavy_cavalry"
name_ja = "重騎兵"
weapons = ["lance", "arming_sword"]
armor = "plate_harness"
helmet = "great_helm"
shield = "heater"
mounted = true
horse = "destrier"
default_formation = "wedge"
alt_formations = ["line", "column"]
attr_bias = { aggression = +35, bravery = +25, discipline = -15 }
tactics = { charge_start_m = 200, prefer_flank = true, regroup_after_charge = true }
sprite_set = "knight_mounted"
```

### 4.1 人物画像の責務境界

`data/visual-profiles/<id>.toml` はシミュレーターが所有し、次だけを定義する。

- 時代・地域
- 必要な兵科ロール、説明、安定した `troop_type_index`
- ロールごとに必要な画像行動
- `State` 全値から画像行動への対応
- 8行の再生方式（通常行動 `cycle: 1x8`、待機 `variant-loop: 4x2`、
  死亡 `static-variants: 8x1`）

服装、染色、武器寸法、防具構造、持ち手、装具の左右などはシミュレーションの
判断ロジックではないため、このファイルへ記述しない。スプライト生成スキルが
時代・地域・ロール説明に沿ってWeb上の史料を調査し、出典付き `research.json` と
ImageGen用 `role.json` を `art/sprites/sets/` に作る。

スキルは同時に `web/public/assets/sprites/v2/profiles/<id>.json` を生成する。描画側は
スナップショットの `troop_type` と `state`、この実行時プロファイル、v2 manifestを
結合してシートを選ぶ。画像生成側から兵科や状態の意味を追加してはならない。

## 5. 陣形

```toml
[[formation]]
id = "schiltron"
name_ja = "シルトロン（円陣）"
shape = "Circle"
file_spacing_mm = 500
rank_spacing_mm = 700
default_ranks = 4
def_front = 1200
def_flank = 1200
def_rear  = 1200
move_mult = 300
turn_mult = 200
cohesion_req = 150             # 必要な平均 discipline
anti_cavalry = 1800
allow_shoot = false
change_time_base_s = 60        # 100 人あたりの変更所要時間
requires = { weapon_reach_mm = 2000 }   # 長柄が必要
```

## 6. 兵の品質

```toml
[[quality]]
id = "veteran"
name_ja = "古参"
# [平均, 標準偏差]
skill     = [175, 22]
bravery   = [165, 25]
discipline= [170, 20]
speed     = [130, 20]
accel     = [130, 20]
endurance = [155, 22]
strength  = [140, 24]
reflex    = [150, 22]
aggression         = [120, 35]
self_preservation  = [125, 30]
loyalty            = [155, 25]
composure          = [170, 22]
morale_bonus = 80
```

## 7. アーキタイプ

```toml
[[archetype]]
id = "honor_hungry_knight"
name_ja = "名誉に飢えた若騎士"
name_en = "Honor-hungry Knight"
description_ja = "武功を立てることに取り憑かれている。命令より目の前の栄誉を選ぶ"
# [平均, 標準偏差]
boldness   = [230, 20]
caution    = [ 40, 18]
initiative = [210, 25]
obedience  = [ 60, 22]
tactical_skill = [90, 30]
ambition   = [245, 12]
charisma   = [140, 30]
flexibility= [ 70, 25]
patience   = [ 30, 15]
ruthlessness=[120, 35]
```

## 8. AI の重み

評価関数の係数をすべて外に出す。

```toml
[soldier.flee]
threat_local        = 3.0
morale_deficit      = 4.0
fatigue             = 2.0
rear_threat         = 5.0
nearby_broken       = 3.0
discipline          = -3.0
commander_near      = -2.0
banner_near         = -2.0
bravery             = -3.0
frontline_ahead     = -400.0

[soldier.attack]
target_exposed      = 3.0
target_wounded      = 2.0
target_is_noble     = 4.0
morale              = 2.0
fatigue             = -2.0
out_of_slot         = -3.0

[commander.envelop]
force_ratio         = 3.0
enemy_flank_exposed = 4.0
reserve_available   = 2.0
boldness            = 3.0
tactical_skill      = 2.0
enemy_cavalry_uncommitted = -3.0
caution             = -3.0

[noise]
decision_noise_base = 40.0     # composure で割られる
```

## 9. 士気

```toml
[morale.decrease]
comrade_death        = 8
comrade_death_radius_m = 8
comrade_broken       = 12
comrade_broken_radius_m = 10
self_wounded_light   = 60
self_wounded_heavy   = 200
flank_enemy          = 25
rear_enemy           = 70
commander_death      = 120
banner_lost          = 150
parent_collapsed     = 80
missile_impact       = 3
cavalry_charge_hit   = 100
out_of_ammo          = 30
formation_broken     = 40

[morale.increase]
enemy_unit_broken    = 40
kill                 = 15
commander_near       = 2      # ×charisma、思考ごと
banner_near          = 1.5
high_ground          = 15
pushing              = 20
rest                 = 1
wounded_recovered_nearby = 15

[morale.thresholds]
wavering = 400
break    = 250
rally    = 450

[morale.contagion]
susceptibility_divisor = 2048
broken_cascade_min = 3
broken_cascade_coef = 6
```

## 10. シナリオ

### 実装済みの範囲

TOML の実行時ローダー（`sim-data`）はまだ無い。`data/scenarios/*.toml` は
人間が読む正本で、実際に読まれるのは同じ内容を写した
`sim_core::scenario` の定数（`data/formations.toml` と
`organization::formation_def` の関係と同じ）。

下のスキーマのうち、現時点で解釈できるのは次の範囲。

| 節 | 状態 |
|---|---|
| `[terrain]` | 生成パラメータ + シナリオ固有の地形整形。**正本は `web/src/terrain/scenarios.ts`**（03 章 0 節・3.7 節） |
| `[[army]]` | 軍 → 部隊の 2 階層。中間階層（Battle/Banner/Company）はまだ作らない |
| `army.contingent` | 兵科・練度・装備・陣形・配置・杭列 |
| `commander` | アーキタイプ（性格）と、軍単位の会戦プラン |
| `[time]` | 未実装（準備時間・開始時刻） |
| `[victory]` | 未実装（勝敗は死傷と残存で読む） |
| `weather` | 未実装。結果である泥濘を地形整形で焼き込んで代用する |

`[terrain]` だけは他の節と扱いが違う。地形の生成器は `web/src/terrain`
（TypeScript）にあり、Rust の `sim_core::scenario` は地形パラメータを持たない
——受け取るのは出来上がったグリッドだけになる（03 章 0 節）。生成パラメータと
整形の正本は `web/src/terrain/scenarios.ts` で、この TOML はその写しである。

シナリオは**命令を持たない**。両軍は陣形の上に立つだけで、動き出すのは
各軍の指揮官 AI（05 章）が性格と会戦プランから下す判断による。

```toml
[scenario]
id = "agincourt_like"
name_ja = "泥濘の隘路"
description_ja = "森に挟まれた耕地。雨で泥濘化している。防御側は数で劣る"

[terrain]
seed = "0x4A17_C0FF"
size_m = 4000
relief = 0.25
sea = "none"
river_density = 0.2
forest_cover = 0.45
marsh_bias = 0.35
road_count = 1
weather = "Rain"
weather_hours_before = 12       # 12 時間降り続いた後。泥濘が広がっている

[time]
start_hour = 11                 # 会戦開始時刻（日照と疲労に影響）
prep_minutes = 90               # 会戦前の準備時間（築城ができる）
max_duration_minutes = 240

[[army]]
faction = "english_1415"
side = "defender"
deploy_area = { x = 1200, y = 1800, w = 900, h = 400, facing = 90 }
commander = { archetype = "cautious_commander", name_ja = "ヘンリー" }
plan = "DefendHighGround"

  [[army.battle]]
  role = "Main"
  commander = { archetype = "veteran_mercenary" }
    [[army.battle.unit]]
    troop = "men_at_arms"
    count = 900
    quality = "veteran"
    formation = "line"
    [[army.battle.unit]]
    troop = "longbowmen"
    count = 5000
    quality = "professional"
    formation = "stakes_line"

  [[army.battle]]
  role = "Engineers"
    [[army.battle.unit]]
    troop = "engineers"
    count = 200
    quality = "professional"
    # 準備時間中に自動でこれをやる
    initial_tasks = [
      { kind = "Stakes", area = "front_line", priority = 250 },
      { kind = "Ditch",  area = "left_flank", priority = 180 },
    ]

[[army]]
faction = "french_1415"
side = "attacker"
deploy_area = { x = 1200, y = 2600, w = 1100, h = 600, facing = 270 }
commander = { archetype = "stubborn_baron", name_ja = "ダルブレ" }
plan = "CenterPush"
# ... 以下同様

[victory]
# 勝敗判定
condition = "ArmyCollapse"      # 一方が崩壊するまで
# または "HoldUntil" { minutes = 180 } / "SeizeArea" { area = "bridge" }
```

## 11. リプレイ

```json
{
  "version": 1,
  "created": "2026-08-15T09:20:00Z",
  "world_seed": "0x5EED1234ABCD",
  "scenario_hash": "sha256:9f2a...",
  "scenario_inline": { "...": "シナリオ TOML を JSON 化したもの" },
  "data_hash": "sha256:c81b...",
  "commands": [
    { "tick": 1200, "issuer": 3, "target": 14,
      "intent": { "MoveTo": { "x": 2800.0, "y": 1500.0, "facing": 16384,
                              "speed": "Quick", "formation": "line" } } },
    { "tick": 3400, "issuer": 3, "target": 22,
      "intent": { "Charge": { "target": 51 } } }
  ],
  "hash_checkpoints": [
    { "tick": 10000, "hash": "0x8A3F..." },
    { "tick": 20000, "hash": "0x1C7E..." }
  ],
  "final_tick": 47820,
  "final_hash": "0xB204..."
}
```

`data_hash` はデータファイル群のハッシュ。バランス調整でデータが変わると
古いリプレイは再現しなくなるので、不一致を検出して警告する。

`hash_checkpoints` は再生中の検証用。ズレたら**どこでズレたか**が分かる。

## 12. バージョニング

- `version` フィールドを全ファイルに持たせる
- シミュレーションのロジックが変わったら `SIM_VERSION` を上げる
- リプレイは `SIM_VERSION` が一致するときのみ完全再現を保証する。
  不一致なら「観賞用の再生」として動かすが、結果が変わりうる旨を UI に出す
