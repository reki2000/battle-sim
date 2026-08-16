//! M4 の戦闘と士気。
//!
//! このモジュールは、兵士の SoA にある HP・士気・状態を戦闘の確定値として
//! 更新する。装備・攻撃タイマー・敗走時間のような M4 固有の配列はここに
//! 置くことで、M3 の指揮ツリーと既存の描画用レイアウトを変更しない。
//!
//! すべての判定は整数で行い、乱数は兵士 ID・目的・tick から導出する。攻撃を
//! ID 順に解決しても、各判定そのものは呼び出し順に依存しない。

use sim_math::{
    angle_diff, brad_from_deg, dist_sq, fx_from_mm, ms_to_ticks, within_arc, Brad, Fx, Purpose,
    Rng, Vec2Fx,
};

use crate::soldiers::{Attrs, SoldierId, Soldiers, State, MAX_FATIGUE, MAX_HP, MAX_MORALE, NO_ID};
use crate::spatial::{SpatialHash, MAX_NEIGHBORS};

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
    /// リーチ（固定小数点の m）。
    pub reach: Fx,
    /// 基本の振り時間。
    pub base_swing_ms: u16,
    /// 命中値への加算。
    pub accuracy: i16,
    /// 筋力補正前の威力。
    pub power: u16,
    pub damage_type: DamageType,
    /// 後列から攻撃できる武器（基礎スコープではパイクだけ）。
    pub requires_front_row: bool,
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
    /// 盾のブロック値。盾そのものは M5 以降で拡張する。
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

/// 戦闘の集計値。UI 用イベントログは M4 後半でこの値をイベント列へ拡張する。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CombatStats {
    pub attacks: u32,
    pub hits: u32,
    pub damage: u32,
    pub kills: u32,
    pub downed: u32,
    pub pursuit_kills: u32,
}

