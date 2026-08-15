# 06. 戦闘と士気

## 1. 空間索引

すべての近傍クエリの土台。

```rust
pub struct SpatialHash {
    cell_m: Fx,                  // 2 m
    dims: (u32, u32),            // 2500 × 2500
    cell_start: Vec<u32>,        // カウントソートの開始 index
    entries: Vec<u32>,           // ソート済みの soldier id
}
```

**毎 tick、カウントソートで再構築**する。O(n) で、50,000 体なら 0.3 ms 程度。
可変長リストや連結リストを使わないのでキャッシュに優しく、決定論的
（同じ入力なら常に同じ配列順）。

- 近傍クエリは 3×3 セル = 36 m²。密集陣形（0.5 m 間隔 = 4 人/m²）でも
  最大 144 人だが、**取得上限を 12 人に切る**（先頭から決定論的に）。
- 陣形が密なときは近い者だけで十分。上限があることで最悪計算量が保証される。

## 2. 移動と衝突

### 2.1 移動積分

```
desired_speed = attrs.speed_base
              × formation.move_mult
              × terrain.move_mult(surface)
              × slope_factor(gradient)
              × fatigue_factor(fatigue)
              × encumbrance_factor(equip.weight, terrain.is_mud)
              × state_mult(state)

accel  = attrs.accel × fatigue_factor
vel    = approach(vel, steering.desired.normalized × desired_speed, accel × dt)
pos   += vel × dt
```

`encumbrance_factor` は**泥では装備重量が強く効く**ようにする。
プレートアーマー（25 kg）の兵士が泥を歩くと速度が半減し疲労が倍増する。
これがアジンクールの再現に必要。

### 2.2 衝突解決（押し合い）

兵士は半径 `r` の円。既定 0.35 m（肩幅ベース）、騎乗時は 1.2 m × 0.5 m を
円で近似して 0.9 m。

```
for iteration in 0..2:
    for each soldier i:
        for each neighbor j in 3x3 cells (max 12):
            d² = dist_sq(i, j)
            if d² < (r_i + r_j)²:
                overlap = (r_i + r_j) - sqrt(d²)
                push_dir = normalize(pos_i - pos_j)
                // 質量比で分配。重い方が押し勝つ
                m_i = mass(i), m_j = mass(j)
                pos_i += push_dir × overlap × (m_j / (m_i + m_j)) × relax
                pos_j -= push_dir × overlap × (m_i / (m_i + m_j)) × relax
```

- `relax` = 0.5。2 反復で収束させる（完全に解消しなくてよい。むしろ多少
  重なったままの方が密集の圧力が表現される）
- `mass` = 体重 + 装備重量 + `strength` 補正。騎兵は馬込みで 500〜600 kg なので、
  歩兵を跳ね飛ばす
- 押し合いは**書き込みフェーズを分離**し、同 tick 内で他者の更新後位置を読まない
  （決定論とスレッド化のため）

### 2.3 押し合いによる前進・後退

白兵戦中、両陣営の押す力が拮抗する。

```
unit_push(A) = Σ over soldiers in A of (mass × effort × morale_factor × ranks_behind_factor)
```

`ranks_behind_factor` は「自分の後ろに何列いるか」。後列は前列を押す。
これにより**厚みのある陣形が薄い陣形を押し込む**。押し込まれた側は
陣形が乱れ、`cohesion` が落ちる。

### 2.4 圧迫（crush）

局所密度が `4 人/m²` を超えると圧迫状態になる。

- 密度 4〜6: 移動不能、攻撃不能（武器を振れない）、疲労が急増
- 密度 6 以上: 継続で `Downed` 判定（窒息・圧死）。中世会戦で実際に多かった死因
- 倒れた兵士はさらに周囲の通行を妨げ、圧迫を悪化させる（連鎖）

これは「密集させれば強い」の上限を作る自然なメカニズムになる。

## 3. 白兵戦

### 3.1 エンゲージメント

兵士 A が敵 B と交戦するのは:

```
dist(A, B) ≤ A.weapon.reach + r_A + r_B
かつ  angle_between(A.facing, dir(A→B)) ≤ 60°
かつ  A の前方に味方が立ち塞がっていない（= A は前列である）
```

**前列判定**が重要。A の facing 方向 ±30°、距離 0.8 m 以内に味方がいれば
A は後列であり、攻撃できない（押すだけ）。これで「前列だけが戦う」が実装される。

武器の `reach`:

| 武器 | reach |
|---|---|
| ダガー | 0.3 m |
| 剣 | 0.9 m |
| ロングソード | 1.2 m |
| 戦斧・メイス | 0.8 m |
| 槍 | 2.2 m |
| ハルバード・ビル | 2.4 m |
| パイク | 5.0 m |
| ランス（騎乗） | 3.5 m |

