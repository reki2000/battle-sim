//! M4 の戦闘と士気。
//!
//! このモジュールは、兵士の SoA にある HP・士気・状態を戦闘の確定値として
//! 更新する。装備・攻撃タイマー・敗走時間のような M4 固有の配列はここに
//! 置くことで、M3 の指揮ツリーと既存の描画用レイアウトを変更しない。
//!
//! すべての判定は整数で行い、乱数は兵士 ID・目的・tick から導出する。攻撃を
//! ID 順に解決しても、各判定そのものは呼び出し順に依存しない。

use sim_math::{
    angle_diff, brad_from_deg, dist, dist_sq, fx_from_mm, ms_to_ticks, per_sec_to_per_tick,
    within_arc, Brad, Fx, Purpose, Rng, Vec2Fx,
};

use crate::soldiers::{
    flags, Attrs, SoldierId, Soldiers, State, MAX_FATIGUE, MAX_HP, MAX_MORALE, NO_ID,
};
use crate::spatial::{CoarseIndex, SpatialHash, MAX_NEIGHBORS};

/// 白兵戦で参照するダメージ種別。
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DamageType {
    Cut,
    Pierce,
    Blunt,
    Missile,
}

/// 防具の大分類。相性表の軸になる。
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArmorClass {
    ClothLeather,
    Mail,
    Plate,
}

/// 攻撃部位。値は仕様 06 章 3.4 の重みで選ばれる。
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitLocation {
    Head,
    Torso,
    Arms,
    Legs,
}

/// 白兵武器の最小データ。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Weapon {
    /// リーチ（固定小数点の m）。射撃武器では最大射程として使う。
    pub reach: Fx,
    /// 基本の振り時間。射撃武器では装填・照準を含む発射間隔。
    pub base_swing_ms: u16,
    /// 命中値への加算。
    pub accuracy: i16,
    /// 筋力補正前の威力。
    pub power: u16,
    pub damage_type: DamageType,
    /// 後列から攻撃できる武器（基礎スコープではパイクと弓兵科）。
    pub requires_front_row: bool,
    /// 射撃武器か（true なら白兵の間合いではなく `CombatSystem` の
    /// 射撃パスで解決し、着弾に遅延を持たせる）。
    pub ranged: bool,
}

impl Weapon {
    pub const fn dagger() -> Self {
        Self {
            reach: fx_from_mm(300),
            base_swing_ms: 700,
            accuracy: 4,
            power: 24,
            damage_type: DamageType::Pierce,
            requires_front_row: true,
            ranged: false,
        }
    }

    pub const fn sword() -> Self {
        Self {
            reach: fx_from_mm(900),
            base_swing_ms: 1100,
            accuracy: 0,
            power: 32,
            damage_type: DamageType::Cut,
            requires_front_row: true,
            ranged: false,
        }
    }

    pub const fn mace() -> Self {
        Self {
            reach: fx_from_mm(800),
            base_swing_ms: 1300,
            accuracy: -2,
            power: 38,
            damage_type: DamageType::Blunt,
            requires_front_row: true,
            ranged: false,
        }
    }

    pub const fn spear() -> Self {
        Self {
            reach: fx_from_mm(2200),
            base_swing_ms: 1500,
            accuracy: 2,
            power: 34,
            damage_type: DamageType::Pierce,
            requires_front_row: true,
            ranged: false,
        }
    }

    pub const fn pike() -> Self {
        Self {
            reach: fx_from_mm(5000),
            base_swing_ms: 1800,
            accuracy: -4,
            power: 42,
            damage_type: DamageType::Pierce,
            requires_front_row: false,
            ranged: false,
        }
    }

    /// ランス。騎乗突撃用。リーチは長いが、徒歩では扱いにくいので
    /// 落馬後は他の武器に持ち替える運用を想定する（仕様 06 章 3.1 節）。
    pub const fn lance() -> Self {
        Self {
            reach: fx_from_mm(3500),
            base_swing_ms: 2000,
            accuracy: -2,
            power: 30,
            damage_type: DamageType::Pierce,
            requires_front_row: true,
            ranged: false,
        }
    }

    /// 長弓。射程が長く威力も高いが、発射間隔も長い。
    pub const fn longbow() -> Self {
        Self {
            reach: fx_from_mm(120_000),
            base_swing_ms: 3500,
            accuracy: -2,
            power: 22,
            damage_type: DamageType::Missile,
            requires_front_row: false,
            ranged: true,
        }
    }

    /// 弩。命中は安定するが発射間隔が長く連射が利かない。
    pub const fn crossbow() -> Self {
        Self {
            reach: fx_from_mm(80_000),
            base_swing_ms: 6000,
            accuracy: 8,
            power: 30,
            damage_type: DamageType::Missile,
            requires_front_row: false,
            ranged: true,
        }
    }
}

impl Default for Weapon {
    fn default() -> Self {
        Self::sword()
    }
}

/// 防具の部位別防御値と被覆率。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Armor {
    pub class: ArmorClass,
    pub head: u8,
    pub torso: u8,
    pub arms: u8,
    pub legs: u8,
    /// 0..=1000。命中部位を覆う割合。
    pub coverage_permille: u16,
    /// 0..=1000。防具の品質。
    pub quality_permille: u16,
    /// 盾による防御判定への加算値。
    pub shield_block: i16,
}

impl Armor {
    pub const fn cloth() -> Self {
        Self {
            class: ArmorClass::ClothLeather,
            head: 8,
            torso: 10,
            arms: 6,
            legs: 6,
            coverage_permille: 500,
            quality_permille: 500,
            shield_block: 0,
        }
    }

    pub const fn mail() -> Self {
        Self {
            class: ArmorClass::Mail,
            head: 32,
            torso: 36,
            arms: 25,
            legs: 24,
            coverage_permille: 750,
            quality_permille: 700,
            shield_block: 12,
        }
    }

    pub const fn plate() -> Self {
        Self {
            class: ArmorClass::Plate,
            head: 65,
            torso: 72,
            arms: 54,
            legs: 50,
            coverage_permille: 850,
            quality_permille: 800,
            shield_block: 8,
        }
    }

    #[inline]
    pub const fn armor_at(self, location: HitLocation) -> u8 {
        match location {
            HitLocation::Head => self.head,
            HitLocation::Torso => self.torso,
            HitLocation::Arms => self.arms,
            HitLocation::Legs => self.legs,
        }
    }
}

impl Default for Armor {
    fn default() -> Self {
        Self::cloth()
    }
}

/// HP から導出する負傷段階。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InjuryStage {
    Light,
    Medium,
    Heavy,
    Downed,
    Dead,
}

impl InjuryStage {
    #[inline]
    pub const fn from_hp(hp: u16) -> Self {
        match hp {
            0 => Self::Dead,
            1..=15 => Self::Downed,
            16..=40 => Self::Heavy,
            41..=70 => Self::Medium,
            _ => Self::Light,
        }
    }
}

/// 会戦の大きな状態。Army ノードそのものは M3 の責務なので、ここでは
/// 局所的な broken 比率から追撃へ移る基礎だけを持つ。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BattlePhase {
    #[default]
    Battle,
    Pursuit,
    Complete,
}

/// 死因の内訳。M4 の受け入れ条件「損害の内訳で追撃が最大の死因になる」を
/// 検証できるよう、キルはどれか 1 つの原因に分類する。追撃は他の原因より
/// 優先して判定する（追撃中・敗走中に受けた最後の一撃は武器種によらず
/// 「追撃」を死因とする）。
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeathCause {
    Melee,
    Missile,
    Crush,
    Bleed,
    Pursuit,
    /// 騎兵の突撃（衝撃）による死。仕様 12 章 M5、06 章 4.1 節。
    Charge,
}

/// 戦闘の集計値。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CombatStats {
    pub attacks: u32,
    pub hits: u32,
    pub damage: u32,
    pub kills: u32,
    pub downed: u32,
    pub pursuit_kills: u32,
    pub melee_kills: u32,
    pub missile_kills: u32,
    pub crush_kills: u32,
    pub bleed_kills: u32,
    pub shots_fired: u32,
    pub friendly_fire_hits: u32,
    /// 騎兵突撃の衝撃で倒れた数（仕様 12 章 M5）。
    pub charge_kills: u32,
    /// 馬が倒れて落馬した回数。
    pub dismounts: u32,
    /// 馬の忌避（refusal）で突撃が失敗した回数。
    pub horse_refusals: u32,
}

