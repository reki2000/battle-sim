//! M5 の騎兵：突撃のモメンタム、馬の忌避、衝撃、落馬後の徒歩化の土台。
//!
//! 仕様は `docs/spec/06-combat.md` 4 節、`docs/spec/12-roadmap.md` の M5。
//!
//! 馬の体力・落馬処理は既存の被ダメージ経路（負傷段階・士気・死因統計）を
//! そのまま通したいので `combat::CombatSystem` 側に置いた。このモジュールは
//! 「突撃という運動」に固有の状態（モメンタム・馬の疲労・恐怖）と、
//! それが引き起こす一度きりのイベント（忌避・衝撃）だけを持つ。
//!
//! `World::tick` では `combat.tick` より前に呼ぶ。これにより、忌避や衝撃で
//! `State::Wavering` / `State::Engaged` へ落ちた騎兵を、同じ tick 内で
//! `combat.tick` の通常交戦ロジックがそのまま拾える。

use sim_math::{dist_sq, fx_from_mm, fx_scale_permille, within_arc, Fx, Purpose, Rng, Vec2Fx};

use crate::combat::CombatSystem;
use crate::soldiers::{flags, SoldierId, Soldiers, State, NO_ID};
use crate::spatial::{SpatialHash, MAX_NEIGHBORS};
use crate::structures::StructureSystem;

/// 助走距離がこれに達すると、疲労のない馬ではモメンタムが 1000‰（全開）になる。
/// 仕様 06 章 4.1 節「最高速に達するには平地で約 100 m」。
const CHARGE_FULL_DISTANCE_MM: u32 = 100_000;

/// この速度（1 tick あたりの移動量）未満では「走っている」とみなさず、
/// 助走距離を積まない。
const CHARGE_MIN_SPEED_MM_PER_TICK: i32 = 60;

/// 馬の疲労の最大値（歩兵の `MAX_FATIGUE` とは別スケール）。
const MAX_HORSE_FATIGUE: u16 = 10_000;
/// 突撃中の馬の疲労蓄積（tick あたり）。1 回の全力疾走（助走 100 m 前後、
/// 20〜30 秒）ではまだ大きく効かないが、休みなく突撃を繰り返すと
/// すぐ頭打ちになる速さにしてある（仕様「馬の疲労は騎手より早く蓄積する」）。
const HORSE_FATIGUE_CHARGE_RATE: u16 = 10;
/// 突撃していない間の馬の疲労回復（tick あたり）。
const HORSE_FATIGUE_RECOVERY: u16 = 4;

/// モメンタムがこの値に達するまでは、接触しても衝突判定（忌避／衝撃）を
/// 行わない。助走が足りない「なんちゃって突撃」は通常の白兵戦に任せる
/// ——これにより静止・低速の騎兵は歩兵に対して特別な優位を持たない。
const IMPACT_MOMENTUM_MIN_PERMILLE: u16 = 250;

/// 衝撃ダメージの基準値。モメンタム 1000‰ でこの値が満額入る。
const BASE_IMPACT_DAMAGE: i32 = 70;
/// ノックバックの基準距離（モメンタム 1000‰ で全額）。
const KNOCKBACK_BASE_MM: i32 = 1_400;
/// 衝撃を受けた側の士気ダメージ（仕様 06 章 5.2「騎兵突撃を受ける -100」）。
const IMPACT_MORALE_SHOCK: u16 = 120;
/// 忌避時の士気ダメージ。
const REFUSAL_MORALE_SHOCK: u16 = 40;
/// 忌避 1 回あたりの馬の恐怖の増分。
const REFUSAL_FEAR_GAIN: u16 = 180;

/// 前方で「槍衾」を検知する際の間合い（衝突半径の和に加える分）。
const SPEAR_WALL_DETECT_MM: i32 = 3_000;
/// 槍衾とみなす武器のリーチ下限（槍・パイク）。
const SPEAR_REACH_MM: i32 = 2_000;
/// 接触（忌避判定・衝撃）を行う間合い（衝突半径の和に加える分）。
const CONTACT_RANGE_MM: i32 = 1_500;

/// 杭列（仕様 07 章 2 節）を忌避せずに踏み抜いたときの、馬への基準ダメージ。
/// モメンタム 1000‰ でこの値が満額入る。槍衾を突破したときの
/// [`BASE_IMPACT_DAMAGE`] よりずっと大きい（「大ダメージ」）。
const STAKES_IMPACT_DAMAGE: i32 = 150;
/// 杭列を踏み抜いたときに構造物側が受ける耐久ダメージ（「杭が折れる」）。
const STAKES_STRUCTURE_DAMAGE: u16 = 10;

