//! 兵士個人の局所知覚（仕様 05 章 3.3 節、B1）。
//!
//! 兵士は戦場の全体を知らない。ここでは、各兵士の周囲だけを決定論的にまとめ、
//! 「近くに敵がいる」と「近くで味方が戦っている」を別の事実として持つ。
//!
//! 知覚の更新は兵士ごとに位相をずらした周期で行い、1 回に見る人数にも上限を
//! 置く（`scan_budget`）。人が一度に把握できる人数には限りがあり、密集地でも
//! 計算量が兵数に対して一定に保てるため。全兵士対全兵士の走査はしない。
//!
//! 唯一ワールドを書き換えるのは「背後を突かれた驚き」による士気低下で、これは
//! 仕様 05 章 3.2 節の `rear_threat` を、判断ではなく状態として表したもの。

use sim_math::{angle_diff, fx, fx_from_mm, fx_to_mm, isqrt64, Brad, Fx, Vec2Fx};

use crate::soldiers::{SoldierId, Soldiers, State, NO_ID};
use crate::spatial::{CoarseIndex, MAX_NEIGHBORS};

/// 知覚の広さ・更新頻度・上限。意味のある単位名を持たせ、調整はここに集約する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PerceptionConfig {
    /// 周囲を把握する半径（m）。任務ごとの迎撃半径とは別で、こちらは「見えて
    /// いる範囲」。
    pub sight_radius_m: i32,
    /// 落ち着いている兵士が周囲を見直す間隔（tick）。
    pub update_interval_ticks: u32,
    /// 交戦中・突撃中の兵士が見直す間隔（tick）。
    pub combat_update_interval_ticks: u32,
    /// 敵が視界にいない兵士が見直す間隔（tick）。遠方の非接敵兵は頻繁に周囲を
    /// 見直す必要がない（B7 の active/sleeping 方針）。
    pub idle_update_interval_ticks: u32,
    /// 1 回の更新で見る人数の上限。
    pub scan_budget: u32,
    /// 正面とみなす扇の半角（brad）。
    pub front_half_arc_brad: u32,
    /// 側面とみなす扇の半角（brad）。これを超えたら背面。
    pub flank_half_arc_brad: u32,
    /// 押し合いの元になる密集を測る半径（mm）。
    pub crowd_radius_mm: i32,
    /// 味方の戦闘を「援護できる」とみなす距離（mm）。
    pub support_radius_mm: i32,
    /// 背後の敵に初めて気づいたときに落ちる士気。
    pub rear_surprise_morale: u16,
    /// 一度驚いてから、次に驚けるようになるまで（tick）。混戦の中で敵が
    /// 前後を出入りするたびに士気が削れて、接触した瞬間に全軍が崩れるのを防ぐ。
    pub startle_cooldown_ticks: u32,
}

impl Default for PerceptionConfig {
    fn default() -> Self {
        PerceptionConfig {
            sight_radius_m: 14,
            // 20 Hz なので 8 tick = 0.4 秒。交戦中は 2 tick = 0.1 秒。
            update_interval_ticks: 8,
            combat_update_interval_ticks: 2,
            // 24 tick = 1.2 秒。敵が現れれば次の更新で拾い、そこから 8 tick 周期へ戻る。
            idle_update_interval_ticks: 24,
            scan_budget: 32,
            // ±60 度が正面、±120 度までが側面、それより後ろが背面。
            front_half_arc_brad: 10_922,
            flank_half_arc_brad: 21_845,
            crowd_radius_mm: 1_500,
            support_radius_mm: 6_000,
            rear_surprise_morale: 40,
            // 20 Hz なので 200 tick = 10 秒。
            startle_cooldown_ticks: 200,
        }
    }
}

/// 脅威が来ている方向。正面・側面・背面で反応の速さと士気への効き方が変わる。
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThreatArc {
    #[default]
    Front = 0,
    Flank = 1,
    Rear = 2,
}