/// UI・戦闘報告向けのイベント種別。
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CombatEventKind {
    Hit,
    Downed,
    Killed,
    Broken,
    Rallied,
    /// 馬が倒れて騎手が落馬した（仕様 12 章 M5「落馬と徒歩化」）。
    Dismounted,
    /// 馬が忌避して突撃が失敗した（仕様 06 章 4.2 節）。
    HorseRefused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CombatEvent {
    pub tick: u32,
    pub attacker: SoldierId,
    pub defender: SoldierId,
    pub kind: CombatEventKind,
    pub cause: DeathCause,
}

/// イベントログの保持上限。UI は直近のイベントだけを見るので、長時間戦でも
/// メモリが際限なく増えないように打ち切る（`organization::CommandEvent` は
/// 発生数が少ないため無制限だが、こちらは 1 tick に数千件出うる）。
const MAX_EVENTS: usize = 4096;

/// この圧力（‰、push 量を半径に対する割合で表す）を超えた分だけ圧迫の実害になる。
/// 通常の押し合いや行進中のジョストルはこれを超えない。
const CRUSH_THRESHOLD_PERMILLE: u32 = 600;

/// 盾を構えて受け止める姿勢の防御ボーナス（仕様 06 章 3.8 節）。
const BRACED_DEFENSE_BONUS: i32 = 35;

/// 射撃の標的探索に使う粗いセルの一辺（m）。近接戦用の `SpatialHash`
/// （2 m セル）では弓の射程まで届かないための専用インデックス。
const RANGED_CELL_M: i32 = 48;

#[derive(Clone, Copy, Debug)]
struct DamageIntent {
    attacker: SoldierId,
    defender: SoldierId,
    amount: u16,
    location: HitLocation,
    instant_death: bool,
    cause: DeathCause,
}

/// 飛んでいる最中の矢・ボルト。着弾まで `remaining_ticks` を数える。
#[derive(Clone, Copy, Debug)]
struct PendingShot {
    attacker: SoldierId,
    target: SoldierId,
    aim_point: Vec2Fx,
    remaining_ticks: u16,
    weapon: Weapon,
    attacker_skill: u8,
    attacker_strength: u8,
    range_permille: u32,
}

/// 標準的な矢筒・ボルトケースの初期弾薬数。
const DEFAULT_AMMO: u16 = 24;

/// 馬の最大体力（仕様 06 章 4.3 節）。騎手とは独立に減っていく。
pub const MAX_HORSE_HP: u16 = 180;

/// 騎乗している兵士が被弾したとき、その被害のうち馬に向く割合。
/// 馬体は大きく当たりやすいので、騎手本体より高い割合を割く
/// （仕様「対騎兵では馬を狙う」）。残りは通常どおり騎手の HP を減らす。
const HORSE_DAMAGE_SHARE_PERMILLE: u32 = 650;

/// 落馬時に騎手が受ける追加ダメージ。
const DISMOUNT_FALL_DAMAGE: u16 = 18;

/// 戦闘システム。配列の index は `SoldierId` と一致する。
#[derive(Debug, Default)]
pub struct CombatSystem {
    pub weapons: Vec<Weapon>,
    pub armors: Vec<Armor>,
    attack_timer: Vec<u16>,
    broken_ticks: Vec<u16>,
    rally_ticks: Vec<u8>,
    targets: Vec<SoldierId>,
    engaged_count: Vec<u16>,
    intents: Vec<DamageIntent>,
    morale_pressure: Vec<u8>,
    pub ammo: Vec<u16>,
    pending_shots: Vec<PendingShot>,
    /// 馬の体力。騎乗していない兵士では常に 0（仕様 12 章 M5）。
    pub horse_hp: Vec<u16>,
    pub events: std::collections::VecDeque<CombatEvent>,
    pub phase: BattlePhase,
    pub stats: CombatStats,
}

impl CombatSystem {
    /// 兵士の生成に対応する。既存セーブやテストが直接 `Soldiers` を作った場合も
    /// `tick` 内の `ensure_len` が補完する。
    pub fn register(&mut self) {
        self.weapons.push(Weapon::default());
        self.armors.push(Armor::default());
        self.attack_timer.push(0);
        self.broken_ticks.push(0);
        self.rally_ticks.push(0);
        self.ammo.push(DEFAULT_AMMO);
        self.horse_hp.push(0);
    }

    fn ensure_len(&mut self, len: usize) {
        while self.weapons.len() < len {
            self.register();
        }
    }

    /// 騎乗状態を設定する。乗せるときは馬を満タンの体力で用意し、
    /// 下ろす（落馬させる）ときは体力を 0 にする。`Soldiers` 側の
    /// `flags::MOUNTED` は呼び出し側が別途更新する。
    pub fn set_mounted(&mut self, id: SoldierId, mounted: bool) {
        let i = id as usize;
        if i >= self.horse_hp.len() {
            return;
        }
        self.horse_hp[i] = if mounted { MAX_HORSE_HP } else { 0 };
    }

    /// 馬の体力。騎乗していなければ 0。
    #[inline]
    pub fn horse_hp(&self, id: SoldierId) -> u16 {
        self.horse_hp.get(id as usize).copied().unwrap_or(0)
    }

    /// 馬の忌避が発生したことをイベントログに記録する（`cavalry` モジュールから呼ぶ）。
    pub fn record_horse_refusal(&mut self, rider: SoldierId, target: SoldierId, tick: u32) {
        self.push_event(CombatEvent {
            tick,
            attacker: rider,
            defender: target,
            kind: CombatEventKind::HorseRefused,
            cause: DeathCause::Melee,
        });
    }

    /// 騎兵突撃の衝撃ダメージを外部（`cavalry` モジュール）から適用する。
    /// 通常の白兵ダメージと同じ経路（馬への被害分割・負傷段階・士気・
    /// 死因統計）を通す。
    pub fn apply_impact_damage(
        &mut self,
        attacker: SoldierId,
        defender: SoldierId,
        amount: u16,
        soldiers: &mut Soldiers,
        tick: u32,
    ) {
        if amount == 0 {
            return;
        }
        self.stats.damage = self.stats.damage.saturating_add(amount as u32);
        self.push_event(CombatEvent {
            tick,
            attacker,
            defender,
            kind: CombatEventKind::Hit,
            cause: DeathCause::Charge,
        });
        self.apply_raw_damage(
            defender,
            amount,
            soldiers,
            Some(attacker),
            false,
            DeathCause::Charge,
            tick,
        );
    }

    fn push_event(&mut self, event: CombatEvent) {
        self.events.push_back(event);
        if self.events.len() > MAX_EVENTS {
            self.events.pop_front();
        }
    }

    /// 兵士の武器と防具を変更する。
    pub fn set_loadout(&mut self, id: SoldierId, weapon: Weapon, armor: Armor) -> bool {
        let i = id as usize;
        if i >= self.weapons.len() {
            return false;
        }
        self.weapons[i] = weapon;
        self.armors[i] = armor;
        true
    }

    /// 弾薬数を設定する（射撃兵科の初期配備に使う）。
    pub fn set_ammo(&mut self, id: SoldierId, ammo: u16) -> bool {
        let i = id as usize;
        if i >= self.ammo.len() {
            return false;
        }
        self.ammo[i] = ammo;
        true
    }

    /// 現在の交戦相手。`NO_ID` なら交戦していない。
    #[inline]
    pub fn target(&self, soldiers: &Soldiers, id: SoldierId) -> Option<SoldierId> {
        soldiers
            .index_if_present(id)
            .and_then(|i| (soldiers.target[i] != NO_ID).then_some(soldiers.target[i]))
    }

    /// 1 tick 分の白兵戦・負傷・士気・追撃を解決する。
    pub fn tick(
        &mut self,
        world_seed: u64,
        tick: u32,
        soldiers: &mut Soldiers,
        hash: &SpatialHash,
    ) {
        self.ensure_len(soldiers.len());
        self.apply_bleeding(tick, soldiers);
        self.update_phase(soldiers);

        let n = soldiers.len();
        self.targets.resize(n, NO_ID);
        self.targets.fill(NO_ID);
        self.engaged_count.resize(n, 0);
        self.engaged_count.fill(0);
        self.morale_pressure.resize(n, 0);
        self.morale_pressure.fill(0);
        self.intents.clear();
        let mut neighbors = [0u32; MAX_NEIGHBORS];
        let mut front_neighbors = [0u32; MAX_NEIGHBORS];

        // 交戦相手は全員の旧状態から先に決める。これで ID 順の攻撃解決が
        // その tick のターゲット選択を変えない。
        for i in 0..n {
            if !soldiers.is_alive(i) || soldiers.hot.state[i] == State::Rallying {
                continue;
            }
            let pos = soldiers.pos(i);
            let count =
                hash.query_enemies(soldiers, pos.x, pos.y, soldiers.faction[i], &mut neighbors);
            let mut best: Option<(i64, SoldierId)> = None;
            for &candidate_id in &neighbors[..count] {
                let j = candidate_id as usize;
                if !is_targetable(soldiers.hot.state[j]) {
                    continue;
                }
                if self.phase == BattlePhase::Pursuit && soldiers.hot.state[j] != State::Broken {
                    continue;
                }
                let candidate = soldiers.pos(j);
                let reach = self.weapons[i].reach + soldiers.radius(i) + soldiers.radius(j);
                let distance_sq = dist_sq(pos, candidate);
                if distance_sq > (reach as i64) * (reach as i64)
                    || !within_arc(soldiers.hot.facing[i], pos, candidate, 10_923)
                {
                    continue;
                }
                if self.weapons[i].requires_front_row
                    && !is_front_row(i, pos, soldiers, hash, &mut front_neighbors)
                {
                    continue;
                }
                if best.map_or(true, |current| (distance_sq, candidate_id) < current) {
                    best = Some((distance_sq, candidate_id));
                }
            }
            if let Some((_, target)) = best {
                self.targets[i] = target;
                self.engaged_count[target as usize] =
                    self.engaged_count[target as usize].saturating_add(1);
            }
        }

        // 射程の長い兵科（弓・弩）は、近接用の細かい空間ハッシュでは届かない
        // 距離の敵を探す必要がある。まだ標的のいない射撃兵だけが対象なので、
        // 該当者がいなければ粗い索引の構築自体を省く（仕様 12 章 M2/M4）。
        let ranged_seekers: Vec<u32> = (0..n as u32)
            .filter(|&idx| {
                let i = idx as usize;
                soldiers.is_alive(i)
                    && self.weapons[i].ranged
                    && self.targets[i] == NO_ID
                    && self.ammo[i] > 0
                    && !matches!(soldiers.hot.state[i], State::Broken | State::Rallying)
            })
            .collect();
        if !ranged_seekers.is_empty() {
            let ranged_index = CoarseIndex::build(RANGED_CELL_M, soldiers);
            let mut buf = [0u32; MAX_NEIGHBORS];
            for &idx in &ranged_seekers {
                let i = idx as usize;
                let pos = soldiers.pos(i);
                let count = ranged_index.query_excluding_faction(
                    soldiers,
                    pos.x,
                    pos.y,
                    soldiers.faction[i],
                    &mut buf,
                );
                let mut best: Option<(i64, SoldierId)> = None;
                for &candidate_id in &buf[..count] {
                    let j = candidate_id as usize;
                    if !is_targetable(soldiers.hot.state[j]) {
                        continue;
                    }
                    if self.phase == BattlePhase::Pursuit && soldiers.hot.state[j] != State::Broken
                    {
                        continue;
                    }
                    let candidate = soldiers.pos(j);
                    let reach = self.weapons[i].reach;
                    let distance_sq = dist_sq(pos, candidate);
                    if distance_sq > (reach as i64) * (reach as i64)
                        || !within_arc(soldiers.hot.facing[i], pos, candidate, 16_384)
                    {
                        continue;
                    }
                    if best.map_or(true, |current| (distance_sq, candidate_id) < current) {
                        best = Some((distance_sq, candidate_id));
                    }
                }
                if let Some((_, target)) = best {
                    self.targets[i] = target;
                }
            }
        }

        for i in 0..n {
            soldiers.target[i] = self.targets[i];
            if self.targets[i] != NO_ID {
                if !matches!(soldiers.hot.state[i], State::Wavering | State::Broken) {
                    soldiers.hot.state[i] = if self.weapons[i].ranged {
                        State::Shooting
                    } else {
                        State::Engaged
                    };
                }
            } else if matches!(soldiers.hot.state[i], State::Engaged | State::Shooting) {
                soldiers.hot.state[i] = State::Idle;
            }
        }

        for i in 0..n {
            if !soldiers.is_alive(i)
                || matches!(soldiers.hot.state[i], State::Broken | State::Rallying)
                || self.targets[i] == NO_ID
                // 転倒中は武器を振れない（`charge::ChargeSystem` が管理する）
                || soldiers.is_stumbling(i)
            {
                continue;
            }
            if self.attack_timer[i] > 0 {
                self.attack_timer[i] -= 1;
                continue;
            }
            let weapon = self.weapons[i];
            let attacker_attrs = soldiers.attrs[i];
            self.attack_timer[i] = swing_ticks(
                weapon,
                attacker_attrs,
                soldiers.fatigue[i],
                soldiers.hot.state[i],
                soldiers.is_braced(i),
            );

            if weapon.ranged {
                if self.ammo[i] == 0 {
                    continue;
                }
                self.ammo[i] -= 1;
                self.stats.shots_fired = self.stats.shots_fired.saturating_add(1);
                let defender = self.targets[i] as usize;
                let origin = soldiers.pos(i);
                let aim_point = soldiers.pos(defender);
                // 矢速はおよそ 45 m/s。飛翔中に着弾点は動かず、狙われた瞬間の
                // 位置へ飛ぶ（=標的がその間に動けば外れうる）。
                let speed_per_tick = per_sec_to_per_tick(fx_from_mm(45_000)).max(1);
                let distance = dist(origin, aim_point);
                let travel_ticks = ((distance as i64 + speed_per_tick as i64 - 1)
                    / speed_per_tick as i64)
                    .clamp(1, u16::MAX as i64) as u16;
                let range_permille =
                    ((distance as i64 * 1000) / (weapon.reach.max(1) as i64)).clamp(0, 1000) as u32;
                self.pending_shots.push(PendingShot {
                    attacker: i as SoldierId,
                    target: self.targets[i],
                    aim_point,
                    remaining_ticks: travel_ticks,
                    weapon,
                    attacker_skill: attacker_attrs.skill,
                    attacker_strength: attacker_attrs.strength,
                    range_permille,
                });
                continue;
            }

            let defender = self.targets[i] as usize;
            self.stats.attacks = self.stats.attacks.saturating_add(1);
            let defender_attrs = soldiers.attrs[defender];
            let flank_bonus = flank_bonus(
                soldiers.hot.facing[defender],
                soldiers.pos(defender),
                soldiers.pos(i),
            );
            let attack_roll = injury_skill(attacker_attrs.skill, soldiers.hp[i])
                + weapon.accuracy as i32
                - (soldiers.fatigue[i] as i32 * 40 / 100)
                + flank_bonus
                + Rng::stream(world_seed, i as u32, Purpose::HitRoll, tick).range(-30, 31);
            let defense_roll = injury_skill(defender_attrs.skill, soldiers.hp[defender])
                + defense_stance(defender_attrs)
                + self.armors[defender].shield_block as i32
                - (soldiers.fatigue[defender] as i32 * 50 / 100)
                - (self.engaged_count[defender].saturating_sub(1) as i32 * 25)
                - if soldiers.hot.state[defender] == State::Broken {
                    100
                } else {
                    0
                }
                // 倒れた相手はほとんど受けられない。密集した戦列で転ぶことが
                // 致命的なのは中世会戦の現実そのもの
                - if soldiers.is_stumbling(defender) { 90 } else { 0 }
                // 盾を構えて足を止めている相手は崩しにくい
                + if soldiers.is_braced(defender) {
                    BRACED_DEFENSE_BONUS
                } else {
                    0
                }
                + Rng::stream(world_seed, defender as u32, Purpose::HitRoll, tick).range(-30, 31);

            if attack_roll <= defense_roll {
                continue;
            }

            self.stats.hits = self.stats.hits.saturating_add(1);
            let mut location_rng = Rng::stream(world_seed, i as u32, Purpose::HitLocation, tick);
            let location = hit_location(&mut location_rng);
            let amount = damage_amount(
                weapon,
                attacker_attrs,
                self.armors[defender],
                location,
                soldiers.fatigue[i],
                self.phase == BattlePhase::Pursuit,
            );
            let mut damage_rng = Rng::stream(world_seed, i as u32, Purpose::DamageRoll, tick);
            let instant_death = location == HitLocation::Head
                && matches!(weapon.damage_type, DamageType::Pierce | DamageType::Blunt)
                && amount >= 20
                && damage_rng.chance_permille(80 + (amount as u32).min(120));
            let cause = if self.phase == BattlePhase::Pursuit {
                DeathCause::Pursuit
            } else {
                DeathCause::Melee
            };
            self.push_event(CombatEvent {
                tick,
                attacker: i as SoldierId,
                defender: self.targets[i],
                kind: CombatEventKind::Hit,
                cause,
            });
            self.intents.push(DamageIntent {
                attacker: i as SoldierId,
                defender: self.targets[i],
                amount,
                location,
                instant_death,
                cause,
            });
        }

        self.resolve_pending_shots(world_seed, tick, soldiers, hash);

        for index in 0..self.intents.len() {
            let intent = self.intents[index];
            self.apply_damage(intent, soldiers, tick);
        }
        self.intents.clear();
        self.update_morale(world_seed, tick, soldiers, hash);
    }

    /// 飛翔中の矢・ボルトを進め、着弾したものを解決する。着弾点付近に実際に
    /// 立っている兵士（味方も含む）から命中相手を選ぶことで、面積命中と
    /// 味方誤射を同じ仕組みで表現する（仕様 12 章 M4「射撃」）。
    fn resolve_pending_shots(
        &mut self,
        world_seed: u64,
        tick: u32,
        soldiers: &mut Soldiers,
        hash: &SpatialHash,
    ) {
        let mut index = 0;
        while index < self.pending_shots.len() {
            if self.pending_shots[index].remaining_ticks > 0 {
                self.pending_shots[index].remaining_ticks -= 1;
                index += 1;
                continue;
            }
            let shot = self.pending_shots.swap_remove(index);

            let mut buf = [0u32; MAX_NEIGHBORS];
            let count = hash.query_neighbors(shot.aim_point.x, shot.aim_point.y, &mut buf);
            let mut candidates: Vec<(i64, u32)> = buf[..count]
                .iter()
                .copied()
                .filter(|&id| {
                    id != shot.attacker
                        && soldiers
                            .index_if_present(id)
                            .is_some_and(|j| is_targetable(soldiers.hot.state[j]))
                })
                .map(|id| (dist_sq(shot.aim_point, soldiers.pos(id as usize)), id))
                .collect();
            candidates.sort_unstable_by_key(|&(d, id)| (d, id));

            let base_accuracy =
                injury_skill(shot.attacker_skill, MAX_HP) + shot.weapon.accuracy as i32;
            let range_penalty = shot.range_permille as i32 / 4;
            let hit_chance = (500 + base_accuracy * 4 - range_penalty).clamp(50, 950) as u32;
            let mut rng = Rng::stream(world_seed, shot.attacker, Purpose::ArrowSpread, tick);
            if candidates.is_empty() || !rng.chance_permille(hit_chance) {
                continue;
            }
            // 当たった相手を選ぶ。狙った相手が着弾点にまだ立っていれば、
            // それが当たった相手。
            //
            // 近傍集合（`query_neighbors`）は密集地では上限
            // （[`MAX_NEIGHBORS`]）で先着順に切れるので、狙った相手が
            // そこに入っている保証がない（`query_enemies` の注記と同じ
            // 事情）。集合だけを見て「一番近い者」に当てると、白兵の塊へ
            // 射ち込んだ矢がほぼ毎回手前の味方に当たり、密集した射撃部隊が
            // 自分の前列を撃ち崩してしまう。狙った相手を直接確かめることで
            // これを避ける。誤射（流れ矢）は、狙った相手がその場を離れた
            // ときに残る。
            let target_still_at_aim = soldiers
                .index_if_present(shot.target)
                .filter(|&j| is_targetable(soldiers.hot.state[j]))
                .is_some_and(|j| {
                    dist_sq(shot.aim_point, soldiers.pos(j)) <= AIM_POINT_TOLERANCE_SQ
                });
            let shooter_faction = soldiers.faction.get(shot.attacker as usize).copied();
            let hit_id = if target_still_at_aim {
                shot.target
            } else {
                candidates
                    .iter()
                    // 狙いを外れた矢も、着弾点に敵がいるなら味方より先に敵へ当たる。
                    .find(|&&(_, id)| {
                        shooter_faction.is_some_and(|f| soldiers.faction[id as usize] != f)
                    })
                    .map(|&(_, id)| id)
                    .unwrap_or(candidates[0].1)
            };
            let hit_idx = hit_id as usize;

            let location = hit_location(&mut rng);
            let synth_attrs = Attrs::new(0, 0, 0, shot.attacker_strength, 0, 0, 0, 0, 0, 0, 0, 0);
            let mut amount = damage_amount(
                shot.weapon,
                synth_attrs,
                self.armors[hit_idx],
                location,
                0,
                false,
            ) as i32;
            // 装甲貫通の距離減衰: 最大射程付近では威力が 6 割まで落ちる。
            let falloff = (1000 - shot.range_permille as i32 * 400 / 1000).max(400);
            amount = (amount * falloff / 1000).max(1);
            let amount = amount as u16;

            let attacker_faction = soldiers.faction.get(shot.attacker as usize).copied();
            let is_friendly = attacker_faction.is_some_and(|f| f == soldiers.faction[hit_idx]);
            if is_friendly {
                self.stats.friendly_fire_hits = self.stats.friendly_fire_hits.saturating_add(1);
            }
            self.stats.hits = self.stats.hits.saturating_add(1);
            self.push_event(CombatEvent {
                tick,
                attacker: shot.attacker,
                defender: hit_id,
                kind: CombatEventKind::Hit,
                cause: DeathCause::Missile,
            });
            self.stats.damage = self.stats.damage.saturating_add(amount as u32);
            self.apply_raw_damage(
                hit_id,
                amount,
                soldiers,
                Some(shot.attacker),
                false,
                DeathCause::Missile,
                tick,
            );
        }
    }

    fn apply_bleeding(&mut self, tick: u32, soldiers: &mut Soldiers) {
        if tick % sim_math::TICK_HZ != 0 {
            return;
        }
        for i in 0..soldiers.len() {
            if soldiers.hot.state[i] == State::Dead {
                continue;
            }
            let stage = InjuryStage::from_hp(soldiers.hp[i]);
            let amount = match stage {
                InjuryStage::Medium => 1,
                InjuryStage::Heavy => 2,
                _ => 0,
            };
            if amount != 0 {
                self.apply_raw_damage(
                    i as SoldierId,
                    amount,
                    soldiers,
                    None,
                    false,
                    DeathCause::Bleed,
                    tick,
                );
            }
        }
    }

    fn apply_damage(&mut self, intent: DamageIntent, soldiers: &mut Soldiers, tick: u32) {
        self.stats.damage = self.stats.damage.saturating_add(intent.amount as u32);
        self.apply_raw_damage(
            intent.defender,
            intent.amount,
            soldiers,
            Some(intent.attacker),
            intent.instant_death,
            intent.cause,
            tick,
        );
        let _ = intent.location;
    }

    /// 圧迫（crush）による負傷を適用する。密集しすぎた集団の中で押し潰される
    /// 兵士を表す。`pressure_permille` は押し戻し量から呼び出し側が計算する
    /// 圧力の目安で、閾値（[`CRUSH_THRESHOLD_PERMILLE`]）を超えた分だけが実害になる
    /// （仕様 12 章 M4「圧迫」、`lib.rs::resolve_collisions` から呼ばれる）。
    pub fn apply_crush(
        &mut self,
        id: SoldierId,
        soldiers: &mut Soldiers,
        pressure_permille: u32,
        world_seed: u64,
        tick: u32,
    ) {
        let i = id as usize;
        if i >= soldiers.len()
            || !soldiers.is_alive(i)
            || pressure_permille <= CRUSH_THRESHOLD_PERMILLE
        {
            return;
        }
        let excess = pressure_permille - CRUSH_THRESHOLD_PERMILLE;
        let mut rng = Rng::stream(world_seed, id, Purpose::Crush, tick);
        if !rng.chance_permille((excess / 3).min(1000)) {
            return;
        }
        let amount = (excess / 60).clamp(1, 15) as u16;
        self.stats.damage = self.stats.damage.saturating_add(amount as u32);
        self.apply_raw_damage(id, amount, soldiers, None, false, DeathCause::Crush, tick);
        soldiers.fatigue[i] = soldiers.fatigue[i]
            .saturating_add((excess / 2) as u16)
            .min(MAX_FATIGUE);
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_raw_damage(
        &mut self,
        defender: SoldierId,
        amount: u16,
        soldiers: &mut Soldiers,
        attacker: Option<SoldierId>,
        instant_death: bool,
        cause: DeathCause,
        tick: u32,
    ) {
        let i = defender as usize;
        if i >= soldiers.len() || soldiers.hot.state[i] == State::Dead {
            return;
        }
        let before = InjuryStage::from_hp(soldiers.hp[i]);
        let was_broken = soldiers.hot.state[i] == State::Broken;
        let old_hp = soldiers.hp[i];
        soldiers.hp[i] = if instant_death {
            0
        } else {
            old_hp.saturating_sub(amount).min(MAX_HP)
        };
        let after = InjuryStage::from_hp(soldiers.hp[i]);
        if after != before {
            let penalty = match after {
                InjuryStage::Medium => 60,
                InjuryStage::Heavy => 120,
                InjuryStage::Downed | InjuryStage::Dead => 200,
                InjuryStage::Light => 0,
            };
            soldiers.morale[i] = soldiers.morale[i].saturating_sub(penalty as u16);
        }
        match after {
            InjuryStage::Dead => {
                if before != InjuryStage::Dead {
                    self.stats.kills = self.stats.kills.saturating_add(1);
                    let effective_cause = if self.phase == BattlePhase::Pursuit || was_broken {
                        DeathCause::Pursuit
                    } else {
                        cause
                    };
                    match effective_cause {
                        DeathCause::Melee => {
                            self.stats.melee_kills = self.stats.melee_kills.saturating_add(1)
                        }
                        DeathCause::Missile => {
                            self.stats.missile_kills = self.stats.missile_kills.saturating_add(1)
                        }
                        DeathCause::Crush => {
                            self.stats.crush_kills = self.stats.crush_kills.saturating_add(1)
                        }
                        DeathCause::Bleed => {
                            self.stats.bleed_kills = self.stats.bleed_kills.saturating_add(1)
                        }
                        DeathCause::Pursuit => {
                            self.stats.pursuit_kills = self.stats.pursuit_kills.saturating_add(1)
                        }
                        DeathCause::Charge => {
                            self.stats.charge_kills = self.stats.charge_kills.saturating_add(1)
                        }
                    }
                    self.push_event(CombatEvent {
                        tick,
                        attacker: attacker.unwrap_or(NO_ID),
                        defender,
                        kind: CombatEventKind::Killed,
                        cause: effective_cause,
                    });
                }
                soldiers.hot.state[i] = State::Dead;
                soldiers.target[i] = NO_ID;
            }
            InjuryStage::Downed => {
                if soldiers.hot.state[i] != State::Downed {
                    self.stats.downed = self.stats.downed.saturating_add(1);
                    self.push_event(CombatEvent {
                        tick,
                        attacker: attacker.unwrap_or(NO_ID),
                        defender,
                        kind: CombatEventKind::Downed,
                        cause,
                    });
                }
                soldiers.hot.state[i] = State::Downed;
                soldiers.target[i] = NO_ID;
            }
            _ => {}
        }
        // 馬への被害（仕様 12 章 M5「馬と落馬」）。騎乗中で、まだ死んでいない
        // 場合のみ。馬体力が尽きたらその場で落馬させる。
        if soldiers.hot.state[i] != State::Dead
            && soldiers.hot.flags[i] & flags::MOUNTED != 0
            && self.horse_hp.get(i).copied().unwrap_or(0) > 0
        {
            let horse_damage = ((amount as u32 * HORSE_DAMAGE_SHARE_PERMILLE) / 1000).max(1) as u16;
            self.horse_hp[i] = self.horse_hp[i].saturating_sub(horse_damage);
            if self.horse_hp[i] == 0 {
                self.dismount(defender, soldiers, tick);
            }
        }
        if let Some(attacker) = attacker {
            let a = attacker as usize;
            if a < soldiers.len() {
                soldiers.fatigue[a] = soldiers.fatigue[a].saturating_add(40).min(MAX_FATIGUE);
            }
            soldiers.morale[i] = soldiers.morale[i].min(MAX_MORALE);
        }
    }

    /// 馬が力尽きたときの落馬処理。`MOUNTED` フラグを外し、落下ダメージを
    /// 騎手に与える（生き延びれば徒歩の重装兵として戦い続ける）。
    fn dismount(&mut self, id: SoldierId, soldiers: &mut Soldiers, tick: u32) {
        let i = id as usize;
        if i >= soldiers.len() || soldiers.hot.flags[i] & flags::MOUNTED == 0 {
            return;
        }
        soldiers.hot.flags[i] &= !flags::MOUNTED;
        self.horse_hp[i] = 0;
        self.stats.dismounts = self.stats.dismounts.saturating_add(1);
        self.push_event(CombatEvent {
            tick,
            attacker: NO_ID,
            defender: id,
            kind: CombatEventKind::Dismounted,
            cause: DeathCause::Melee,
        });
        self.apply_raw_damage(
            id,
            DISMOUNT_FALL_DAMAGE,
            soldiers,
            None,
            false,
            DeathCause::Melee,
            tick,
        );
    }

    fn update_morale(
        &mut self,
        world_seed: u64,
        tick: u32,
        soldiers: &mut Soldiers,
        hash: &SpatialHash,
    ) {
        let n = soldiers.len();
        let mut neighbors = [0u32; MAX_NEIGHBORS];
        let mut next_morale = soldiers.morale.clone();

        // このループは next_morale だけでなく soldiers 側の複数の並列配列
        // （hot.state, attrs, target, fatigue など）も同じ添字 i で読む SoA
        // アクセスなので、next_morale だけを iter_mut().enumerate() に
        // 置き換えても i の使用は消えない。
        #[allow(clippy::needless_range_loop)]
        for i in 0..n {
            if soldiers.hot.state[i] == State::Dead || soldiers.hot.state[i] == State::Downed {
                continue;
            }
            if soldiers.hot.state[i] == State::Broken {
                self.broken_ticks[i] = self.broken_ticks[i].saturating_add(1);
                if self.try_rally(world_seed, tick, i, soldiers, hash, &mut neighbors) {
                    continue;
                }
            }

            let count =
                hash.query_neighbors(soldiers.hot.pos_x[i], soldiers.hot.pos_y[i], &mut neighbors);
            let mut same_morale = 0i32;
            let mut same_count = 0i32;
            let mut local_broken = 0i32;
            let mut local_dead = 0i32;
            let mut enemy_broken = 0i32;
            for &id in &neighbors[..count] {
                let j = id as usize;
                if j == i {
                    continue;
                }
                if soldiers.faction[j] == soldiers.faction[i] {
                    if soldiers.hot.state[j] != State::Dead {
                        same_morale += soldiers.morale[j] as i32;
                        same_count += 1;
                    }
                    if soldiers.hot.state[j] == State::Broken {
                        local_broken += 1;
                    }
                    if matches!(soldiers.hot.state[j], State::Downed | State::Dead) {
                        local_dead += 1;
                    }
                } else if soldiers.hot.state[j] == State::Broken {
                    enemy_broken += 1;
                }
            }

            let mut morale = soldiers.morale[i] as i32;
            if same_count != 0 {
                let average = same_morale / same_count;
                let susceptibility = 255 - soldiers.attrs[i].composure as i32;
                morale += (average - morale) * susceptibility / 2048;
            }
            if local_broken >= 3 {
                let susceptibility = 255 - soldiers.attrs[i].composure as i32;
                morale -= local_broken * local_broken * 6 * susceptibility / 255;
            }
            morale -= local_dead * 8;
            morale += enemy_broken * 8;
            morale -= soldiers.fatigue[i] as i32 / 5000;
            // 白兵の間合いで組み合っている圧力。射撃兵が遠くの敵を狙っている
            // だけの状態はこれに当たらない——撃ち返されているとは限らないし、
            // 撃つ側の士気が撃つほど下がるのは実態と逆になる。この区別が無いと、
            // 射程 120 m の長弓兵は接触の何分も前から士気を削られ、一度も
            // 撃たれないまま崩れる。
            if soldiers.target[i] != NO_ID && !self.weapons[i].ranged {
                morale -= 25.min(morale.max(0));
            }
            self.morale_pressure[i] = u8::from(
                soldiers.target[i] != NO_ID
                    || local_broken > 0
                    || local_dead > 0
                    || enemy_broken > 0,
            );
            next_morale[i] = morale.clamp(0, MAX_MORALE as i32) as u16;
        }

        // 同じ理由（上のループ参照）で soldiers 側の複数配列も i で読み書きする
        #[allow(clippy::needless_range_loop)]
        for i in 0..n {
            if soldiers.hot.state[i] == State::Dead || soldiers.hot.state[i] == State::Downed {
                continue;
            }
            soldiers.morale[i] = next_morale[i];
            match soldiers.hot.state[i] {
                State::Broken => {}
                State::Rallying => {
                    if self.rally_ticks[i] > 0 {
                        self.rally_ticks[i] -= 1;
                    }
                    if self.rally_ticks[i] == 0 {
                        soldiers.hot.state[i] = State::Wavering;
                        soldiers.morale[i] = soldiers.morale[i].max(300);
                        self.push_event(CombatEvent {
                            tick,
                            attacker: NO_ID,
                            defender: i as SoldierId,
                            kind: CombatEventKind::Rallied,
                            cause: DeathCause::Melee,
                        });
                    }
                }
                _ if self.morale_pressure[i] != 0 && soldiers.morale[i] <= 250 => {
                    let pressure = (250 - soldiers.morale[i] as i32) * 4;
                    let resistance = soldiers.attrs[i].bravery as i32;
                    let chance = (100 + pressure - resistance / 2).clamp(0, 1000) as u32;
                    let mut rng = Rng::stream(world_seed, i as u32, Purpose::MoraleCheck, tick);
                    if soldiers.morale[i] <= 80 || rng.chance_permille(chance) {
                        soldiers.hot.state[i] = State::Broken;
                        self.broken_ticks[i] = 0;
                        soldiers.target[i] = NO_ID;
                        self.push_event(CombatEvent {
                            tick,
                            attacker: NO_ID,
                            defender: i as SoldierId,
                            kind: CombatEventKind::Broken,
                            cause: DeathCause::Melee,
                        });
                    }
                }
                _ if self.morale_pressure[i] != 0 && soldiers.morale[i] <= 400 => {
                    soldiers.hot.state[i] = State::Wavering;
                }
                _ if soldiers.hot.state[i] == State::Wavering => {
                    soldiers.hot.state[i] = State::Idle;
                }
                _ => {}
            }
        }
    }

    fn try_rally(
        &mut self,
        world_seed: u64,
        tick: u32,
        i: usize,
        soldiers: &mut Soldiers,
        hash: &SpatialHash,
        neighbors: &mut [u32; MAX_NEIGHBORS],
    ) -> bool {
        if self.broken_ticks[i] < sim_math::TICK_HZ as u16 {
            return false;
        }
        let count = hash.query_neighbors(soldiers.hot.pos_x[i], soldiers.hot.pos_y[i], neighbors);
        let mut nearest_enemy = i64::MAX;
        for &id in &neighbors[..count] {
            let j = id as usize;
            if soldiers.faction[j] != soldiers.faction[i] && is_targetable(soldiers.hot.state[j]) {
                nearest_enemy = nearest_enemy.min(dist_sq(soldiers.pos(i), soldiers.pos(j)));
            }
        }
        // SpatialHash は近傍しか返さないため、敵が見えない場合を安全距離として扱う。
        let far_from_enemy =
            nearest_enemy == i64::MAX || nearest_enemy > (fx_from_mm(100_000) as i64).pow(2);
        let mut chance =
            20 + soldiers.attrs[i].bravery as i32 / 2 + soldiers.attrs[i].discipline as i32 / 2
                - (self.broken_ticks[i] as i32 / 10)
                - soldiers.fatigue[i] as i32 / 100;
        if far_from_enemy {
            chance += 40;
        }
        let mut rng = Rng::stream(world_seed, i as u32, Purpose::RallyCheck, tick);
        if rng.chance_permille(chance.clamp(0, 1000) as u32) {
            soldiers.hot.state[i] = State::Rallying;
            self.rally_ticks[i] = 20;
            soldiers.morale[i] = soldiers.morale[i].max(300);
            true
        } else {
            false
        }
    }

    fn update_phase(&mut self, soldiers: &Soldiers) {
        if self.phase == BattlePhase::Complete {
            return;
        }
        let mut faction_a = None;
        let mut faction_b = None;
        let mut active_a = 0u32;
        let mut active_b = 0u32;
        let mut broken_a = 0u32;
        let mut broken_b = 0u32;
        for i in 0..soldiers.len() {
            if soldiers.hot.state[i] == State::Dead {
                continue;
            }
            match faction_a {
                None => faction_a = Some(soldiers.faction[i]),
                Some(f) if f != soldiers.faction[i] && faction_b.is_none() => {
                    faction_b = Some(soldiers.faction[i])
                }
                _ => {}
            }
            if Some(soldiers.faction[i]) == faction_a {
                active_a += 1;
                broken_a += u32::from(soldiers.hot.state[i] == State::Broken);
            } else if Some(soldiers.faction[i]) == faction_b {
                active_b += 1;
                broken_b += u32::from(soldiers.hot.state[i] == State::Broken);
            }
        }
        if faction_b.is_none() {
            return;
        }
        if active_a == 0 || active_b == 0 {
            self.phase = BattlePhase::Complete;
        } else if broken_a * 100 >= active_a * 40 || broken_b * 100 >= active_b * 40 {
            self.phase = BattlePhase::Pursuit;
        } else {
            // 崩れた兵が rally で持ち直せば追撃フェーズから引き返す。片道の
            // ラチェットのままだと、緒戦の局所的な崩れで一度 Pursuit に入った
            // 後、双方が rally で Broken でなくなっても Pursuit のままに固定され、
            // 交戦相手探索が Broken 限定のままなので誰も再交戦できず、
            // 大規模会戦が白兵戦ゼロで停止する（issue #5）。
            self.phase = BattlePhase::Battle;
        }
    }

    /// 戦闘専用の状態をワールドハッシュへ含める。
    pub fn state_hash(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut mix = |value: u64| {
            h ^= value;
            h = h.wrapping_mul(0x0100_0000_01b3);
        };
        mix(self.phase as u64);
        mix(self.stats.attacks as u64);
        mix(self.stats.hits as u64);
        mix(self.stats.damage as u64);
        mix(self.stats.kills as u64);
        mix(self.stats.downed as u64);
        mix(self.stats.melee_kills as u64);
        mix(self.stats.missile_kills as u64);
        mix(self.stats.crush_kills as u64);
        mix(self.stats.bleed_kills as u64);
        mix(self.stats.pursuit_kills as u64);
        mix(self.stats.shots_fired as u64);
        mix(self.stats.friendly_fire_hits as u64);
        mix(self.stats.charge_kills as u64);
        mix(self.stats.dismounts as u64);
        mix(self.stats.horse_refusals as u64);
        mix(self.pending_shots.len() as u64);
        for i in 0..self.weapons.len() {
            let weapon = self.weapons[i];
            let armor = self.armors[i];
            mix(weapon.reach as u32 as u64);
            mix(weapon.base_swing_ms as u64);
            mix(weapon.accuracy as u16 as u64);
            mix(weapon.power as u64);
            mix(weapon.damage_type as u64);
            mix(u64::from(weapon.requires_front_row));
            mix(u64::from(weapon.ranged));
            mix(armor.class as u64);
            mix(armor.head as u64);
            mix(armor.torso as u64);
            mix(armor.arms as u64);
            mix(armor.legs as u64);
            mix(armor.coverage_permille as u64);
            mix(armor.quality_permille as u64);
            mix(armor.shield_block as u16 as u64);
            mix(self.attack_timer[i] as u64);
            mix(self.broken_ticks[i] as u64);
            mix(self.rally_ticks[i] as u64);
            mix(self.ammo[i] as u64);
            mix(self.horse_hp[i] as u64);
            if let Some(&target) = self.targets.get(i) {
                mix(target as u64);
            }
        }
        h
    }
}

fn is_targetable(state: State) -> bool {
    state != State::Dead
}

fn is_front_row(
    i: usize,
    pos: Vec2Fx,
    soldiers: &Soldiers,
    hash: &SpatialHash,
    neighbors: &mut [u32; MAX_NEIGHBORS],
) -> bool {
    let count = hash.query_neighbors(pos.x, pos.y, neighbors);
    let front_distance = fx_from_mm(800);
    for &id in &neighbors[..count] {
        let j = id as usize;
        if j == i || !soldiers.is_alive(j) || soldiers.faction[j] != soldiers.faction[i] {
            continue;
        }
        if dist_sq(pos, soldiers.pos(j)) <= (front_distance as i64).pow(2)
            && within_arc(soldiers.hot.facing[i], pos, soldiers.pos(j), 5_461)
        {
            return false;
        }
    }
    true
}

fn flank_bonus(defender_facing: Brad, defender_pos: Vec2Fx, attacker_pos: Vec2Fx) -> i32 {
    let direction = attacker_pos.sub(defender_pos).angle();
    let degrees = angle_diff(direction, defender_facing).unsigned_abs();
    if degrees > brad_from_deg(120) as u32 {
        80
    } else if degrees > brad_from_deg(60) as u32 {
        40
    } else {
        0
    }
}

fn defense_stance(attrs: Attrs) -> i32 {
    // self_preservation が高い兵士は防御姿勢を取りやすい。攻撃機会を
    // 減らす細かな AI は後続スコープで追加し、ここでは防御値だけ反映する。
    attrs.self_preservation as i32 / 8
}

fn injury_skill(skill: u8, hp: u16) -> i32 {
    let multiplier = match InjuryStage::from_hp(hp) {
        InjuryStage::Light => 900,
        InjuryStage::Medium => 750,
        InjuryStage::Heavy => 500,
        InjuryStage::Downed | InjuryStage::Dead => 0,
    };
    skill as i32 * multiplier / 1000
}

fn swing_ticks(weapon: Weapon, attrs: Attrs, fatigue: u16, state: State, braced: bool) -> u16 {
    // skill_factor・fatigue_factor・crowding_factor はいずれもパーミル
    // （1000 = ×1.0）で、3 つ掛け合わせると 1000^3 = 1e9 倍になる。
    // 1e6 で割ると 1000 倍大きい ms が残り、剣の 1 振り約 2 秒のはずが
    // 約 2000 秒（33 分）になって白兵戦がほぼ発生しなくなる（issue #5）。
    let skill_factor = (2000 - attrs.skill as i32 * 4).max(980) as i64;
    let fatigue_factor = if fatigue > 6000 { 1400 } else { 1000 };
    let crowding_factor = if state == State::Wavering { 1200 } else { 1000 };
    // 盾を構えて受け止める姿勢は守りに寄っている。攻撃の手数は落ちる
    // （仕様 06 章 3.8 節）
    let stance_factor: i64 = if braced { 1300 } else { 1000 };
    let ms = weapon.base_swing_ms as i64
        * skill_factor
        * fatigue_factor as i64
        * crowding_factor as i64
        * stance_factor
        / 1_000_000_000_000;
    ms_to_ticks(ms.max(50).min(u32::MAX as i64) as u32).max(1) as u16
}

fn hit_location(rng: &mut Rng) -> HitLocation {
    match rng.range(0, 100) {
        0..=11 => HitLocation::Head,
        12..=56 => HitLocation::Torso,
        57..=79 => HitLocation::Arms,
        _ => HitLocation::Legs,
    }
}

/// 矢が「狙ったところに落ちた」とみなす許容半径（mm）。着弾点からこの距離に
/// 標的がまだ立っていれば、当たったのはその標的。
const AIM_POINT_TOLERANCE_MM: i32 = 1_000;
const AIM_POINT_TOLERANCE_SQ: i64 =
    (fx_from_mm(AIM_POINT_TOLERANCE_MM) as i64) * (fx_from_mm(AIM_POINT_TOLERANCE_MM) as i64);

fn type_factor(damage_type: DamageType, armor: ArmorClass) -> i32 {
    match (damage_type, armor) {
        (DamageType::Cut, ArmorClass::ClothLeather) => 1000,
        (DamageType::Cut, ArmorClass::Mail) => 350,
        (DamageType::Cut, ArmorClass::Plate) => 100,
        (DamageType::Pierce, ArmorClass::ClothLeather) => 900,
        (DamageType::Pierce, ArmorClass::Mail) => 650,
        (DamageType::Pierce, ArmorClass::Plate) => 300,
        (DamageType::Blunt, ArmorClass::ClothLeather | ArmorClass::Plate) => 700,
        (DamageType::Blunt, ArmorClass::Mail) => 850,
        (DamageType::Missile, ArmorClass::ClothLeather) => 900,
        (DamageType::Missile, ArmorClass::Mail) => 500,
        (DamageType::Missile, ArmorClass::Plate) => 250,
    }
}

fn damage_amount(
    weapon: Weapon,
    attrs: Attrs,
    armor: Armor,
    location: HitLocation,
    fatigue: u16,
    pursuit: bool,
) -> u16 {
    let strength_factor = attrs.strength as i32 + 128;
    let mut damage = weapon.power as i32 * strength_factor / 256;
    damage = damage * type_factor(weapon.damage_type, armor.class) / 1000;
    let armor_factor = 1000 - (armor.armor_at(location) as i32 * 2).min(300);
    damage = damage * armor_factor / 1000;
    let coverage_factor =
        1000 - (armor.coverage_permille as i32 * armor.quality_permille as i32 / 1000);
    damage = damage * coverage_factor / 1000;
    damage = damage * (1000 - (fatigue as i32 * 300 / MAX_FATIGUE as i32)) / 1000;
    if pursuit {
        damage = damage * 1300 / 1000;
    }
    damage.max(1).min(MAX_HP as i32) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soldiers::Attrs;
    use sim_math::{brad_from_deg, fx, fx_from_mm};

    fn attrs(skill: u8, bravery: u8) -> Attrs {
        Attrs::new(
            120, 120, 140, 180, 140, skill, bravery, 180, 120, 120, 160, 180,
        )
    }

    fn soldiers_pair() -> Soldiers {
        let mut soldiers = Soldiers::default();
        soldiers.push(fx(10), fx(10), brad_from_deg(0), 0, 0, attrs(220, 220), 0);
        soldiers.push(fx(11), fx(10), brad_from_deg(180), 0, 1, attrs(20, 20), 0);
        soldiers
    }

    #[test]
    fn weapon_and_armor_matchups_are_distinct() {
        let strong = attrs(220, 220);
        let cut_mail = damage_amount(
            Weapon::sword(),
            strong,
            Armor::mail(),
            HitLocation::Torso,
            0,
            false,
        );
        let blunt_mail = damage_amount(
            Weapon::mace(),
            strong,
            Armor::mail(),
            HitLocation::Torso,
            0,
            false,
        );
        let cut_plate = damage_amount(
            Weapon::sword(),
            strong,
            Armor::plate(),
            HitLocation::Torso,
            0,
            false,
        );
        assert!(blunt_mail > cut_mail);
        assert!(cut_mail > cut_plate);
    }

    #[test]
    fn engagement_requires_front_arc_and_front_row() {
        let mut soldiers = soldiers_pair();
        let mut hash = SpatialHash::default();
        hash.rebuild(&soldiers);
        let mut combat = CombatSystem::default();
        combat.ensure_len(soldiers.len());
        combat.tick(1, 0, &mut soldiers, &hash);
        assert_eq!(soldiers.target[0], 1);
        assert_eq!(soldiers.target[1], 0);

        soldiers.push(
            fx_from_mm(10_700),
            fx(10),
            brad_from_deg(0),
            0,
            0,
            attrs(220, 220),
            0,
        );
        hash.rebuild(&soldiers);
        combat.tick(1, 1, &mut soldiers, &hash);
        // ID 2 が ID 0 の前方を塞ぐため、ID 0 は後列になり攻撃しない。
        assert_eq!(soldiers.target[0], NO_ID);
        assert_eq!(soldiers.target[2], 1);
        assert!(combat.stats.attacks >= 1);
    }

    #[test]
    fn low_hp_transitions_to_downed_and_dead() {
        let mut soldiers = Soldiers::default();
        soldiers.push(0, 0, 0, 0, 0, attrs(200, 200), 0);
        let mut combat = CombatSystem::default();
        combat.ensure_len(1);
        combat.apply_raw_damage(0, 59, &mut soldiers, None, false, DeathCause::Melee, 0);
        assert_eq!(InjuryStage::from_hp(soldiers.hp[0]), InjuryStage::Medium);
        assert_eq!(InjuryStage::from_hp(30), InjuryStage::Heavy);
        combat.apply_raw_damage(0, 30, &mut soldiers, None, false, DeathCause::Melee, 0);
        assert_eq!(soldiers.hot.state[0], State::Downed);
        combat.apply_raw_damage(0, 20, &mut soldiers, None, false, DeathCause::Melee, 0);
        assert_eq!(soldiers.hot.state[0], State::Dead);
    }

    #[test]
    fn morale_can_break_and_rally_deterministically() {
        let mut soldiers = soldiers_pair();
        soldiers.morale[0] = 0;
        soldiers.hot.state[0] = State::Wavering;
        let mut hash = SpatialHash::default();
        hash.rebuild(&soldiers);
        let mut combat = CombatSystem::default();
        combat.ensure_len(soldiers.len());
        combat.tick(9, 0, &mut soldiers, &hash);
        assert_eq!(soldiers.hot.state[0], State::Broken);

        combat.broken_ticks[0] = sim_math::TICK_HZ as u16;
        soldiers.morale[0] = 400;
        combat.tick(9, 1, &mut soldiers, &hash);
        assert!(matches!(
            soldiers.hot.state[0],
            State::Rallying | State::Broken
        ));
    }

    #[test]
    fn same_seed_produces_same_combat_hash() {
        let run = || {
            let mut soldiers = soldiers_pair();
            let mut hash = SpatialHash::default();
            let mut combat = CombatSystem::default();
            combat.ensure_len(soldiers.len());
            let mut values = Vec::new();
            for tick in 0..80 {
                hash.rebuild(&soldiers);
                combat.tick(77, tick, &mut soldiers, &hash);
                values.push((
                    combat.state_hash(),
                    soldiers.hp.clone(),
                    soldiers.morale.clone(),
                ));
            }
            values
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn ranged_weapon_hits_a_distant_target_and_consumes_ammo() {
        let mut soldiers = Soldiers::default();
        // 近接用の空間ハッシュ（3x3 セル = 6 m）では届かない距離に離す。
        soldiers.push(fx(10), fx(10), brad_from_deg(90), 0, 0, attrs(200, 200), 0);
        soldiers.push(fx(10), fx(70), brad_from_deg(270), 0, 1, attrs(20, 20), 0);
        let mut hash = SpatialHash::default();
        let mut combat = CombatSystem::default();
        combat.ensure_len(soldiers.len());
        combat.set_loadout(0, Weapon::longbow(), Armor::cloth());
        let start_ammo = combat.ammo[0];
        let start_hp = soldiers.hp[1];

        for tick in 0..400 {
            hash.rebuild(&soldiers);
            combat.tick(123, tick, &mut soldiers, &hash);
            if combat.ammo[0] < start_ammo {
                break;
            }
        }
        assert!(combat.ammo[0] < start_ammo, "矢が発射されていない");
        assert!(combat.stats.shots_fired >= 1);

        for tick in 400..2000 {
            hash.rebuild(&soldiers);
            combat.tick(123, tick, &mut soldiers, &hash);
            if soldiers.hp[1] < start_hp {
                break;
            }
        }
        assert!(
            soldiers.hp[1] < start_hp || combat.stats.shots_fired > 1,
            "矢が着弾も再発射もしていない"
        );
    }

    /// 遠くの敵を狙っているだけの射撃兵は、白兵の圧力による士気低下を受けない。
    /// 受けてしまうと、射程 120 m の長弓兵は接触の何分も前から士気を削られ、
    /// 一度も撃たれないまま崩れる。
    #[test]
    fn shooting_at_a_distant_enemy_does_not_drain_morale_like_melee_does() {
        let drain = |ranged: bool| {
            let mut soldiers = Soldiers::default();
            let mut combat = CombatSystem::default();
            if ranged {
                // 長弓の射程内・白兵の間合いの外に敵を置く。
                soldiers.push(fx(10), fx(10), brad_from_deg(90), 0, 0, attrs(200, 200), 0);
                soldiers.push(fx(10), fx(70), brad_from_deg(270), 0, 1, attrs(20, 20), 0);
                combat.ensure_len(soldiers.len());
                combat.set_loadout(0, Weapon::longbow(), Armor::cloth());
            } else {
                soldiers = soldiers_pair();
                combat.ensure_len(soldiers.len());
            }
            let mut hash = SpatialHash::default();
            let before = soldiers.morale[0];
            for tick in 0..20 {
                hash.rebuild(&soldiers);
                combat.tick(5, tick, &mut soldiers, &hash);
            }
            assert_eq!(soldiers.target[0], 1, "標的を捕捉していない");
            before as i32 - soldiers.morale[0] as i32
        };
        assert!(drain(false) > 100, "白兵の圧力で士気が下がっていない");
        assert!(drain(true) <= 0, "射撃だけで士気が削られている");
    }

    /// 味方の密集の向こうにいる敵を狙った矢は、狙った相手に当たる。
    ///
    /// 着弾点の近傍集合は上限で先着順に切れるため、ここで標的を直接
    /// 確かめないと、混戦へ射ち込んだ矢が毎回手前の味方に当たり、密集した
    /// 射撃部隊が自分の前列を撃ち崩してしまう。
    #[test]
    fn arrows_reach_the_soldier_they_were_aimed_at_through_a_friendly_crowd() {
        let mut soldiers = Soldiers::default();
        // 射手。
        soldiers.push(fx(10), fx(10), brad_from_deg(90), 0, 0, attrs(220, 220), 0);
        // 標的（敵）。射手から 30 m 先。
        soldiers.push(fx(10), fx(40), brad_from_deg(270), 0, 1, attrs(20, 20), 0);
        // 標的を取り囲む味方の塊。近傍集合の枠を先に埋める。
        for k in 0..MAX_NEIGHBORS as i32 {
            let dx = fx_from_mm(300 * (k % 4 - 2));
            let dy = fx_from_mm(300 * (k / 4 - 1));
            soldiers.push(
                fx(10) + dx,
                fx(40) + dy,
                brad_from_deg(90),
                0,
                0,
                attrs(120, 120),
                0,
            );
        }
        let mut hash = SpatialHash::default();
        let mut combat = CombatSystem::default();
        combat.ensure_len(soldiers.len());
        combat.set_loadout(0, Weapon::longbow(), Armor::cloth());
        for i in 1..soldiers.len() {
            combat.set_loadout(i as SoldierId, Weapon::dagger(), Armor::cloth());
        }

        for tick in 0..600 {
            hash.rebuild(&soldiers);
            combat.tick(31, tick, &mut soldiers, &hash);
        }
        assert!(combat.stats.shots_fired > 0, "矢が発射されていない");
        assert_eq!(
            combat.stats.friendly_fire_hits, 0,
            "狙った敵がその場に立っているのに味方へ当たっている"
        );
    }

    #[test]
    fn horse_death_dismounts_the_rider() {
        let mut soldiers = Soldiers::default();
        soldiers.push(
            0,
            0,
            0,
            0,
            0,
            attrs(120, 200),
            crate::soldiers::flags::MOUNTED,
        );
        let mut combat = CombatSystem::default();
        combat.ensure_len(1);
        combat.set_mounted(0, true);
        assert!(soldiers.hot.flags[0] & crate::soldiers::flags::MOUNTED != 0);
        assert_eq!(combat.horse_hp(0), MAX_HORSE_HP);

        // 馬の体力を使い切るまで打ち続ける。都度、被害の大半は馬に向く。
        // 騎手自身が先に力尽きないよう、都度 HP を保つ（馬の減りだけを見る）。
        for _ in 0..60 {
            if combat.horse_hp(0) == 0 {
                break;
            }
            soldiers.hp[0] = MAX_HP;
            combat.apply_raw_damage(0, 10, &mut soldiers, None, false, DeathCause::Melee, 0);
        }
        assert_eq!(combat.horse_hp(0), 0, "馬の体力が尽きていない");
        assert_eq!(
            soldiers.hot.flags[0] & crate::soldiers::flags::MOUNTED,
            0,
            "落馬後も MOUNTED フラグが残っている"
        );
        assert_eq!(combat.stats.dismounts, 1);
        assert!(combat
            .events
            .iter()
            .any(|e| e.kind == CombatEventKind::Dismounted));
    }

    #[test]
    fn apply_impact_damage_is_tagged_as_charge() {
        let mut soldiers = soldiers_pair();
        let mut combat = CombatSystem::default();
        combat.ensure_len(soldiers.len());
        let hp_before = soldiers.hp[1];
        combat.apply_impact_damage(0, 1, 25, &mut soldiers, 0);
        assert!(soldiers.hp[1] < hp_before);
        assert!(combat.events.iter().any(|e| e.cause == DeathCause::Charge));
    }

    #[test]
    fn crush_only_hurts_above_the_pressure_threshold() {
        let mut soldiers = Soldiers::default();
        soldiers.push(0, 0, 0, 0, 0, attrs(120, 120), 0);
        let mut combat = CombatSystem::default();
        combat.ensure_len(1);

        let hp_before = soldiers.hp[0];
        combat.apply_crush(0, &mut soldiers, CRUSH_THRESHOLD_PERMILLE, 1, 0);
        assert_eq!(
            soldiers.hp[0], hp_before,
            "閾値以下では被害が出てはいけない"
        );

        let fatigue_before = soldiers.fatigue[0];
        // 閾値超過分が大きいほど確実に被害が出るよう、十分大きい圧力で試す。
        combat.apply_crush(0, &mut soldiers, 5000, 1, 0);
        assert!(
            soldiers.hp[0] < hp_before || soldiers.fatigue[0] > fatigue_before,
            "強い圧迫なのに被害も疲労増加もない"
        );
    }
}