/// M5 の騎兵固有の per-soldier 状態。配列の index は `SoldierId` と一致する。
/// 徒歩の兵士では常に 0。
#[derive(Debug, Default)]
pub struct CavalrySystem {
    /// 突撃のモメンタム（0..1000‰）。`State::Charging` のときだけ育つ。
    momentum_permille: Vec<u16>,
    /// 今回の突撃で走った距離（mm）。モメンタムの元になる。
    charge_run_mm: Vec<u32>,
    /// 馬の疲労（0..10000）。速度低下に使う。
    horse_fatigue: Vec<u16>,
    /// 馬の恐怖（0..1000）。忌避のたびに上がり、時間で下がる。
    horse_fear: Vec<u16>,
}

impl CavalrySystem {
    pub fn register(&mut self) {
        self.momentum_permille.push(0);
        self.charge_run_mm.push(0);
        self.horse_fatigue.push(0);
        self.horse_fear.push(0);
    }

    fn ensure_len(&mut self, len: usize) {
        while self.momentum_permille.len() < len {
            self.register();
        }
    }

    /// 現在の突撃モメンタム（0..1000‰）。
    #[inline]
    pub fn momentum_permille(&self, id: SoldierId) -> u16 {
        self.momentum_permille
            .get(id as usize)
            .copied()
            .unwrap_or(0)
    }

    /// 馬の疲労（0..10000）。
    #[inline]
    pub fn horse_fatigue(&self, id: SoldierId) -> u16 {
        self.horse_fatigue.get(id as usize).copied().unwrap_or(0)
    }

    /// 馬の恐怖（0..1000）。
    #[inline]
    pub fn horse_fear(&self, id: SoldierId) -> u16 {
        self.horse_fear.get(id as usize).copied().unwrap_or(0)
    }