/// 1 人ぶんの局所知覚。導出値なので状態ハッシュには入れない
/// （士気への影響だけが `Soldiers` 側に残る）。
///
/// 既定値は「まだ何も見ていない」。敵の ID を 0 のままにすると、周囲を一度も
/// 見ていない兵士が「目の前に 0 番の敵がいる」と誤読されるので、番兵で埋める。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalAwareness {
    /// 最後に周囲を見た tick。
    pub updated_tick: u32,
    /// 視界内の敵と味方の人数（`scan_budget` で頭打ち）。
    pub enemies: u8,
    pub allies: u8,
    /// 正面・側面・背面から来ている敵の人数。
    pub threat_front: u8,
    pub threat_flank: u8,
    pub threat_rear: u8,
    /// `crowd_radius_mm` 以内の味方の人数（押し合いの元）。
    pub crowding: u8,
    /// 近くで崩れている味方（`Wavering` / `Broken`）の人数。
    pub local_broken: u8,
    /// もっとも近い敵。
    pub nearest_enemy: SoldierId,
    pub nearest_enemy_mm: i32,
    /// 近くの味方が戦っている相手のうち、もっとも近い敵。援護に入れる戦闘が
    /// あるかどうかは、「近くに敵がいる」とは別の事実として持つ。
    pub supportable_enemy: SoldierId,
    pub supportable_enemy_mm: i32,
}

impl Default for LocalAwareness {
    fn default() -> Self {
        LocalAwareness {
            updated_tick: 0,
            enemies: 0,
            allies: 0,
            threat_front: 0,
            threat_flank: 0,
            threat_rear: 0,
            crowding: 0,
            local_broken: 0,
            nearest_enemy: NO_ID,
            nearest_enemy_mm: i32::MAX,
            supportable_enemy: NO_ID,
            supportable_enemy_mm: i32::MAX,
        }
    }
}

impl LocalAwareness {
    /// 視界内に敵がいるか。
    #[inline]
    pub fn sees_enemy(&self) -> bool {
        self.nearest_enemy != NO_ID
    }

    /// 援護できる味方の戦闘があるか。
    #[inline]
    pub fn can_support_fight(&self) -> bool {
        self.supportable_enemy != NO_ID
    }

    /// 正面以外から脅威を受けているか。
    #[inline]
    pub fn threatened_from_behind(&self) -> bool {
        self.threat_flank > 0 || self.threat_rear > 0
    }
}

/// 兵士ごとの局所知覚をまとめて更新する。
#[derive(Debug)]
pub struct PerceptionSystem {
    cfg: PerceptionConfig,
    awareness: Vec<LocalAwareness>,
    next_update: Vec<u32>,
    /// 前回の更新で背面に敵がいたか。初めて気づいた瞬間だけ士気へ効かせる。
    rear_threat_seen: Vec<bool>,
    /// 次に驚けるようになる tick。
    startle_ready_tick: Vec<u32>,
    index: Option<CoarseIndex>,
    /// 索引を組み直した tick。
    index_tick: u32,
}

/// 索引のセルの一辺（m）。視界半径の半分程度にしておくと、走査するリング数が
/// 少なく、1 セルあたりの人数も過大にならない。
const INDEX_CELL_M: i32 = 8;
/// 索引を組み直す間隔（tick）。
const INDEX_REBUILD_TICKS: u32 = 4;

impl Default for PerceptionSystem {
    fn default() -> Self {
        PerceptionSystem::new(PerceptionConfig::default())
    }
}

impl PerceptionSystem {
    pub fn new(cfg: PerceptionConfig) -> Self {
        PerceptionSystem {
            cfg,
            awareness: Vec::new(),
            next_update: Vec::new(),
            rear_threat_seen: Vec::new(),
            startle_ready_tick: Vec::new(),
            index: None,
            index_tick: u32::MAX,
        }
    }

    pub fn config(&self) -> &PerceptionConfig {
        &self.cfg
    }