#[derive(Clone, Copy, Debug)]
struct DamageIntent {
    attacker: SoldierId,
    defender: SoldierId,
    amount: u16,
    location: HitLocation,
    instant_death: bool,
}

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
    }

    fn ensure_len(&mut self, len: usize) {
        while self.weapons.len() < len {
            self.register();
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
            let count = hash.query_neighbors(pos.x, pos.y, &mut neighbors);
            let mut best: Option<(i64, SoldierId)> = None;
            for &candidate_id in &neighbors[..count] {
                let j = candidate_id as usize;
                if j == i || !is_targetable(soldiers.hot.state[j]) {
                    continue;
                }
                if soldiers.faction[i] == soldiers.faction[j]
                    || (self.phase == BattlePhase::Pursuit
                        && soldiers.hot.state[j] != State::Broken)
                {
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

        for i in 0..n {
            soldiers.target[i] = self.targets[i];
            if self.targets[i] != NO_ID {
                if !matches!(soldiers.hot.state[i], State::Wavering | State::Broken) {
                    soldiers.hot.state[i] = State::Engaged;
                }
            } else if soldiers.hot.state[i] == State::Engaged {
                soldiers.hot.state[i] = State::Idle;
            }
        }

        for i in 0..n {
            if !soldiers.is_alive(i)
                || matches!(soldiers.hot.state[i], State::Broken | State::Rallying)
                || self.targets[i] == NO_ID
            {
                continue;
            }
            if self.attack_timer[i] > 0 {
                self.attack_timer[i] -= 1;
                continue;
            }

            let defender = self.targets[i] as usize;
            self.stats.attacks = self.stats.attacks.saturating_add(1);
            let attacker_attrs = soldiers.attrs[i];
            let defender_attrs = soldiers.attrs[defender];
            let flank_bonus = flank_bonus(
                soldiers.hot.facing[defender],
                soldiers.pos(defender),
                soldiers.pos(i),
            );
            let attack_roll = injury_skill(attacker_attrs.skill, soldiers.hp[i])
                + self.weapons[i].accuracy as i32
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
                + Rng::stream(world_seed, defender as u32, Purpose::HitRoll, tick).range(-30, 31);

            self.attack_timer[i] = swing_ticks(
                self.weapons[i],
                attacker_attrs,
                soldiers.fatigue[i],
                soldiers.hot.state[i],
            );
            if attack_roll <= defense_roll {
                continue;
            }

            self.stats.hits = self.stats.hits.saturating_add(1);
            let mut location_rng = Rng::stream(world_seed, i as u32, Purpose::HitLocation, tick);
            let location = hit_location(&mut location_rng);
            let amount = damage_amount(
                self.weapons[i],
                attacker_attrs,
                self.armors[defender],
                location,
                soldiers.fatigue[i],
                self.phase == BattlePhase::Pursuit,
            );
            let mut damage_rng = Rng::stream(world_seed, i as u32, Purpose::DamageRoll, tick);
            let instant_death = location == HitLocation::Head
                && matches!(
                    self.weapons[i].damage_type,
                    DamageType::Pierce | DamageType::Blunt
                )
                && amount >= 20
                && damage_rng.chance_permille(80 + (amount as u32).min(120));
            self.intents.push(DamageIntent {
                attacker: i as SoldierId,
                defender: self.targets[i],
                amount,
                location,
                instant_death,
            });
        }

        for index in 0..self.intents.len() {
            let intent = self.intents[index];
            self.apply_damage(intent, soldiers);
        }
        self.intents.clear();
        self.update_morale(world_seed, tick, soldiers, hash);
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
                self.apply_raw_damage(i as SoldierId, amount, soldiers, None, false);
            }
        }
    }

    fn apply_damage(&mut self, intent: DamageIntent, soldiers: &mut Soldiers) {
        self.stats.damage = self.stats.damage.saturating_add(intent.amount as u32);
        self.apply_raw_damage(
            intent.defender,
            intent.amount,
            soldiers,
            Some(intent.attacker),
            intent.instant_death,
        );
        let _ = intent.location;
    }

    fn apply_raw_damage(
        &mut self,
        defender: SoldierId,
        amount: u16,
        soldiers: &mut Soldiers,
        attacker: Option<SoldierId>,
        instant_death: bool,
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
                    if self.phase == BattlePhase::Pursuit || was_broken {
                        self.stats.pursuit_kills = self.stats.pursuit_kills.saturating_add(1);
                    }
                }
                soldiers.hot.state[i] = State::Dead;
                soldiers.target[i] = NO_ID;
            }
            InjuryStage::Downed => {
                if soldiers.hot.state[i] != State::Downed {
                    self.stats.downed = self.stats.downed.saturating_add(1);
                }
                soldiers.hot.state[i] = State::Downed;
                soldiers.target[i] = NO_ID;
            }
            _ => {}
        }
        if let Some(attacker) = attacker {
            let a = attacker as usize;
            if a < soldiers.len() {
                soldiers.fatigue[a] = soldiers.fatigue[a].saturating_add(40).min(MAX_FATIGUE);
            }
            soldiers.morale[i] = soldiers.morale[i].min(MAX_MORALE);
        }
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
            if soldiers.target[i] != NO_ID {
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
        for i in 0..self.weapons.len() {
            let weapon = self.weapons[i];
            let armor = self.armors[i];
            mix(weapon.reach as u32 as u64);
            mix(weapon.base_swing_ms as u64);
            mix(weapon.accuracy as u16 as u64);
            mix(weapon.power as u64);
            mix(weapon.damage_type as u64);
            mix(u64::from(weapon.requires_front_row));
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

fn swing_ticks(weapon: Weapon, attrs: Attrs, fatigue: u16, state: State) -> u16 {
    let skill_factor = (2000 - attrs.skill as i32 * 4).max(980) as i64;
    let fatigue_factor = if fatigue > 6000 { 1400 } else { 1000 };
    let crowding_factor = if state == State::Wavering { 1200 } else { 1000 };
    let ms =
        weapon.base_swing_ms as i64 * skill_factor * fatigue_factor as i64 * crowding_factor as i64
            / 1_000_000;
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

fn type_factor(damage_type: DamageType, armor: ArmorClass) -> i32 {
    match (damage_type, armor) {
        (DamageType::Cut, ArmorClass::ClothLeather) => 1000,
        (DamageType::Cut, ArmorClass::Mail) => 350,
        (DamageType::Cut, ArmorClass::Plate) => 100,
        (DamageType::Pierce, ArmorClass::ClothLeather) => 900,
        (DamageType::Pierce, ArmorClass::Mail) => 650,
        (DamageType::Pierce, ArmorClass::Plate) => 300,
        (DamageType::Blunt, ArmorClass::ClothLeather) => 700,
        (DamageType::Blunt, ArmorClass::Mail) => 850,
        (DamageType::Blunt, ArmorClass::Plate) => 700,
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
        combat.apply_raw_damage(0, 59, &mut soldiers, None, false);
        assert_eq!(InjuryStage::from_hp(soldiers.hp[0]), InjuryStage::Medium);
        assert_eq!(InjuryStage::from_hp(30), InjuryStage::Heavy);
        combat.apply_raw_damage(0, 30, &mut soldiers, None, false);
        assert_eq!(soldiers.hot.state[0], State::Downed);
        combat.apply_raw_damage(0, 20, &mut soldiers, None, false);
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
}