    /// 1 tick 分の突撃モメンタム・忌避・衝撃を解決する。
    ///
    /// `map_limit` はワールドの一辺（Fx）。ノックバックでマップ外へ押し出さない
    /// ようにクランプする。
    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        world_seed: u64,
        tick: u32,
        soldiers: &mut Soldiers,
        hash: &SpatialHash,
        combat: &mut CombatSystem,
        structures: &mut StructureSystem,
        map_limit: Fx,
    ) {
        self.ensure_len(soldiers.len());
        let n = soldiers.len();
        let mut neighbors = [0u32; MAX_NEIGHBORS];

        for i in 0..n {
            if !soldiers.is_alive(i) || soldiers.hot.flags[i] & flags::MOUNTED == 0 {
                self.momentum_permille[i] = 0;
                self.charge_run_mm[i] = 0;
                self.horse_fear[i] = self.horse_fear[i].saturating_sub(2);
                continue;
            }

            if soldiers.hot.state[i] != State::Charging {
                // 突撃をやめれば助走とモメンタムはすぐに失われ、馬は落ち着く。
                self.momentum_permille[i] = 0;
                self.charge_run_mm[i] = 0;
                self.horse_fatigue[i] =
                    self.horse_fatigue[i].saturating_sub(HORSE_FATIGUE_RECOVERY);
                self.horse_fear[i] = self.horse_fear[i].saturating_sub(3);
                continue;
            }

            let vel = Vec2Fx::new(soldiers.hot.vel_x[i] as Fx, soldiers.hot.vel_y[i] as Fx);
            let speed_mm = sim_math::fx_to_mm(vel.len());
            if speed_mm >= CHARGE_MIN_SPEED_MM_PER_TICK {
                self.charge_run_mm[i] =
                    self.charge_run_mm[i].saturating_add(speed_mm.max(0) as u32);
                self.horse_fatigue[i] = self.horse_fatigue[i]
                    .saturating_add(HORSE_FATIGUE_CHARGE_RATE)
                    .min(MAX_HORSE_FATIGUE);
            }

            let raw_permille = ((self.charge_run_mm[i] as u64 * 1000)
                / CHARGE_FULL_DISTANCE_MM as u64)
                .min(1000) as u32;
            // 馬が疲れているほど、出せるモメンタムの上限が下がる。
            let fatigue_cap = 1000u32
                .saturating_sub(self.horse_fatigue[i] as u32 * 400 / MAX_HORSE_FATIGUE as u32)
                .max(200);
            self.momentum_permille[i] = ((raw_permille * fatigue_cap) / 1000).min(1000) as u16;

            if self.momentum_permille[i] < IMPACT_MOMENTUM_MIN_PERMILLE {
                continue;
            }

            let pos = soldiers.pos(i);
            let count =
                hash.query_enemies(soldiers, pos.x, pos.y, soldiers.faction[i], &mut neighbors);
            let ri = soldiers.radius(i);
            let contact_r = ri + fx_from_mm(CONTACT_RANGE_MM);
            let detect_r = ri + fx_from_mm(SPEAR_WALL_DETECT_MM);
            let half_arc = sim_math::brad_from_deg(45) as u32;

            let mut spear_wall = 0i32;
            let mut nearest: Option<(i64, usize)> = None;
            for &jid in &neighbors[..count] {
                let j = jid as usize;
                if !soldiers.is_alive(j) {
                    continue;
                }
                let jp = soldiers.pos(j);
                if !within_arc(soldiers.hot.facing[i], pos, jp, half_arc) {
                    continue;
                }
                let d2 = dist_sq(pos, jp);
                let rj = soldiers.radius(j);
                if d2 <= ((detect_r + rj) as i64).pow(2)
                    && combat.weapons[j].reach >= fx_from_mm(SPEAR_REACH_MM)
                    && !matches!(soldiers.hot.state[j], State::Broken | State::Downed)
                {
                    spear_wall += 1;
                }
                if d2 <= ((contact_r + rj) as i64).pow(2)
                    && nearest.map_or(true, |(best, _)| d2 < best)
                {
                    nearest = Some((d2, j));
                }
            }

            // 杭列（仕様 07 章 2 節）: 忌避判定を極端に厳しくする。杭だけが
            // 立っていて隣に生身の防御側がいなくても検知できるよう、
            // `nearest`（接触しうる兵士）とは独立に調べる。
            let stakes_id =
                structures.stakes_ahead(pos, soldiers.hot.facing[i], half_arc, detect_r);

            let target = match nearest {
                Some((_, j)) => Some(j),
                None if stakes_id.is_some() => None,
                None => continue,
            };

            let defender_formed = target.map_or(true, |t| {
                !matches!(
                    soldiers.hot.state[t],
                    State::Broken | State::Wavering | State::Downed
                )
            });
            let mut refusal = 30i32;
            refusal += spear_wall.min(6) * 220;
            refusal += if defender_formed { 200 } else { -400 };
            refusal += self.horse_fear[i] as i32 / 5;
            refusal -= soldiers.attrs[i].skill as i32 / 8;
            refusal -= self.momentum_permille[i] as i32 / 10;
            if let Some(id) = stakes_id {
                refusal += structures.stakes_refusal_bonus_permille(id) as i32;
            }
            let refusal = refusal.clamp(20, 970) as u32;

            let mut rng = Rng::stream(world_seed, i as u32, Purpose::HorseRefusal, tick);
            if rng.chance_permille(refusal) {
                soldiers.hot.vel_x[i] = 0;
                soldiers.hot.vel_y[i] = 0;
                soldiers.hot.state[i] = State::Wavering;
                self.horse_fear[i] = self.horse_fear[i]
                    .saturating_add(REFUSAL_FEAR_GAIN)
                    .min(1000);
                self.momentum_permille[i] = 0;
                self.charge_run_mm[i] = 0;
                soldiers.morale[i] = soldiers.morale[i].saturating_sub(REFUSAL_MORALE_SHOCK);
                combat.stats.horse_refusals = combat.stats.horse_refusals.saturating_add(1);
                combat.record_horse_refusal(
                    i as SoldierId,
                    target.map_or(NO_ID, |t| t as SoldierId),
                    tick,
                );
                continue;
            }

            let m = self.momentum_permille[i] as i32;

            // 忌避せずに杭を踏み抜いた: 馬に大ダメージ、杭も傷む
            // （仕様「突撃してきた馬に大ダメージ」「杭が折れる」）。
            if let Some(id) = stakes_id {
                let stakes_damage = ((STAKES_IMPACT_DAMAGE * m) / 1000).max(15) as u16;
                combat.apply_impact_damage(
                    i as SoldierId,
                    i as SoldierId,
                    stakes_damage,
                    soldiers,
                    tick,
                );
                structures.damage(id, STAKES_STRUCTURE_DAMAGE);
                if !soldiers.is_alive(i) {
                    continue;
                }
            }

            let Some(target) = target else {
                // 杭だけを踏み抜き、隣に人はいなかった: 馬が動揺して足を止める。
                self.momentum_permille[i] = 0;
                self.charge_run_mm[i] = 0;
                soldiers.hot.vel_x[i] = 0;
                soldiers.hot.vel_y[i] = 0;
                soldiers.hot.state[i] = State::Wavering;
                continue;
            };

            let damage = ((BASE_IMPACT_DAMAGE * m) / 1000).max(6) as u16;
            combat.apply_impact_damage(i as SoldierId, target as SoldierId, damage, soldiers, tick);

            if soldiers.is_alive(target) {
                let dir = soldiers.pos(target).sub(pos).normalized();
                let push_len = fx_scale_permille(fx_from_mm(KNOCKBACK_BASE_MM), m);
                let pushed = soldiers.pos(target).add(dir.scale(push_len));
                let clamped =
                    Vec2Fx::new(pushed.x.clamp(0, map_limit), pushed.y.clamp(0, map_limit));
                soldiers.set_pos(target, clamped);
                soldiers.morale[target] =
                    soldiers.morale[target].saturating_sub(IMPACT_MORALE_SHOCK);
            }

            // 突撃後は速度を失い、弱い Engaged 状態へ落ちる（仕様 06 章 4.1 節）。
            self.momentum_permille[i] = 0;
            self.charge_run_mm[i] = 0;
            if soldiers.is_alive(i) {
                soldiers.hot.state[i] = State::Engaged;
            }
        }
    }

    /// 決定論検証用の状態ハッシュ。
    pub fn state_hash(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut mix = |v: u64| {
            h ^= v;
            h = h.wrapping_mul(0x0100_0000_01b3);
        };
        for i in 0..self.momentum_permille.len() {
            mix(self.momentum_permille[i] as u64);
            mix(self.charge_run_mm[i] as u64);
            mix(self.horse_fatigue[i] as u64);
            mix(self.horse_fear[i] as u64);
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::{Armor, Weapon};
    use crate::soldiers::Attrs;
    use sim_math::{brad_from_deg, fx};

    fn rider_attrs() -> Attrs {
        Attrs::new(140, 140, 150, 160, 150, 140, 150, 150, 150, 120, 150, 150)
    }

    fn infantry_attrs() -> Attrs {
        Attrs::new(120, 120, 140, 140, 140, 120, 150, 150, 100, 120, 150, 150)
    }

    fn set_charging(soldiers: &mut Soldiers, i: usize, speed_mm_per_tick: i32) {
        soldiers.hot.state[i] = State::Charging;
        soldiers.hot.vel_x[i] = fx_from_mm(speed_mm_per_tick) as i16;
        soldiers.hot.vel_y[i] = 0;
    }

    /// 助走を積ませて実際に助走距離ぶんモメンタムを持たせる。
    fn spin_up_momentum(cavalry: &mut CavalrySystem, soldiers: &mut Soldiers, i: usize) {
        cavalry.ensure_len(soldiers.len());
        cavalry.charge_run_mm[i] = CHARGE_FULL_DISTANCE_MM;
        let _ = i;
        let _ = soldiers;
    }

    #[test]
    fn charging_into_a_dense_pike_wall_is_usually_refused() {
        let world_seed = 0xC0FFEE_u64;
        let mut refused = 0;
        let trials = 24;
        for trial in 0..trials {
            let mut soldiers = Soldiers::default();
            let rider = soldiers.push(
                fx(10),
                fx(10 + trial),
                brad_from_deg(0),
                0,
                0,
                rider_attrs(),
                flags::MOUNTED,
            );
            let mut combat = CombatSystem::default();
            combat.register();
            combat.set_mounted(rider, true);
            // 前方に密な槍衾を配置する。
            for k in 0..5 {
                soldiers.push(
                    fx(12 + k),
                    fx(10 + trial),
                    brad_from_deg(180),
                    0,
                    1,
                    infantry_attrs(),
                    0,
                );
                combat.register();
                combat.set_loadout(1 + k as u32, Weapon::pike(), Armor::mail());
            }
            let mut cavalry = CavalrySystem::default();
            spin_up_momentum(&mut cavalry, &mut soldiers, rider as usize);
            set_charging(&mut soldiers, rider as usize, 400);

            let mut hash = SpatialHash::default();
            hash.rebuild(&soldiers);
            let mut structures = StructureSystem::default();
            cavalry.tick(
                world_seed,
                trial as u32,
                &mut soldiers,
                &hash,
                &mut combat,
                &mut structures,
                fx(1000),
            );

            if soldiers.hot.state[rider as usize] == State::Wavering {
                refused += 1;
            }
        }
        assert!(
            refused * 3 >= trials * 2,
            "槍衾への正面突撃が十分な頻度で失敗していない: {refused}/{trials}"
        );
    }

    /// 仕様 12 章 M6 の受け入れ条件「杭列が騎兵突撃を実際に止める」。
    /// 隣に防御側の兵士が誰もいなくても、杭列そのものが忌避判定を厳しくする
    /// （`StructureSystem::stakes_ahead` は兵士の探索とは独立に働く）。
    #[test]
    fn charging_into_completed_stakes_alone_is_usually_refused() {
        let world_seed = 0xC0FFEE_u64;
        let mut refused = 0;
        let trials = 24;
        for trial in 0..trials {
            let mut soldiers = Soldiers::default();
            let rider = soldiers.push(
                fx(10),
                fx(10 + trial),
                brad_from_deg(0),
                0,
                0,
                rider_attrs(),
                flags::MOUNTED,
            );
            let mut combat = CombatSystem::default();
            combat.register();
            combat.set_mounted(rider, true);

            let mut structures = StructureSystem::default();
            let stakes = structures.build(
                crate::structures::StructureKind::Stakes,
                Vec2Fx::new(fx(13), fx(5 + trial)),
                Vec2Fx::new(fx(13), fx(15 + trial)),
                1,
            );
            structures.set_completion(stakes, 1000);

            let mut cavalry = CavalrySystem::default();
            spin_up_momentum(&mut cavalry, &mut soldiers, rider as usize);
            set_charging(&mut soldiers, rider as usize, 400);

            let mut hash = SpatialHash::default();
            hash.rebuild(&soldiers);
            cavalry.tick(
                world_seed,
                trial as u32,
                &mut soldiers,
                &hash,
                &mut combat,
                &mut structures,
                fx(1000),
            );

            if soldiers.hot.state[rider as usize] == State::Wavering {
                refused += 1;
            }
        }
        assert!(
            refused * 3 >= trials * 2,
            "杭列だけでも正面突撃が十分な頻度で失敗していない: {refused}/{trials}"
        );
    }

    /// 忌避せずに杭列へ突っ込んだ馬は大ダメージを受け、杭も傷む。
    #[test]
    fn charging_through_stakes_hurts_the_horse_and_damages_the_stakes() {
        let mut soldiers = Soldiers::default();
        let rider = soldiers.push(
            fx(10),
            fx(10),
            brad_from_deg(0),
            0,
            0,
            rider_attrs(),
            flags::MOUNTED,
        );
        let mut combat = CombatSystem::default();
        combat.register();
        combat.set_mounted(rider, true);
        let horse_hp_before = combat.horse_hp(rider);

        let mut structures = StructureSystem::default();
        let stakes = structures.build(
            crate::structures::StructureKind::Stakes,
            Vec2Fx::new(fx(13), fx(5)),
            Vec2Fx::new(fx(13), fx(15)),
            1,
        );
        structures.set_completion(stakes, 1000);
        let hp_before = structures.get(stakes).unwrap().hp;

        let mut cavalry = CavalrySystem::default();
        spin_up_momentum(&mut cavalry, &mut soldiers, rider as usize);
        set_charging(&mut soldiers, rider as usize, 400);

        let mut hash = SpatialHash::default();
        hash.rebuild(&soldiers);
        // seed を探すのではなく、忌避 vs 突破のどちらでも「杭を踏んだ結果として
        // 何か実害が起きている」ことを何 tick か回して確認する
        // （毎 tick 判定するので、いずれかの tick で必ず突破が起きる）。
        let mut breached = false;
        for tick in 0..40 {
            cavalry.tick(
                1,
                tick,
                &mut soldiers,
                &hash,
                &mut combat,
                &mut structures,
                fx(1000),
            );
            if combat.horse_hp(rider) < horse_hp_before
                || structures.get(stakes).unwrap().hp < hp_before
            {
                breached = true;
                break;
            }
            if soldiers.hot.state[rider as usize] != State::Charging {
                // 忌避して足を止めた、あるいは衝撃で Engaged に落ちた:
                // このテストでは「踏み抜いたときに実害が出るか」だけを見たいので、
                // 再突撃させて次の機会を待つ。
                set_charging(&mut soldiers, rider as usize, 400);
                spin_up_momentum(&mut cavalry, &mut soldiers, rider as usize);
            }
        }
        assert!(breached, "杭を踏み抜いても馬にも杭にも実害が出ていない");
    }

    #[test]
    fn charging_a_broken_enemy_is_decisive() {
        let mut soldiers = Soldiers::default();
        let rider = soldiers.push(
            fx(10),
            fx(10),
            brad_from_deg(0),
            0,
            0,
            rider_attrs(),
            flags::MOUNTED,
        );
        let victim = soldiers.push(
            fx(12),
            fx(10),
            brad_from_deg(180),
            0,
            1,
            infantry_attrs(),
            0,
        );
        soldiers.hot.state[victim as usize] = State::Broken;
        let hp_before = soldiers.hp[victim as usize];
        let morale_before = soldiers.morale[victim as usize];
        let pos_before = soldiers.pos(victim as usize);

        let mut combat = CombatSystem::default();
        combat.register();
        combat.set_mounted(rider, true);
        combat.register();

        let mut cavalry = CavalrySystem::default();
        spin_up_momentum(&mut cavalry, &mut soldiers, rider as usize);
        set_charging(&mut soldiers, rider as usize, 400);

        let mut hash = SpatialHash::default();
        hash.rebuild(&soldiers);
        cavalry.tick(
            1,
            0,
            &mut soldiers,
            &hash,
            &mut combat,
            &mut StructureSystem::default(),
            fx(1000),
        );

        assert_eq!(
            soldiers.hot.state[rider as usize],
            State::Engaged,
            "忌避せず衝撃に至っているはず"
        );
        assert!(
            soldiers.hp[victim as usize] < hp_before,
            "ダメージが入っていない"
        );
        assert!(
            soldiers.morale[victim as usize] < morale_before,
            "士気が下がっていない"
        );
        assert_ne!(
            soldiers.pos(victim as usize),
            pos_before,
            "ノックバックしていない"
        );
    }

    #[test]
    fn stationary_mounted_soldier_gets_no_charge_bonus() {
        // Charging 状態でなければモメンタムは常に 0 で、衝撃も忌避も起きない。
        let mut soldiers = Soldiers::default();
        let rider = soldiers.push(
            fx(10),
            fx(10),
            brad_from_deg(0),
            0,
            0,
            rider_attrs(),
            flags::MOUNTED,
        );
        soldiers.push(
            fx(11),
            fx(10),
            brad_from_deg(180),
            0,
            1,
            infantry_attrs(),
            0,
        );
        soldiers.hot.state[rider as usize] = State::Idle;

        let mut combat = CombatSystem::default();
        combat.register();
        combat.set_mounted(rider, true);
        combat.register();

        let mut cavalry = CavalrySystem::default();
        let mut hash = SpatialHash::default();
        hash.rebuild(&soldiers);
        cavalry.tick(
            1,
            0,
            &mut soldiers,
            &hash,
            &mut combat,
            &mut StructureSystem::default(),
            fx(1000),
        );

        assert_eq!(cavalry.momentum_permille(rider), 0);
        assert_eq!(combat.stats.charge_kills, 0);
        assert_eq!(combat.stats.horse_refusals, 0);
    }

    #[test]
    fn momentum_builds_up_over_charge_distance() {
        let mut soldiers = Soldiers::default();
        let rider = soldiers.push(
            fx(10),
            fx(10),
            brad_from_deg(0),
            0,
            0,
            rider_attrs(),
            flags::MOUNTED,
        );
        let mut combat = CombatSystem::default();
        combat.register();
        combat.set_mounted(rider, true);
        let mut cavalry = CavalrySystem::default();
        set_charging(&mut soldiers, rider as usize, 400);
        let hash = SpatialHash::default();

        for _ in 0..300 {
            cavalry.tick(
                1,
                0,
                &mut soldiers,
                &hash,
                &mut combat,
                &mut StructureSystem::default(),
                fx(1000),
            );
        }
        assert!(
            cavalry.momentum_permille(rider) > 500,
            "十分な距離を走ってもモメンタムが育っていない: {}",
            cavalry.momentum_permille(rider)
        );
    }
}