    pub fn register(&mut self) {
        self.awareness.push(LocalAwareness::default());
        // 初回の更新時刻を ID で散らし、全員が同じ tick に周囲を見ないようにする。
        let id = self.awareness.len() as u32 - 1;
        self.next_update
            .push(id % self.cfg.update_interval_ticks.max(1));
        self.rear_threat_seen.push(false);
        self.startle_ready_tick.push(0);
    }

    fn ensure_len(&mut self, len: usize) {
        while self.awareness.len() < len {
            self.register();
        }
    }

    /// この兵士がいま把握している周囲。
    #[inline]
    pub fn awareness(&self, id: SoldierId) -> LocalAwareness {
        self.awareness.get(id as usize).copied().unwrap_or_default()
    }

    /// 局所判断が半径問い合わせに使う索引。知覚と同じものを共有し、同じ tick に
    /// 索引を 2 つ持たない。
    #[inline]
    pub fn index(&self) -> Option<&CoarseIndex> {
        self.index.as_ref()
    }

    /// 周囲の把握を 1 tick ぶん進める。
    ///
    /// 更新するのは自分の周期が来た兵士だけ。背後の敵に初めて気づいた兵士は
    /// 驚いて士気を落とす（それ以外に `soldiers` を書き換えない）。
    pub fn tick(&mut self, tick: u32, soldiers: &mut Soldiers) {
        let n = soldiers.len();
        self.ensure_len(n);
        if self.index.is_none() || tick.wrapping_sub(self.index_tick) >= INDEX_REBUILD_TICKS {
            self.index = Some(CoarseIndex::build(INDEX_CELL_M, soldiers));
            self.index_tick = tick;
        }
        // 走査中は索引を手元へ取り出す。`self` の他のフィールド（更新時刻・
        // 知覚結果）を書き換えるので、`self` から借りたままにしない。
        let Some(index) = self.index.take() else {
            return;
        };

        let sight = fx(self.cfg.sight_radius_m);
        let crowd_sq = {
            let r = fx_from_mm(self.cfg.crowd_radius_mm) as i64;
            r * r
        };
        let support_sq = {
            let r = fx_from_mm(self.cfg.support_radius_mm) as i64;
            r * r
        };

        for i in 0..n {
            if !soldiers.is_alive(i) {
                continue;
            }
            if tick < self.next_update[i] {
                continue;
            }
            // 前回の把握で敵が見えていなければ、次の見直しまで間隔を空ける。
            let interval = if soldiers.hot.state[i].is_fighting() {
                self.cfg.combat_update_interval_ticks
            } else if self.awareness[i].sees_enemy() || self.awareness[i].updated_tick == 0 {
                self.cfg.update_interval_ticks
            } else {
                self.cfg.idle_update_interval_ticks
            };
            self.next_update[i] = tick.saturating_add(interval.max(1));

            let pos = soldiers.pos(i);
            let facing = soldiers.hot.facing[i];
            let faction = soldiers.faction[i];
            let mut next = LocalAwareness {
                updated_tick: tick,
                ..LocalAwareness::default()
            };
            // 味方が戦っている相手を拾うため、いったん敵の位置を覚えておく。
            let mut nearest_enemy_d2 = i64::MAX;
            let mut support_d2 = i64::MAX;

            // 敵は味方とは別に引く。密集陣形では周りがほぼ味方なので、両方を
            // 同じ走査枠で数えると、枠を味方で使い切って「敵が見えない」ことに
            // なってしまう（`query_enemies_in_radius` は味方を枠に数えない）。
            // 敵の人数は最大 `MAX_NEIGHBORS` までで頭打ちになる。
            let mut enemies = [0u32; MAX_NEIGHBORS];
            let found =
                index.query_enemies_in_radius(soldiers, pos.x, pos.y, sight, faction, &mut enemies);
            for &id in &enemies[..found] {
                let other = id as usize;
                if !soldiers.is_alive(other) {
                    continue;
                }
                next.enemies = next.enemies.saturating_add(1);
                match threat_arc(facing, pos, soldiers.pos(other), &self.cfg) {
                    ThreatArc::Front => next.threat_front = next.threat_front.saturating_add(1),
                    ThreatArc::Flank => next.threat_flank = next.threat_flank.saturating_add(1),
                    ThreatArc::Rear => next.threat_rear = next.threat_rear.saturating_add(1),
                }
                let d2 = sim_math::dist_sq(pos, soldiers.pos(other));
                if d2 < nearest_enemy_d2 {
                    nearest_enemy_d2 = d2;
                    next.nearest_enemy = id;
                }
            }

            // 味方は走査枠のぶんだけ見る。密集の度合いと、援護に入れる戦闘を拾う。
            index.scan_in_radius(
                soldiers,
                pos.x,
                pos.y,
                sight,
                self.cfg.scan_budget,
                |id, d2| {
                    let other = id as usize;
                    if other == i || !soldiers.is_alive(other) {
                        return;
                    }
                    if soldiers.faction[other] == faction {
                        next.allies = next.allies.saturating_add(1);
                        if d2 <= crowd_sq {
                            next.crowding = next.crowding.saturating_add(1);
                        }
                        if matches!(soldiers.hot.state[other], State::Wavering | State::Broken) {
                            next.local_broken = next.local_broken.saturating_add(1);
                        }
                        // 味方が交戦している相手は「援護に入れる戦闘」。
                        let opponent = soldiers.target[other];
                        if soldiers.hot.state[other].is_fighting()
                            && opponent != NO_ID
                            && d2 <= support_sq
                        {
                            if let Some(t) = soldiers.index_if_present(opponent) {
                                if soldiers.is_alive(t) && soldiers.faction[t] != faction {
                                    let od2 = sim_math::dist_sq(pos, soldiers.pos(t));
                                    if od2 < support_d2 {
                                        support_d2 = od2;
                                        next.supportable_enemy = opponent;
                                    }
                                }
                            }
                        }
                    }
                },
            );

            next.nearest_enemy_mm = distance_mm_from_sq(nearest_enemy_d2);
            next.supportable_enemy_mm = distance_mm_from_sq(support_d2);

            // 背後を取られたと初めて気づいた瞬間だけ、驚きとして士気が落ちる。
            //
            // 「驚き」は、前も横も無事だと思っていた兵士が背後の敵に気づくこと。
            // 正面で戦っている最中に敵が回り込むのは混戦のうちで、驚きとしては
            // 数えない（数えると、両軍が接触した瞬間に全員の士気が削れて一斉に
            // 崩れる）。続けて驚かないよう間隔も置く。
            let surprised = next.threat_rear > 0
                && next.threat_front == 0
                && next.threat_flank == 0
                && !self.rear_threat_seen[i]
                && !soldiers.hot.state[i].is_fighting()
                && tick >= self.startle_ready_tick[i];
            if surprised {
                soldiers.morale[i] =
                    soldiers.morale[i].saturating_sub(self.cfg.rear_surprise_morale);
                self.startle_ready_tick[i] = tick.saturating_add(self.cfg.startle_cooldown_ticks);
            }
            self.rear_threat_seen[i] = next.threat_rear > 0;
            self.awareness[i] = next;
        }
        self.index = Some(index);
    }
}

