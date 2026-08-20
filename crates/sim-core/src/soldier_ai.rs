//! 兵士個人の局所判断。
//!
//! 指揮系統が与える陣形スロットを基準にしつつ、近くに敵が現れた兵士だけが
//! 個別のタイミングで迎撃へ出る。部隊全体の目標を書き換えず、各兵士の `goal` を
//! 一時的に上書きする点は徒歩突撃の [`crate::charge`] と同じ。

use sim_math::{dist_sq, fx_from_mm, Purpose, Rng, Vec2Fx};

use crate::organization::{CommandTree, Intent};
use crate::soldiers::{flags, SoldierId, Soldiers, State, NO_ID};
use crate::spatial::{CoarseIndex, MAX_NEIGHBORS};

/// 個人が周囲を見渡すための粗い索引。8 m セルから任務の反応半径を覆う範囲を
/// 調べ、正確な距離と近さで候補を絞る。
const AWARENESS_CELL_M: i32 = 8;
/// 索引の更新間隔。4 tick = 0.2 秒なので、反応の遅れとしても自然な範囲。
const AWARENESS_REBUILD_TICKS: u32 = 4;

const DEFAULT_REACTION_RADIUS_MM: i32 = 8_000;
const MOVE_REACTION_RADIUS_MM: i32 = 6_000;
const HOLD_REACTION_RADIUS_MM: i32 = 10_000;
const ATTACK_REACTION_RADIUS_MM: i32 = 14_000;
const SCREEN_REACTION_RADIUS_MM: i32 = 12_000;
const DISENGAGE_MARGIN_MM: i32 = 3_000;