リーチが長い方が先に攻撃できる。パイクは接近される前に複数列が攻撃できる
（後列のパイクも前に届く = パイク方陣が正面極強な理由）。

### 3.2 攻撃サイクル

各兵士は独立した攻撃タイマーを持つ。

```
swing_time = weapon.base_swing_ms
           × (2000 - attrs.skill × 4) / 1000     // 熟練者は速い
           × fatigue_penalty(fatigue)            // 疲れると遅い
           × formation_crowding_penalty          // 密集で振りにくい
```

タイマー満了で 1 回の攻撃判定。

### 3.3 命中と防御

```
attack_roll  = attrs.skill
             + weapon.accuracy
             - fatigue × 0.4
             + flank_bonus            // 側面 +40 / 背面 +80
             + height_bonus           // 高い位置から +15
             + formation_bonus
             + rng(-30, +30)

defense_roll = defender.attrs.skill × defense_stance_mult
             + shield.block
             - defender.fatigue × 0.5
             - (defender.state == Broken ? 100 : 0)
             - (defender.engaged_count - 1) × 25   // 複数を相手にすると捌けない
             + rng(-30, +30)

if attack_roll > defense_roll:  命中 → ダメージ判定へ
else:                           受け / 回避 / 盾で防御
```

**防御姿勢**は防御側 AI が選ぶ。`self_preservation` が高いと防御的な構えを
取りやすく、被弾は減るが自分の攻撃機会も減る。

**`engaged_count`** は「今何人に囲まれているか」。3 人に囲まれた兵士は
ほぼ確実に死ぬ。包囲が致命的なのはこれで表現される。

### 3.4 ダメージ

部位ごとに装甲値が違う。

```
hit_location = weighted_random { Head 12%, Torso 45%, Arms 23%, Legs 20% }
armor  = equip.armor_at(hit_location)     // 部位別
```

武器はダメージタイプを持ち、装甲との相性が異なる。

| タイプ | 布/革 | メイル | プレート | 備考 |
|---|---|---|---|---|
| `Cut` 斬撃 | 1.0 | 0.35 | 0.10 | メイルに極端に弱い |
| `Pierce` 刺突 | 0.9 | 0.65 | 0.30 | 隙間を狙える |
| `Blunt` 打撃 | 0.7 | 0.85 | 0.70 | 装甲を通して衝撃が届く |
| `Missile` 射撃 | — | 距離依存 | 距離依存 | 3.6 節 |

```
damage = weapon.power
       × (attrs.strength + 128) / 256
       × type_vs_armor[weapon.type][armor.class]
       × (1 - armor.coverage_at(location) × armor.quality)
       × momentum_mult                   // 突撃中は大きい
       × fatigue_mult(attacker)
```

**メイル相手にはメイスが有効、プレート相手にはポールアクスや刺突**という
史実の傾向が、テーブルから自然に出る。

### 3.5 負傷の段階

```
hp 100..70:  軽傷。skill -10%, speed -5%
hp  70..40:  中傷。skill -25%, speed -20%, 出血（毎秒 hp 減）
hp  40..15:  重傷。skill -50%, speed -50%, 出血大、士気大幅低下
hp  15..0 :  Downed。行動不能。工兵に回収されれば一部が生還
hp     0 :  Dead
```

即死は頭部への強打撃・刺突でのみ発生する（確率的に）。それ以外は段階的に
弱っていく。`Downed` の兵士は地面に残り、通行の障害になり、周囲の士気を下げる。

### 3.6 射撃

```rust
pub struct Projectile {
    pub pos: Vec2Fx, pub z: Fx,
    pub vel: Vec2Fx, pub vz: Fx,
    pub shooter: SoldierId,
    pub kind: MissileKind,   // Arrow | Bolt | Stone | Javelin
    pub power: u16,
}
```

- 弾道は固定小数点で放物線を積分（重力 9.8 m/s² を Fx 化）。1 tick ごとに更新。
- 兵士は**個々の敵ではなく目標エリア**を狙う。狙い点に散布（`spread`）を乗せる。
  散布は距離・`skill`・疲労・風で広がる。
- 着弾セルの兵士に対して命中判定。密集していれば当たりやすい
  （**面積命中**: セル内の兵士の占有面積比を命中確率にする）。
- **味方誤射**: 射線上に味方がいれば当たる。低い弾道（弩・直射）ほど危険で、
  高い弾道（長弓の曲射）は味方の頭越しに飛ぶ。隊長 AI はこれを考慮する。

装甲貫通は距離で減衰する。

```
armor_pen = missile.base_pen × (1 - distance / max_range)^1.5
if armor_pen > armor.value:  通常ダメージ
else:                        大幅減衰（矢がプレートを弾く）
```