fn distance_mm_from_sq(d2: i64) -> i32 {
    if d2 == i64::MAX {
        return i32::MAX;
    }
    fx_to_mm(isqrt64(d2 as u64) as Fx)
}

/// `from` にいる兵士（`facing` を向いている）から見て、`to` はどの方向か。
pub fn threat_arc(facing: Brad, from: Vec2Fx, to: Vec2Fx, cfg: &PerceptionConfig) -> ThreatArc {
    let bearing = to.sub(from).angle();
    let delta = angle_diff(facing, bearing).unsigned_abs();
    if delta <= cfg.front_half_arc_brad {
        ThreatArc::Front
    } else if delta <= cfg.flank_half_arc_brad {
        ThreatArc::Flank
    } else {
        ThreatArc::Rear
    }
}

/// `mover` が `toward` へ近づいているか。速度の向きで判定するので、こちらへ
/// 向かってくる敵と、通り過ぎていく敵を区別できる。
pub fn is_closing(soldiers: &Soldiers, mover: usize, toward: Vec2Fx) -> bool {
    let vel = Vec2Fx::new(
        soldiers.hot.vel_x[mover] as Fx,
        soldiers.hot.vel_y[mover] as Fx,
    );
    if vel.x == 0 && vel.y == 0 {
        // 止まっている敵は「離れていく」ではない。持ち場の判断では侵入者と
        // 同じに扱う。
        return true;
    }
    let to_target = toward.sub(soldiers.pos(mover));
    (vel.x as i64) * (to_target.x as i64) + (vel.y as i64) * (to_target.y as i64) > 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soldiers::Attrs;
    use sim_math::{fx, BRAD_HALF, BRAD_QUARTER};

    fn soldier_at(soldiers: &mut Soldiers, x: i32, y: i32, faction: u8, facing: Brad) -> SoldierId {
        soldiers.push(fx(x), fx(y), facing, 0, faction, Attrs::default(), 0)
    }

    /// 正面・側面・背面の切り分け。兵士は +X（brad 0）を向いている。
    #[test]
    fn threat_arcs_split_front_flank_and_rear() {
        let cfg = PerceptionConfig::default();
        let me = Vec2Fx::new(fx(100), fx(100));
        assert_eq!(
            threat_arc(0, me, Vec2Fx::new(fx(110), fx(100)), &cfg),
            ThreatArc::Front
        );
        assert_eq!(
            threat_arc(0, me, Vec2Fx::new(fx(100), fx(110)), &cfg),
            ThreatArc::Flank
        );
        assert_eq!(
            threat_arc(0, me, Vec2Fx::new(fx(90), fx(100)), &cfg),
            ThreatArc::Rear
        );
        // 向きが変われば同じ敵が別の方向になる。
        assert_eq!(
            threat_arc(BRAD_HALF as Brad, me, Vec2Fx::new(fx(90), fx(100)), &cfg),
            ThreatArc::Front
        );
    }

    #[test]
    fn awareness_counts_neighbours_and_finds_the_nearest_enemy() {
        let mut soldiers = Soldiers::default();
        let me = soldier_at(&mut soldiers, 100, 100, 0, 0);
        soldier_at(&mut soldiers, 101, 100, 0, 0);
        soldier_at(&mut soldiers, 102, 100, 0, 0);
        let near_enemy = soldier_at(&mut soldiers, 104, 100, 1, BRAD_HALF as Brad);
        soldier_at(&mut soldiers, 108, 100, 1, BRAD_HALF as Brad);

        let mut perception = PerceptionSystem::default();
        perception.tick(0, &mut soldiers);
        let a = perception.awareness(me);
        assert_eq!(a.allies, 2);
        assert_eq!(a.enemies, 2);
        assert_eq!(a.nearest_enemy, near_enemy);
        assert!(a.nearest_enemy_mm >= 3_900 && a.nearest_enemy_mm <= 4_100);
        assert_eq!(a.threat_front, 2, "正面の敵を正面として数えていない");
        assert_eq!(a.crowding, 1, "1.5 m 以内の味方だけを密集として数える");
    }

    /// 「近くに敵がいる」と「近くで味方が戦っている」は別の事実。
    #[test]
    fn supportable_fights_are_separate_from_merely_seeing_an_enemy() {
        let mut soldiers = Soldiers::default();
        let me = soldier_at(&mut soldiers, 100, 100, 0, 0);
        let lone_enemy = soldier_at(&mut soldiers, 105, 100, 1, BRAD_HALF as Brad);
        let mut perception = PerceptionSystem::default();
        perception.tick(0, &mut soldiers);
        let a = perception.awareness(me);
        assert_eq!(a.nearest_enemy, lone_enemy);
        assert!(
            !a.can_support_fight(),
            "誰も戦っていないのに援護できる戦闘があることになっている"
        );

        // 味方が敵と組み合ったら、その相手が援護対象になる。
        let comrade = soldier_at(&mut soldiers, 103, 100, 0, 0);
        let engaged_enemy = soldier_at(&mut soldiers, 104, 100, 1, BRAD_HALF as Brad);
        soldiers.hot.state[comrade as usize] = State::Engaged;
        soldiers.target[comrade as usize] = engaged_enemy;
        let mut perception = PerceptionSystem::default();
        perception.tick(0, &mut soldiers);
        let a = perception.awareness(me);
        assert!(a.can_support_fight());
        assert_eq!(a.supportable_enemy, engaged_enemy);
    }

    /// 背後に敵が現れた兵士は驚いて士気を落とす。気づいた最初の 1 回だけ。
    #[test]
    fn being_taken_from_behind_costs_morale_once() {
        let mut soldiers = Soldiers::default();
        let me = soldier_at(&mut soldiers, 100, 100, 0, 0);
        let mut perception = PerceptionSystem::default();
        perception.tick(0, &mut soldiers);
        let calm = soldiers.morale[me as usize];

        soldier_at(&mut soldiers, 96, 100, 1, 0);
        perception.tick(8, &mut soldiers);
        let startled = soldiers.morale[me as usize];
        assert!(startled < calm, "背後の敵に驚いていない");
        assert_eq!(perception.awareness(me).threat_rear, 1);

        perception.tick(16, &mut soldiers);
        assert_eq!(
            soldiers.morale[me as usize], startled,
            "同じ敵に何度も驚いている"
        );
    }

    /// 交戦中の兵士は周囲を警戒しているので、背後の敵に驚かない。
    #[test]
    fn fighting_soldiers_are_not_startled() {
        let mut soldiers = Soldiers::default();
        let me = soldier_at(&mut soldiers, 100, 100, 0, 0);
        soldiers.hot.state[me as usize] = State::Engaged;
        soldier_at(&mut soldiers, 96, 100, 1, 0);
        let before = soldiers.morale[me as usize];
        let mut perception = PerceptionSystem::default();
        perception.tick(0, &mut soldiers);
        assert_eq!(soldiers.morale[me as usize], before);
    }

    #[test]
    fn closing_enemies_are_told_apart_from_leaving_ones() {
        let mut soldiers = Soldiers::default();
        let post = Vec2Fx::new(fx(100), fx(100));
        let approaching = soldier_at(&mut soldiers, 120, 100, 1, 0);
        let leaving = soldier_at(&mut soldiers, 120, 100, 1, 0);
        soldiers.hot.vel_x[approaching as usize] = -fx_from_mm(60) as i16;
        soldiers.hot.vel_x[leaving as usize] = fx_from_mm(60) as i16;
        assert!(is_closing(&soldiers, approaching as usize, post));
        assert!(!is_closing(&soldiers, leaving as usize, post));
        // 止まっている敵は「離れていく」ではない。
        let standing = soldier_at(&mut soldiers, 120, 100, 1, 0);
        assert!(is_closing(&soldiers, standing as usize, post));
    }

    /// 更新は兵士ごとに位相がずれていて、全員が同じ tick に周囲を見ない。
    #[test]
    fn perception_updates_are_staggered() {
        let mut soldiers = Soldiers::default();
        for k in 0..16 {
            soldier_at(&mut soldiers, 100 + k, 100, 0, BRAD_QUARTER as Brad);
        }
        let mut perception = PerceptionSystem::default();
        // 既定値の `updated_tick` は 0 なので、0 以外の tick で数える。
        perception.tick(4, &mut soldiers);
        let updated = (0..soldiers.len() as u32)
            .filter(|&id| perception.awareness(id).updated_tick == 4)
            .count();
        assert!(updated > 0, "誰も周囲を見直していない");
        assert!(
            updated < soldiers.len(),
            "全員が同じ tick に周囲を見直している"
        );
    }
}