/// 一人の敵へ全員が殺到しないよう、既に向かっている味方一人あたりに課す点数。
const TARGET_LOAD_PENALTY: i32 = 180;
/// 人物追跡でも、対象本人へ直接殺到するのは小集団に限る。残りは部隊の移動に
/// 従い、途中で遭遇した護衛や近くの敵へ対処する。
const HUNT_TARGET_MAX_PURSUERS: u16 = 4;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IndividualAction {
    /// 部隊の陣形・命令に従う。
    #[default]
    FollowOrder = 0,
    /// 近くの敵を迎撃する。
    JoinFight = 1,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SoldierAiStats {
    pub joins: u32,
    pub retargets: u32,
    pub disengages: u32,
}

/// per-soldier の局所判断状態。認識用索引・任務ポリシー・負荷配列は導出キャッシュ。
/// 行動・対象・継続期限に加え、受理済み陣形目標と次回更新 tick は決定論ハッシュへ
/// 含める。
#[derive(Debug, Default)]
pub struct SoldierAiSystem {
    action: Vec<IndividualAction>,
    focus: Vec<SoldierId>,
    commit_until: Vec<u32>,
    /// 指揮系統から最後に受理した陣形目標。隊長の目標が毎 tick 動いても、
    /// 兵士は自分の反応周期でここへ取り込むため、全員が同時に歩き出さない。
    accepted_formation_goal: Vec<Vec2Fx>,
    next_formation_sample: Vec<u32>,
    awareness: Option<CoarseIndex>,
    reaction_radius_mm: Vec<i32>,
    disengage_radius_mm: Vec<i32>,
    ordered_focus: Vec<SoldierId>,
    commanded: Vec<bool>,
    target_load: Vec<u16>,
    pub stats: SoldierAiStats,
}

impl SoldierAiSystem {
    pub fn register(&mut self) {
        self.action.push(IndividualAction::FollowOrder);
        self.focus.push(NO_ID);
        self.commit_until.push(0);
        self.accepted_formation_goal.push(Vec2Fx::ZERO);
        self.next_formation_sample.push(u32::MAX);
        self.reaction_radius_mm.push(DEFAULT_REACTION_RADIUS_MM);
        self.disengage_radius_mm
            .push(DEFAULT_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM);
        self.ordered_focus.push(NO_ID);
        self.commanded.push(false);
        self.target_load.push(0);
    }

    fn ensure_len(&mut self, len: usize) {
        while self.action.len() < len {
            self.register();
        }
        self.reaction_radius_mm
            .resize(len, DEFAULT_REACTION_RADIUS_MM);
        self.disengage_radius_mm
            .resize(len, DEFAULT_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM);
        self.accepted_formation_goal.resize(len, Vec2Fx::ZERO);
        self.next_formation_sample.resize(len, u32::MAX);
        self.ordered_focus.resize(len, NO_ID);
        self.commanded.resize(len, false);
        self.target_load.resize(len, 0);
    }

    #[inline]
    pub fn action(&self, id: SoldierId) -> IndividualAction {
        self.action
            .get(id as usize)
            .copied()
            .unwrap_or(IndividualAction::FollowOrder)
    }

    #[inline]
    pub fn focus(&self, id: SoldierId) -> Option<SoldierId> {
        self.focus
            .get(id as usize)
            .copied()
            .filter(|&target| target != NO_ID)
    }

    /// 個々の認識タイミングで局所行動を選び、継続中の行動目標を毎 tick 適用する。
    pub fn tick(
        &mut self,
        world_seed: u64,
        tick: u32,
        soldiers: &mut Soldiers,
        command: &CommandTree,
        goals: &mut [Vec2Fx],
    ) {
        let n = soldiers.len().min(goals.len());
        self.ensure_len(n);
        self.assign_mission_policy(command, n);
        self.latch_formation_goals(tick, soldiers, goals, n);

        if self.awareness.is_none() || tick % AWARENESS_REBUILD_TICKS == 0 {
            self.awareness = Some(CoarseIndex::build(AWARENESS_CELL_M, soldiers));
        }

        self.target_load.fill(0);
        for i in 0..n {
            if self.action[i] != IndividualAction::JoinFight {
                continue;
            }
            if let Some(target) = soldiers.index_if_present(self.focus[i]) {
                self.target_load[target] = self.target_load[target].saturating_add(1);
            }
        }

        // `goals` はこの時点では全員ぶんの陣形目標なので、局所行動で上書きする
        // 前に持ち場からの距離を検証する。これで敵を追い続けて守備地点から際限なく
        // 離れることがない。
        for (i, &formation_goal) in goals.iter().take(n).enumerate() {
            if self.action[i] == IndividualAction::JoinFight
                && !self.current_action_is_valid(i, soldiers, formation_goal)
            {
                self.clear(i);
                self.stats.disengages = self.stats.disengages.saturating_add(1);
            }
        }

        let mut candidates = [0u32; MAX_NEIGHBORS];
        for (i, &formation_goal) in goals.iter().take(n).enumerate() {
            if tick < soldiers.think_at[i] {
                continue;
            }
            soldiers.think_at[i] = tick.saturating_add(think_interval(soldiers, i));
            if !can_choose_local_action(soldiers, i) {
                continue;
            }
            if self.action[i] == IndividualAction::JoinFight && tick < self.commit_until[i] {
                continue;
            }
            let radius_mm = self.reaction_radius_mm[i];
            if radius_mm <= 0 {
                if self.action[i] == IndividualAction::JoinFight {
                    self.clear(i);
                }
                continue;
            }

            let Some(index) = self.awareness.as_ref() else {
                continue;
            };
            let pos = soldiers.pos(i);
            let count = index.query_enemies_in_radius(
                soldiers,
                pos.x,
                pos.y,
                fx_from_mm(radius_mm),
                soldiers.faction[i],
                &mut candidates,
            );
            let radius = fx_from_mm(radius_mm) as i64;
            let mut best: Option<(i32, SoldierId)> = None;
            for &target_id in &candidates[..count] {
                let Some(target) = soldiers.index_if_present(target_id) else {
                    continue;
                };
                if !soldiers.is_alive(target) {
                    continue;
                }
                let d2 = dist_sq(pos, soldiers.pos(target));
                if d2 > radius * radius {
                    continue;
                }
                // 持ち場から見ても反応範囲内の敵だけを選ぶ。自分が既に前へ出ている
                // からといって、そこからさらに追撃範囲を延長しない。
                if dist_sq(formation_goal, soldiers.pos(target)) > radius * radius {
                    continue;
                }
                let is_ordered_target = self.ordered_focus[i] == target_id;
                if is_ordered_target
                    && self.target_load[target] >= HUNT_TARGET_MAX_PURSUERS
                    && self.focus[i] != target_id
                {
                    continue;
                }
                let distance_mm = sim_math::fx_to_mm(sim_math::isqrt64(d2 as u64) as i32);
                let fighting_bonus = if soldiers.hot.state[target].is_fighting() {
                    220
                } else {
                    0
                };
                let load_penalty = self.target_load[target] as i32 * TARGET_LOAD_PENALTY;
                let mission_bonus = if is_ordered_target { 900 } else { 0 };
                let score =
                    1_000 - distance_mm / 20 + fighting_bonus + mission_bonus - load_penalty;
                if best.map_or(true, |(best_score, best_id)| {
                    score > best_score || (score == best_score && target_id < best_id)
                }) {
                    best = Some((score, target_id));
                }
            }

            let Some((_, target)) = best else {
                if self.action[i] == IndividualAction::JoinFight {
                    self.clear(i);
                    self.stats.disengages = self.stats.disengages.saturating_add(1);
                }
                continue;
            };
            let mut rng = Rng::stream(world_seed, i as SoldierId, Purpose::DecisionNoise, tick);
            let attrs = soldiers.attrs[i];
            let morale_term = (soldiers.morale[i] as i32 - 400) / 3;
            let fatigue_term = soldiers.fatigue[i] as i32 / 45;
            let chance = (180
                + attrs.aggression as i32 * 2
                + attrs.bravery as i32
                + attrs.discipline as i32 / 2
                - attrs.self_preservation as i32
                + morale_term
                - fatigue_term)
                .clamp(40, 950) as u32;
            if !rng.chance_permille(chance) {
                continue;
            }

            let old = self.focus[i];
            if old != NO_ID && old != target {
                if let Some(old_index) = soldiers.index_if_present(old) {
                    self.target_load[old_index] = self.target_load[old_index].saturating_sub(1);
                }
                self.stats.retargets = self.stats.retargets.saturating_add(1);
            } else if self.action[i] != IndividualAction::JoinFight {
                self.stats.joins = self.stats.joins.saturating_add(1);
            }
            self.action[i] = IndividualAction::JoinFight;
            self.focus[i] = target;
            self.target_load[target as usize] = self.target_load[target as usize].saturating_add(1);
            let commitment = 16 + rng.range(0, 21) as u32 + attrs.composure as u32 / 8;
            self.commit_until[i] = tick.saturating_add(commitment);
        }

        // 判断がすべて済んでから目標を上書きする。途中で上書きすると、後続の兵士が
        // 「陣形上の持ち場」ではなく前の局所目標を追撃起点として読んでしまう。
        for i in 0..n {
            if self.action[i] == IndividualAction::JoinFight {
                let _ = self.apply_current_action(i, soldiers, goals);
            }
        }
    }

    fn assign_mission_policy(&mut self, command: &CommandTree, n: usize) {
        self.reaction_radius_mm[..n].fill(DEFAULT_REACTION_RADIUS_MM);
        self.disengage_radius_mm[..n].fill(DEFAULT_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM);
        self.ordered_focus[..n].fill(NO_ID);
        self.commanded[..n].fill(false);
        for node in &command.nodes {
            let Some(unit) = &node.unit else {
                continue;
            };
            let (radius, disengage, ordered_focus) = match node.objective {
                Some(
                    Intent::Reserve { .. }
                    | Intent::Withdraw {
                        fighting: false, ..
                    },
                ) => (0, 0, NO_ID),
                Some(Intent::Withdraw { fighting: true, .. }) => (
                    MOVE_REACTION_RADIUS_MM,
                    MOVE_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM,
                    NO_ID,
                ),
                Some(Intent::MoveTo { .. }) => (
                    MOVE_REACTION_RADIUS_MM,
                    MOVE_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM,
                    NO_ID,
                ),
                Some(Intent::Hold { .. }) => (
                    HOLD_REACTION_RADIUS_MM,
                    HOLD_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM,
                    NO_ID,
                ),
                Some(
                    Intent::Attack { .. }
                    | Intent::Charge { .. }
                    | Intent::Flank { .. }
                    | Intent::Envelop { .. }
                    | Intent::Pursue { .. },
                ) => (
                    ATTACK_REACTION_RADIUS_MM,
                    ATTACK_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM,
                    NO_ID,
                ),
                Some(Intent::Screen { .. }) => (
                    SCREEN_REACTION_RADIUS_MM,
                    SCREEN_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM,
                    NO_ID,
                ),
                Some(Intent::ShootAt { .. }) => (0, 0, NO_ID),
                Some(Intent::HuntPerson { target }) => (
                    ATTACK_REACTION_RADIUS_MM,
                    ATTACK_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM,
                    target,
                ),
                Some(Intent::OccupyArea { .. }) => (
                    DEFAULT_REACTION_RADIUS_MM,
                    DEFAULT_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM,
                    NO_ID,
                ),
                Some(Intent::GuardArea {
                    intercept_radius_m, ..
                }) => {
                    let intercept = i32::from(intercept_radius_m).saturating_mul(1_000);
                    (intercept, intercept + DISENGAGE_MARGIN_MM, NO_ID)
                }
                None => (
                    DEFAULT_REACTION_RADIUS_MM,
                    DEFAULT_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM,
                    NO_ID,
                ),
            };
            for &id in &unit.soldiers {
                if let Some(slot) = self.reaction_radius_mm.get_mut(id as usize) {
                    *slot = radius;
                }
                if let Some(slot) = self.disengage_radius_mm.get_mut(id as usize) {
                    *slot = disengage;
                }
                if let Some(slot) = self.ordered_focus.get_mut(id as usize) {
                    *slot = ordered_focus;
                }
                if let Some(slot) = self.commanded.get_mut(id as usize) {
                    *slot = true;
                }
            }
        }
    }

    /// 隊列アンカーが作った最新目標を、兵士ごとの反応周期で受理する。
    ///
    /// `formation_goals` は毎 tick 全員ぶんを更新するが、ここでいったん個人の
    /// 受理済み目標へラッチする。初回更新の位相を ID から散らすことで、命令が
    /// 届いた瞬間に全員が同時発進する代わりに、短い時間幅の中で順次動き出す。
    fn latch_formation_goals(
        &mut self,
        tick: u32,
        soldiers: &Soldiers,
        goals: &mut [Vec2Fx],
        n: usize,
    ) {
        for (i, goal) in goals.iter_mut().take(n).enumerate() {
            let raw_goal = *goal;
            let interval = formation_goal_interval(soldiers, i);
            if self.next_formation_sample[i] == u32::MAX {
                self.accepted_formation_goal[i] = raw_goal;
                let phase = formation_goal_phase(i as SoldierId, interval);
                self.next_formation_sample[i] = tick.saturating_add(phase);
            } else {
                // 工兵タスクは作業地点・補給先・負傷者へ直接向かう必要がある。
                // また、指揮系統に属さないテスト／サンドボックス兵は、局所迎撃中に
                // 前 tick の迎撃目標を「新しい命令」と誤認して取り込まない。
                let immediate = soldiers.hot.flags[i] & flags::ENGINEER != 0
                    || soldiers.hot.state[i] == State::Working;
                let may_sample = self.commanded[i]
                    || self.action[i] == IndividualAction::FollowOrder
                    || immediate;
                if may_sample && (immediate || tick >= self.next_formation_sample[i]) {
                    self.accepted_formation_goal[i] = raw_goal;
                    self.next_formation_sample[i] = tick.saturating_add(interval);
                }
            }
            *goal = self.accepted_formation_goal[i];
        }
    }

    fn apply_current_action(
        &self,
        i: usize,
        soldiers: &mut Soldiers,
        goals: &mut [Vec2Fx],
    ) -> bool {
        if !self.current_action_is_valid(i, soldiers, goals[i]) {
            return false;
        }
        let target = self.focus[i] as usize;
        goals[i] = soldiers.pos(target);
        if matches!(
            soldiers.hot.state[i],
            State::Idle | State::Marching | State::Repositioning
        ) {
            soldiers.hot.state[i] = State::Advancing;
        }
        true
    }

    fn current_action_is_valid(
        &self,
        i: usize,
        soldiers: &Soldiers,
        formation_goal: Vec2Fx,
    ) -> bool {
        if !soldiers.is_alive(i) || self.reaction_radius_mm[i] <= 0 {
            return false;
        }
        let Some(target) = soldiers.index_if_present(self.focus[i]) else {
            return false;
        };
        if !soldiers.is_alive(target) {
            return false;
        }
        let drop_radius = fx_from_mm(self.disengage_radius_mm[i]) as i64;
        dist_sq(formation_goal, soldiers.pos(target)) <= drop_radius * drop_radius
    }

    fn clear(&mut self, i: usize) {
        self.action[i] = IndividualAction::FollowOrder;
        self.focus[i] = NO_ID;
        self.commit_until[i] = 0;
    }

    pub fn state_hash(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for i in 0..self.action.len() {
            h ^= self.action[i] as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
            h ^= self.focus[i] as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
            h ^= self.commit_until[i] as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
            h ^= self.accepted_formation_goal[i].x as u32 as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
            h ^= self.accepted_formation_goal[i].y as u32 as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
            h ^= self.next_formation_sample[i] as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h
    }
}

/// 陣形目標を見直す周期。反射と規律が高いほど短く、疲労・低士気で長くなる。
/// 4〜22 tick（0.2〜1.1 秒）の範囲なので、命令伝達そのものの遅延に比べれば短いが、
/// 一斉発進を崩すには十分な個人差になる。
fn formation_goal_interval(soldiers: &Soldiers, i: usize) -> u32 {
    let attrs = soldiers.attrs[i];
    let response = (attrs.reflex as u32 + attrs.discipline as u32) / 2;
    let base = 4 + (255u32.saturating_sub(response) * 12 / 255);
    let fatigue_delay = soldiers.fatigue[i] as u32 / 2_500;
    let morale_delay = if soldiers.morale[i] < 300 { 2 } else { 0 };
    (base + fatigue_delay + morale_delay).clamp(4, 22)
}

/// 連番 ID がそのまま前から後ろへの波にならないよう乗法ハッシュで位相を散らす。
fn formation_goal_phase(id: SoldierId, interval: u32) -> u32 {
    let mixed = id.wrapping_mul(2_654_435_761) ^ id.rotate_left(13);
    let mixed = mixed ^ (mixed >> 16);
    1 + mixed % interval.max(1)
}

fn can_choose_local_action(soldiers: &Soldiers, i: usize) -> bool {
    if !soldiers.is_alive(i)
        || soldiers.hot.flags[i] & (flags::ENGINEER | flags::MISSILE_TROOP) != 0
    {
        return false;
    }
    !matches!(
        soldiers.hot.state[i],
        State::Engaged
            | State::Charging
            | State::Shooting
            | State::Reloading
            | State::Broken
            | State::Rallying
            | State::Working
            | State::Downed
            | State::Dead
    )
}

fn think_interval(soldiers: &Soldiers, i: usize) -> u32 {
    let base = match soldiers.hot.state[i] {
        State::Engaged | State::Charging => 2,
        State::Broken | State::Shooting | State::Advancing | State::Repositioning => 4,
        State::Marching => 10,
        State::Downed | State::Dead => return u32::MAX,
        _ => 20,
    };
    let response = 128 + soldiers.attrs[i].reflex as u32 / 2;
    (base * 192 / response).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soldiers::Attrs;
    use sim_math::fx;

    fn eager_attrs() -> Attrs {
        Attrs::new(160, 160, 160, 160, 255, 160, 255, 220, 255, 0, 160, 200)
    }

    #[test]
    fn formation_goal_acceptance_is_staggered_between_soldiers() {
        let mut soldiers = Soldiers::default();
        for i in 0..8 {
            soldiers.push(fx(100), fx(100 + i), 0, 0, 0, Attrs::default(), 0);
        }
        let mut ai = SoldierAiSystem::default();
        for _ in 0..soldiers.len() {
            ai.register();
        }
        let command = CommandTree::new();
        let initial: Vec<_> = (0..soldiers.len()).map(|i| soldiers.pos(i)).collect();
        let ordered: Vec<_> = initial
            .iter()
            .map(|p| p.add(Vec2Fx::new(fx(20), 0)))
            .collect();
        let mut goals = initial.clone();
        ai.tick(31, 0, &mut soldiers, &command, &mut goals);

        goals.clone_from(&ordered);
        ai.tick(31, 1, &mut soldiers, &command, &mut goals);
        let first_wave = goals
            .iter()
            .zip(&ordered)
            .filter(|(accepted, ordered)| accepted == ordered)
            .count();
        assert!(first_wave > 0);
        assert!(first_wave < soldiers.len());

        for tick in 2..=22 {
            goals.clone_from(&ordered);
            ai.tick(31, tick, &mut soldiers, &command, &mut goals);
        }
        assert_eq!(goals, ordered);
    }

    #[test]
    fn alert_disciplined_soldiers_refresh_formation_goals_faster() {
        let mut soldiers = Soldiers::default();
        soldiers.push(fx(100), fx(100), 0, 0, 0, Attrs::default(), 0);
        soldiers.push(fx(101), fx(100), 0, 0, 0, eager_attrs(), 0);
        assert!(formation_goal_interval(&soldiers, 1) < formation_goal_interval(&soldiers, 0));
    }

    #[test]
    fn engineer_task_goals_bypass_formation_response_delay() {
        let mut soldiers = Soldiers::default();
        let engineer = soldiers.push(fx(100), fx(100), 0, 0, 0, Attrs::default(), flags::ENGINEER);
        let mut ai = SoldierAiSystem::default();
        ai.register();
        let command = CommandTree::new();
        let mut goals = vec![soldiers.pos(engineer as usize)];
        ai.tick(37, 0, &mut soldiers, &command, &mut goals);

        let task_goal = Vec2Fx::new(fx(130), fx(100));
        goals[engineer as usize] = task_goal;
        ai.tick(37, 1, &mut soldiers, &command, &mut goals);
        assert_eq!(goals[engineer as usize], task_goal);
    }

    #[test]
    fn nearby_soldiers_join_a_fight_on_individual_think_ticks() {
        let mut soldiers = Soldiers::default();
        let ally = soldiers.push(fx(100), fx(100), 0, 0, 0, eager_attrs(), 0);
        let enemy = soldiers.push(fx(106), fx(100), 0, 0, 1, Attrs::default(), 0);
        soldiers.hot.state[enemy as usize] = State::Engaged;
        soldiers.think_at[ally as usize] = 0;
        let mut ai = SoldierAiSystem::default();
        ai.register();
        ai.register();
        let command = CommandTree::new();
        let mut goals = vec![soldiers.pos(0), soldiers.pos(1)];

        for tick in 0..40 {
            ai.tick(7, tick, &mut soldiers, &command, &mut goals);
            if ai.action(ally) == IndividualAction::JoinFight {
                break;
            }
        }

        assert_eq!(ai.action(ally), IndividualAction::JoinFight);
        assert_eq!(ai.focus(ally), Some(enemy));
        assert_eq!(goals[ally as usize], soldiers.pos(enemy as usize));
        assert_eq!(soldiers.hot.state[ally as usize], State::Advancing);

        // 兵士自身が敵の近くまで追っていても、陣形上の持ち場から離れすぎたら戻る。
        soldiers.set_pos(ally as usize, Vec2Fx::new(fx(112), fx(100)));
        soldiers.set_pos(enemy as usize, Vec2Fx::new(fx(116), fx(100)));
        goals[ally as usize] = Vec2Fx::new(fx(100), fx(100));
        ai.tick(7, 41, &mut soldiers, &command, &mut goals);
        assert_eq!(ai.action(ally), IndividualAction::FollowOrder);
    }

    #[test]
    fn target_load_spreads_attackers_between_equal_enemies() {
        let mut soldiers = Soldiers::default();
        let allies: Vec<_> = (0..4)
            .map(|i| soldiers.push(fx(100), fx(99 + i), 0, 0, 0, eager_attrs(), 0))
            .collect();
        let enemies = [
            soldiers.push(fx(106), fx(100), 0, 0, 1, Attrs::default(), 0),
            soldiers.push(fx(106), fx(102), 0, 0, 1, Attrs::default(), 0),
        ];
        for &enemy in &enemies {
            soldiers.hot.state[enemy as usize] = State::Engaged;
        }
        for &ally in &allies {
            soldiers.think_at[ally as usize] = 0;
        }
        let mut ai = SoldierAiSystem::default();
        for _ in 0..soldiers.len() {
            ai.register();
        }
        let command = CommandTree::new();
        let mut goals: Vec<_> = (0..soldiers.len()).map(|i| soldiers.pos(i)).collect();

        for tick in 0..80 {
            ai.tick(11, tick, &mut soldiers, &command, &mut goals);
        }
        let focused: std::collections::BTreeSet<_> =
            allies.iter().filter_map(|&ally| ai.focus(ally)).collect();
        assert_eq!(focused.len(), 2);
    }

    #[test]
    fn reserve_order_prevents_local_pursuit() {
        let mut soldiers = Soldiers::default();
        let ally = soldiers.push(fx(100), fx(100), 0, 0, 0, eager_attrs(), 0);
        let enemy = soldiers.push(fx(104), fx(100), 0, 0, 1, Attrs::default(), 0);
        soldiers.hot.state[enemy as usize] = State::Engaged;
        soldiers.think_at[ally as usize] = 0;

        let mut command = CommandTree::new();
        let unit = crate::organization::Unit {
            soldiers: vec![ally],
            troop_type: 0,
            formation: crate::organization::FORMATION_LINE,
            formation_origin: soldiers.pos(ally as usize),
            formation_facing: 0,
            ranks: 1,
            file_spacing: fx_from_mm(800),
            rank_spacing: fx_from_mm(800),
            banner: None,
            formation_change: None,
            path: Vec::new(),
            path_final: soldiers.pos(ally as usize),
            pursuit_leash: None,
        };
        let node = command.add_node(None, 0, 0, ally, vec![], Some(unit));
        command.node_mut(node).unwrap().objective = Some(Intent::Reserve {
            rally_pos: soldiers.pos(ally as usize),
        });
        let mut ai = SoldierAiSystem::default();
        ai.register();
        ai.register();
        let mut goals = vec![soldiers.pos(0), soldiers.pos(1)];

        for tick in 0..80 {
            ai.tick(19, tick, &mut soldiers, &command, &mut goals);
        }
        assert_eq!(ai.action(ally), IndividualAction::FollowOrder);
        assert_eq!(goals[ally as usize], soldiers.pos(ally as usize));
    }

    #[test]
    fn guard_area_intercepts_then_returns_to_the_post() {
        let mut soldiers = Soldiers::default();
        let post = Vec2Fx::new(fx(100), fx(100));
        let ally = soldiers.push(post.x, post.y, 0, 0, 0, eager_attrs(), 0);
        let enemy = soldiers.push(fx(112), fx(100), 0, 0, 1, Attrs::default(), 0);
        soldiers.hot.state[enemy as usize] = State::Engaged;
        soldiers.think_at[ally as usize] = 0;

        let mut command = CommandTree::new();
        let unit = crate::organization::Unit {
            soldiers: vec![ally],
            troop_type: 0,
            formation: crate::organization::FORMATION_LINE,
            formation_origin: post,
            formation_facing: 0,
            ranks: 1,
            file_spacing: fx_from_mm(800),
            rank_spacing: fx_from_mm(800),
            banner: None,
            formation_change: None,
            path: Vec::new(),
            path_final: post,
            pursuit_leash: None,
        };
        let node = command.add_node(None, 0, 0, ally, vec![], Some(unit));
        command.node_mut(node).unwrap().objective = Some(Intent::GuardArea {
            center: post,
            radius_m: 5,
            intercept_radius_m: 14,
        });
        let mut ai = SoldierAiSystem::default();
        ai.register();
        ai.register();
        let mut goals = vec![post, soldiers.pos(enemy as usize)];

        for tick in 0..80 {
            goals[ally as usize] = post;
            ai.tick(23, tick, &mut soldiers, &command, &mut goals);
            if ai.action(ally) == IndividualAction::JoinFight {
                break;
            }
        }
        assert_eq!(ai.action(ally), IndividualAction::JoinFight);

        soldiers.set_pos(enemy as usize, Vec2Fx::new(fx(120), fx(100)));
        goals[ally as usize] = post;
        ai.tick(23, 81, &mut soldiers, &command, &mut goals);
        assert_eq!(ai.action(ally), IndividualAction::FollowOrder);
        assert_eq!(goals[ally as usize], post);
    }

    #[test]
    fn hunt_person_limits_direct_pursuers_to_a_small_group() {
        let mut soldiers = Soldiers::default();
        let allies: Vec<_> = (0..8)
            .map(|i| soldiers.push(fx(100), fx(97 + i), 0, 0, 0, eager_attrs(), 0))
            .collect();
        let target = soldiers.push(fx(106), fx(100), 0, 0, 1, Attrs::default(), 0);
        soldiers.hot.state[target as usize] = State::Engaged;
        for &ally in &allies {
            soldiers.think_at[ally as usize] = 0;
        }

        let mut command = CommandTree::new();
        let post = Vec2Fx::new(fx(100), fx(100));
        let unit = crate::organization::Unit {
            soldiers: allies.clone(),
            troop_type: 0,
            formation: crate::organization::FORMATION_LINE,
            formation_origin: post,
            formation_facing: 0,
            ranks: 2,
            file_spacing: fx_from_mm(800),
            rank_spacing: fx_from_mm(800),
            banner: None,
            formation_change: None,
            path: Vec::new(),
            path_final: post,
            pursuit_leash: None,
        };
        let node = command.add_node(None, 0, 0, allies[0], vec![], Some(unit));
        command.node_mut(node).unwrap().objective = Some(Intent::HuntPerson { target });

        let mut ai = SoldierAiSystem::default();
        for _ in 0..soldiers.len() {
            ai.register();
        }
        let formation_goals: Vec<_> = (0..soldiers.len()).map(|i| soldiers.pos(i)).collect();
        let mut goals = formation_goals.clone();
        for tick in 0..80 {
            goals.clone_from(&formation_goals);
            ai.tick(29, tick, &mut soldiers, &command, &mut goals);
        }
        let pursuers = allies
            .iter()
            .filter(|&&ally| ai.focus(ally) == Some(target))
            .count();
        assert!(pursuers > 0);
        assert!(pursuers <= HUNT_TARGET_MAX_PURSUERS as usize);
    }
}