| 武器 | 射程 | 発射間隔 | 貫通 | 携行数 | 備考 |
|---|---|---|---|---|---|
| 長弓 | 250 m | 6 s | 中 | 60〜72 | 訓練に年単位。曲射で面制圧 |
| 弩（片足鐙） | 180 m | 12 s | 高 | 40 | |
| 弩（クランキン） | 220 m | 30 s | 極高 | 40 | プレートも抜く |
| 投石紐 | 120 m | 5 s | 低 | 30 | 打撃ダメージ。ヘルムに有効 |
| 投槍 | 25 m | 4 s | 中 | 3 | 接近直前の一撃 |

**矢は有限**。尽きた弓兵は白兵戦に移るか後退する。工兵の補給（07 章）が
射撃の継続時間を決める。

## 4. 騎兵

### 4.1 突撃

突撃の価値は**運動量**にある。

```
momentum = (horse.mass + rider.mass) × velocity²  を正規化した値
```

- 停止状態からでは momentum ≈ 0。**騎兵は走ってこそ騎兵**。
- 加速には距離が要る。最高速 8 m/s に達するには平地で約 100 m。
- 最適な突撃開始距離は 150〜250 m。それより遠いと接触前に馬が疲れる
  （`horse.fatigue` が上がり速度が落ちる）。
- 上り坂では momentum が大きく減り、下り坂では増える。

接触時:

```
衝撃を受けた歩兵:
  ダメージ = base × momentum × (1 - target.anti_cavalry_mult)
  ノックバック = momentum / target.mass 方向 = 突撃方向
  士気ダメージ = 大（突撃を受けた側の周囲全員に伝播）
  貫通: momentum が十分なら騎兵は列を突き抜けて次の列へ
```

突撃後、騎兵は速度を失う（`Engaged` 状態へ）。**この状態の騎兵は弱い**。
隊長 AI は離脱して再編し、再突撃するか判断する。これが中世騎兵の実際の使い方。

### 4.2 馬の忌避（horse refusal）

馬は自律的な恐怖判定を持つ。

```
refusal_chance = base
               + spear_wall_density × 3      // 突き出た槍の密度
               + target_cohesion × 2         // 崩れていない密集陣
               + horse.fear
               - rider.skill                 // 熟練の騎手は馬を御す
               - horse.training
               - momentum × 2                // 勢いがついていると止まれない
```

判定に失敗すると馬は減速・停止・横に逸れる。突撃が失敗する。

**槍衾に正面から突っ込まない**という中世騎兵の現実がこれで再現される。
騎兵の正しい使い方は、崩れた敵・側面・背面・射撃で乱れた敵を突くこと。

### 4.3 馬と落馬

```rust
pub struct Horses {
    pub hp: Vec<u16>,
    pub fatigue: Vec<u16>,
    pub fear: Vec<u16>,
    pub rider: Vec<u32>,
    pub speed: Vec<u16>, pub mass: Vec<u16>,
}
```

- 馬は独立に被弾する（馬体は大きく当たりやすい。実際、対騎兵では馬を狙う）
- 馬が倒れると騎手は落馬。落馬ダメージ + `Downed` 判定。
  生き延びれば徒歩の重装兵として戦う（プレートの騎士は徒歩でも強いが、
  疲労が激しく、囲まれれば死ぬ）
- 馬の疲労は騎手より早く蓄積する。連続突撃はできない
- 杭列・堀・鹿砦に対しては忌避判定が極端に厳しくなり、突っ込めば馬が死ぬ

### 4.4 追撃

敵が敗走すると、騎兵は追撃したくなる。

- `aggression` と `ruthlessness` が高いと追撃に入る
- 追撃中の騎兵は**戦場から離れ、戻ってくるのに時間がかかる**。決定的局面で
  騎兵がいない、という中世会戦の典型的な失敗が起きる
- `Pursue { max_distance_m }` 命令で制限できるが、`discipline` が低い部隊は
  制限を超えて追う
- 敗走兵に対する追撃の殺傷率は極めて高い（防御ほぼゼロ、背面ボーナス）。
  **会戦の死者の大半はここで出る**

## 5. 士気

### 5.1 個人士気

```
morale: u16   // 0..1000
```

初期値:

```
morale_0 = 400
         + attrs.bravery × 1.2
         + unit.cohesion × 0.2
         + commander.charisma × 0.3
         + (banner_present ? 60 : 0)
         + quality_bonus
```

### 5.2 増減要因

毎思考で以下を積算する（値は `data/morale.toml`）。

**減少**:

| 要因 | 量 | 備考 |
|---|---|---|
| 近くの味方の死 | −8 / 人 | 距離で減衰（半径 8 m） |
| 近くの味方の敗走 | −12 / 人 | パニックの伝染源。距離減衰 |
| 自分の負傷 | −60 〜 −200 | 重傷度に比例 |
| 疲労 | −0.02 / tick × fatigue率 | じわじわ効く |
| 側面に敵 | −25 / 思考 | |
| **背面に敵** | −70 / 思考 | 最も強い。包囲が士気を折る |
| 指揮官の死 | −120 | 距離減衰、指揮下全員 |
| 旗の喪失 | −150 | 部隊全員 |
| 上位部隊の崩壊 | −80 | 隣の Battle が崩れると連鎖する |
| 数的劣勢の視認 | −0〜50 | 局所の敵味方密度比 |
| 矢の雨 | −3 / 着弾 | 損害が小さくても士気を削る |
| 騎兵突撃を受ける | −100 | 衝撃 |
| 弾薬切れ | −30 | 射手のみ |
| 陣形の崩壊 | −40 | cohesion が閾値を割る |

**増加**:

| 要因 | 量 |
|---|---|
| 敵の敗走を見る | +40 / 敵 Unit |
| 敵を倒す | +15 |
| 指揮官が近く（20 m 以内） | +2 / 思考 × charisma |
| 旗が近い | +1.5 / 思考 |
| 味方の密度が高い | +0〜25 |
| 高地にいる | +15 |
| 押している（momentum 正） | +20 |
| 休息（非交戦で 30 秒以上） | +1 / 思考 |

### 5.3 パニックの伝播

士気の伝播が**この仕様の中心的メカニズム**。

```rust
// 各思考で、近傍の士気に引き寄せられる
let neighbor_avg = mean(perception.neighbors.map(|n| morale[n]));
let susceptibility = (255 - attrs.composure) as i32;  // 動じにくさの逆
morale[i] += (neighbor_avg - morale[i]) × susceptibility / 2048;
```

さらに `local_broken`（近傍で敗走中の味方の数）が非線形に効く。

```
if local_broken >= 3:
    morale[i] -= local_broken² × 6 × susceptibility / 255
```

これにより、**局所的な崩壊が閾値を超えると連鎖的に広がる**。1 人が逃げても
何も起きないが、3〜4 人が逃げると周囲が引きずられ、そこから雪崩が始まる。
中世会戦の「ある瞬間に軍が消える」現象がこれで出る。

### 5.4 状態遷移

```
morale > 400          : 正常
morale 250..400       : Wavering  命令には従うが、逃走スコアが高い。攻撃力低下
morale < 250          : Broken 判定（確率的、bravery で抵抗）
                        → Broken 状態へ
```

`Broken` の兵士:

- 最寄りの安全方向（自軍後方、敵から遠い方向、地形の逃げ道）へ全力で走る
- 命令を受け付けない
- 防御力が大幅に低下（背を向けている）
- 走りながら周囲の味方の士気を削る（伝染）
- 武器を捨てることがある（`ruthlessness` の低い側は捕虜にできる、将来拡張）

### 5.5 再結集（rally）

`Broken` から復帰する条件:

```
rally_chance = base
             + commander_near × charisma × 3     // 指揮官が止めに来る
             + banner_near × 2
             + (enemy_dist > 100m ? 40 : 0)
             + attrs.bravery
             + attrs.discipline
             - fatigue
             - time_broken                        // 長く逃げるほど戻りにくい
```

再結集した部隊は士気が回復するが、上限が下がる（一度折れた部隊は脆い）。
指揮官が自ら敗走兵の前に立つのは危険（敵に近づく）だが、
`boldness` の高い指揮官はそうする。

### 5.6 全軍の崩壊

Army ノードの `broken_ratio` が閾値（既定 40%）を超えるか、
総大将が死亡して継承が失敗すると、**全軍崩壊**。全 Unit が `Broken` 判定を
受け、会戦は終了フェーズ（追撃）に入る。

## 6. 疲労

```
fatigue: u16   // 0..10000
```

| 行為 | 消費 / 秒 |
|---|---|
| 待機 | −2（回復） |
| 歩行 | +3 |
| 早足 | +8 |
| 全力疾走 | +25 |
| 白兵戦（攻撃） | +40 |
| 白兵戦（防御） | +25 |
| 押し合い | +15 |
| 弓を引く | +12 |
| 工兵作業 | +18 |

修正:

```
実消費 = base × equip_weight_factor × terrain_fatigue_mult × (2 - endurance/255)
```

- プレートアーマー（25 kg）は消費 1.6 倍。泥ではさらに 1.9 倍 → 合計 3 倍
- 疲労 6000 超で命中率 −30%、速度 −25%、`swing_time` +40%
- 疲労 8500 超で攻撃がほぼ通らなくなり、士気も削られる

**中世会戦は疲労で決まる**。開始 30 分で重装歩兵は限界に近づく。
指揮官は交代（後列と入れ替え）を命じられる。これができる規律の高い部隊が
持久戦に強い。
