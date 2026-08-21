//! 戦場での個体挙動を数える読み取り専用の観測器（B0 の観測基盤）。
//!
//! [`BattlefieldObserver`] は `&World` しか受け取らない。シミュレーションの
//! 状態を一切変更しないので、観測を挟んでも同一シードの状態ハッシュは変わらない
//! （`metrics_observation_does_not_change_the_simulation` が検査する）。
//!
//! 集計はすべて整数（mm・tick・permille）で行う。後続フェーズ（B1〜B6）の
//! 改善量と性能退行を、固定 seed の実行どうしで数値比較するための土台になる。
//!
//! 閾値はコードへ散らさず [`MetricsConfig`] の名前付きフィールドに置く。

use sim_math::{atan2_fx, dist_sq, fx_floor_int, fx_from_mm, fx_to_mm, isqrt64, Fx, Vec2Fx};

use crate::organization::{HuntState, Intent, MissionState};
use crate::soldier_ai::IndividualAction;
use crate::soldiers::{State, NO_ID};
use crate::World;

/// 観測の閾値。意味と単位を名前に持たせ、調整はここだけで済むようにする。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MetricsConfig {
    /// 「戦闘のそば」とみなす、交戦中の兵士からの距離（m）。
    pub combat_vicinity_m: i32,
    /// 「動き出した」とみなす速さ（mm/tick）。20 Hz なので 30 mm/tick = 0.6 m/s。
    pub move_start_speed_mm_per_tick: i32,
    /// 「止まった」とみなす速さ（mm/tick）。開始判定との差がヒステリシスになり、
    /// 押し合いで速度が上下するだけの兵士を「何度も動き出した」と数えない。
    pub move_stop_speed_mm_per_tick: i32,
    /// 「動き出した」と数えるまでに、止まっていたことを要求する tick 数。
    /// 白兵戦の間合い取りのような小刻みな前後を「命令への反応」と混同しない
    /// ため、これだけ静止していた兵士が動き出したときだけ 1 回と数える。
    pub move_settle_ticks: u32,
    /// 移動開始の方位を丸める分割数。8 なら 45 度ずつ。
    pub heading_sectors: u32,
    /// 移動開始 tick のヒストグラムの刻み（tick）と段数。
    pub start_histogram_bucket_ticks: u32,
    pub start_histogram_buckets: usize,
    /// 隊形スロットからの距離ヒストグラムの刻み（mm）と段数。分位点はここから読む。
    pub slot_distance_bucket_mm: i32,
    pub slot_distance_buckets: usize,
    /// 1 人の敵へ「群がりすぎ」とみなす味方の人数。
    pub target_crowd_limit: u32,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        MetricsConfig {
            combat_vicinity_m: 12,
            move_start_speed_mm_per_tick: 30,
            move_stop_speed_mm_per_tick: 10,
            move_settle_ticks: 20,
            heading_sectors: 8,
            // 5 tick 刻み × 192 段 = 960 tick。命令への反応が散る幅（4〜22 tick）を
            // 分解でき、既定の回帰シナリオ（最長 900 tick）も溢れずに収まる。
            start_histogram_bucket_ticks: 5,
            start_histogram_buckets: 192,
            slot_distance_bucket_mm: 250,
            slot_distance_buckets: 32,
            target_crowd_limit: 4,
        }
    }
}

/// 兵士が受けている任務の種類。`Intent` を観測用に平たくしたもの。
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MissionKind {
    /// どの部隊にも属していない（サンドボックスの単騎、工兵など）。
    #[default]
    NoUnit = 0,
    /// 部隊には属するが、まだ任務を受けていない。
    NoObjective = 1,
    MoveTo = 2,
    Hold = 3,
    Attack = 4,
    Charge = 5,
    Flank = 6,
    Envelop = 7,
    Screen = 8,
    Reserve = 9,
    Withdraw = 10,
    Pursue = 11,
    ShootAt = 12,
    HuntPerson = 13,
    OccupyArea = 14,
    GuardArea = 15,
}

impl MissionKind {
    pub const COUNT: usize = 16;

    pub fn of(objective: Option<&Intent>) -> MissionKind {
        match objective {
            None => MissionKind::NoObjective,
            Some(Intent::MoveTo { .. }) => MissionKind::MoveTo,
            Some(Intent::Hold { .. }) => MissionKind::Hold,
            Some(Intent::Attack { .. }) => MissionKind::Attack,
            Some(Intent::Charge { .. }) => MissionKind::Charge,
            Some(Intent::Flank { .. }) => MissionKind::Flank,
            Some(Intent::Envelop { .. }) => MissionKind::Envelop,
            Some(Intent::Screen { .. }) => MissionKind::Screen,
            Some(Intent::Reserve { .. }) => MissionKind::Reserve,
            Some(Intent::Withdraw { .. }) => MissionKind::Withdraw,
            Some(Intent::Pursue { .. }) => MissionKind::Pursue,
            Some(Intent::ShootAt { .. }) => MissionKind::ShootAt,
            Some(Intent::HuntPerson { .. }) => MissionKind::HuntPerson,
            Some(Intent::OccupyArea { .. }) => MissionKind::OccupyArea,
            Some(Intent::GuardArea { .. }) => MissionKind::GuardArea,
        }
    }

    /// JSON のキーに使う安定した識別子。
    pub fn id(self) -> &'static str {
        match self {
            MissionKind::NoUnit => "no_unit",
            MissionKind::NoObjective => "no_objective",
            MissionKind::MoveTo => "move_to",
            MissionKind::Hold => "hold",
            MissionKind::Attack => "attack",
            MissionKind::Charge => "charge",
            MissionKind::Flank => "flank",
            MissionKind::Envelop => "envelop",
            MissionKind::Screen => "screen",
            MissionKind::Reserve => "reserve",
            MissionKind::Withdraw => "withdraw",
            MissionKind::Pursue => "pursue",
            MissionKind::ShootAt => "shoot_at",
            MissionKind::HuntPerson => "hunt_person",
            MissionKind::OccupyArea => "occupy_area",
            MissionKind::GuardArea => "guard_area",
        }
    }

    pub fn from_index(index: usize) -> MissionKind {
        const ALL: [MissionKind; MissionKind::COUNT] = [
            MissionKind::NoUnit,
            MissionKind::NoObjective,
            MissionKind::MoveTo,
            MissionKind::Hold,
            MissionKind::Attack,
            MissionKind::Charge,
            MissionKind::Flank,
            MissionKind::Envelop,
            MissionKind::Screen,
            MissionKind::Reserve,
            MissionKind::Withdraw,
            MissionKind::Pursue,
            MissionKind::ShootAt,
            MissionKind::HuntPerson,
            MissionKind::OccupyArea,
            MissionKind::GuardArea,
        ];
        ALL[index.min(MissionKind::COUNT - 1)]
    }
}

/// 任務ごとの行動人数（観測した最後の tick 時点）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MissionMetrics {
    /// 生存している人数。
    pub soldiers: u32,
    /// 個人行動ごとの人数（`IndividualAction` の列挙順）。
    pub actions: [u32; IndividualAction::COUNT],
    /// 実際に白兵戦・突撃中の人数。
    pub fighting: u32,
    /// 何もしていない（`Idle`）人数。
    pub idle: u32,
}

/// 移動開始のばらけ方。全員が同じ tick に同じ方向へ動き出していないかを見る。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MovementMetrics {
    /// 観測期間中に「止まっている→動いている」へ移った回数の合計。
    pub starts: u32,
    /// 1 tick に集中した移動開始の最大人数。
    pub peak_starts_in_one_tick: u32,
    /// 同じ tick・同じ方位セクタへ動き出した人数の合計（各 tick の最大群）。
    pub same_heading_starts: u32,
    /// `same_heading_starts / starts`（permille）。小さいほど判断が散っている。
    pub same_heading_permille: u32,
    /// 最初と最後の移動開始 tick。`starts == 0` なら両方 0。
    pub first_start_tick: u32,
    pub last_start_tick: u32,
    /// 移動開始 tick のヒストグラム（刻みは `start_histogram_bucket_ticks`）。
    /// 末尾の 1 段は溢れ。
    pub start_histogram: Vec<u32>,
}

/// 戦闘の近くにいるのに戦っていない兵士の量。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VicinityMetrics {
    /// 交戦中の兵士から `combat_vicinity_m` 以内にいる人数（最終 tick）。
    pub near_combat: u32,
    /// そのうち自分は戦っていない人数（最終 tick）。
    pub idle_near_combat: u32,
    /// 観測期間中の最大値。
    pub peak_idle_near_combat: u32,
    /// 観測期間の平均（tick あたり）。
    pub mean_idle_near_combat: u32,
}

/// 隊形スロットからの離れ方。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FormationMetrics {
    /// 部隊に属している生存兵の人数（最終 tick）。
    pub in_units: u32,
    pub slot_distance_mean_mm: i32,
    /// ヒストグラムから読む 90 パーセンタイル（バケットの上端 mm）。
    pub slot_distance_p90_mm: i32,
    pub slot_distance_max_mm: i32,
    /// 観測期間中の最大。
    pub peak_slot_distance_max_mm: i32,
    /// この tick に、進もうとしている向きと逆へ押し戻された延べ回数（B4）。
    /// 操舵・隊形アンカー・衝突解決が綱引きしている量の目安。
    pub push_conflicts: u32,
    pub peak_push_conflicts: u32,
}

/// 1 人の敵へ何人が向かっているか。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TargetingMetrics {
    /// 白兵戦の相手が決まっている人数（最終 tick）。
    pub engaged_soldiers: u32,
    /// 誰かに狙われている敵の数（白兵の相手＋個人判断の追跡先）。
    pub focused_enemies: u32,
    pub max_attackers_per_enemy: u32,
    pub peak_max_attackers_per_enemy: u32,
    /// `target_crowd_limit` を超えて群がられている敵の数（最終 tick）。
    pub crowded_enemies: u32,
}

/// 局所知覚（B1）が何を捉えているか。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AwarenessMetrics {
    /// 視界内に敵がいる人数。
    pub sees_enemy: u32,
    /// 側面・背面から脅威を受けている人数。
    pub threatened_from_flank: u32,
    pub threatened_from_rear: u32,
    /// 援護に入れる味方の戦闘が見えている人数。
    pub can_support_fight: u32,
    /// 実際にその戦闘へ出ている人数。
    pub supporting: u32,
    /// 視界内の敵の平均人数（見えている兵士あたり、permille）。
    pub enemies_seen_mean_permille: u32,
}

/// 小集団と、戦闘参加の広がり方（B3）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SquadMetrics {
    /// 構成員のいる小集団の数。
    pub squads: u32,
    /// 平均人数（permille）。
    pub mean_size_permille: u32,
    /// 誰かが交戦・迎撃している小集団の数。
    pub engaged_squads: u32,
    /// 観測期間中、1 tick に迎撃へ移った最大人数。小さいほど「一斉突入」から
    /// 遠い（B3 の受け入れ条件）。
    pub peak_joins_in_one_tick: u32,
    /// 迎撃へ移った延べ人数。
    pub joins: u32,
}

/// 任務の段階ごとのノード数（B5）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MissionStateMetrics {
    pub idle: u32,
    pub moving: u32,
    pub deploying: u32,
    pub engaged: u32,
    pub secured: u32,
    pub failed: u32,
    pub returning: u32,
}

/// 追跡任務の進み方（B1 の記憶、B5 の状態機械の下地）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HuntMetrics {
    pub tracking: u32,
    pub searching: u32,
    pub lost: u32,
    pub target_down: u32,
}

/// 地点占拠・地点防衛の達成度。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AreaMetrics {
    pub occupy_assigned: u32,
    pub occupy_inside: u32,
    /// `occupy_inside / occupy_assigned`（permille）。
    pub occupy_permille: u32,
    pub guard_assigned: u32,
    /// 守備範囲（`radius_m`）の外にいる人数。
    pub guard_outside_post: u32,
    /// 迎撃範囲（`intercept_radius_m`）まで出てしまった人数。
    pub guard_beyond_leash: u32,
    pub peak_guard_beyond_leash: u32,
}

/// 観測結果。同一 seed・同一入力なら毎回完全に一致する。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetricsReport {
    pub ticks: u32,
    pub soldiers: u32,
    pub alive: u32,
    pub mission: Vec<MissionMetrics>,
    pub movement: MovementMetrics,
    pub vicinity: VicinityMetrics,
    pub formation: FormationMetrics,
    pub targeting: TargetingMetrics,
    pub area: AreaMetrics,
    pub awareness: AwarenessMetrics,
    pub squad: SquadMetrics,
    pub mission_state: MissionStateMetrics,
    pub hunt: HuntMetrics,
}

impl MetricsReport {
    /// 機械可読な JSON。数値はすべて整数（mm・tick・permille）。
    pub fn to_json(&self) -> String {
        use core::fmt::Write;
        let mut s = String::with_capacity(1024);
        let _ = write!(
            s,
            "{{\"ticks\":{},\"soldiers\":{},\"alive\":{}",
            self.ticks, self.soldiers, self.alive
        );
        let _ = write!(s, ",\"mission\":{{");
        let mut first = true;
        for (index, m) in self.mission.iter().enumerate() {
            if m.soldiers == 0 {
                continue;
            }
            if !first {
                let _ = write!(s, ",");
            }
            first = false;
            let _ = write!(
                s,
                "\"{}\":{{\"soldiers\":{},\"fighting\":{},\"idle\":{},\"actions\":{{",
                MissionKind::from_index(index).id(),
                m.soldiers,
                m.fighting,
                m.idle
            );
            let mut first_action = true;
            for (action_index, &count) in m.actions.iter().enumerate() {
                if count == 0 {
                    continue;
                }
                if !first_action {
                    let _ = write!(s, ",");
                }
                first_action = false;
                let _ = write!(
                    s,
                    "\"{}\":{count}",
                    IndividualAction::from_index(action_index).id()
                );
            }
            let _ = write!(s, "}}}}");
        }
        let _ = write!(s, "}}");

        let mv = &self.movement;
        let _ = write!(
            s,
            ",\"movement\":{{\"starts\":{},\"peak_starts_in_one_tick\":{},\
             \"same_heading_starts\":{},\"same_heading_permille\":{},\
             \"first_start_tick\":{},\"last_start_tick\":{},\"start_histogram\":[",
            mv.starts,
            mv.peak_starts_in_one_tick,
            mv.same_heading_starts,
            mv.same_heading_permille,
            mv.first_start_tick,
            mv.last_start_tick
        );
        for (i, v) in mv.start_histogram.iter().enumerate() {
            let _ = write!(s, "{}{}", if i == 0 { "" } else { "," }, v);
        }
        let _ = write!(s, "]}}");

        let v = &self.vicinity;
        let _ = write!(
            s,
            ",\"vicinity\":{{\"near_combat\":{},\"idle_near_combat\":{},\
             \"peak_idle_near_combat\":{},\"mean_idle_near_combat\":{}}}",
            v.near_combat, v.idle_near_combat, v.peak_idle_near_combat, v.mean_idle_near_combat
        );

        let f = &self.formation;
        let _ = write!(
            s,
            ",\"formation\":{{\"in_units\":{},\"slot_distance_mean_mm\":{},\
             \"slot_distance_p90_mm\":{},\"slot_distance_max_mm\":{},\
             \"peak_slot_distance_max_mm\":{},\"push_conflicts\":{},\
             \"peak_push_conflicts\":{}}}",
            f.in_units,
            f.slot_distance_mean_mm,
            f.slot_distance_p90_mm,
            f.slot_distance_max_mm,
            f.peak_slot_distance_max_mm,
            f.push_conflicts,
            f.peak_push_conflicts
        );

        let t = &self.targeting;
        let _ = write!(
            s,
            ",\"targeting\":{{\"engaged_soldiers\":{},\"focused_enemies\":{},\
             \"max_attackers_per_enemy\":{},\"peak_max_attackers_per_enemy\":{},\
             \"crowded_enemies\":{}}}",
            t.engaged_soldiers,
            t.focused_enemies,
            t.max_attackers_per_enemy,
            t.peak_max_attackers_per_enemy,
            t.crowded_enemies
        );

        let a = &self.area;
        let _ = write!(
            s,
            ",\"area\":{{\"occupy_assigned\":{},\"occupy_inside\":{},\"occupy_permille\":{},\
             \"guard_assigned\":{},\"guard_outside_post\":{},\"guard_beyond_leash\":{},\
             \"peak_guard_beyond_leash\":{}}}",
            a.occupy_assigned,
            a.occupy_inside,
            a.occupy_permille,
            a.guard_assigned,
            a.guard_outside_post,
            a.guard_beyond_leash,
            a.peak_guard_beyond_leash
        );

        let w = &self.awareness;
        let _ = write!(
            s,
            ",\"awareness\":{{\"sees_enemy\":{},\"threatened_from_flank\":{},\
             \"threatened_from_rear\":{},\"can_support_fight\":{},\"supporting\":{},\
             \"enemies_seen_mean_permille\":{}}}",
            w.sees_enemy,
            w.threatened_from_flank,
            w.threatened_from_rear,
            w.can_support_fight,
            w.supporting,
            w.enemies_seen_mean_permille
        );

        let q = &self.squad;
        let _ = write!(
            s,
            ",\"squad\":{{\"squads\":{},\"mean_size_permille\":{},\"engaged_squads\":{},\
             \"peak_joins_in_one_tick\":{},\"joins\":{}}}",
            q.squads, q.mean_size_permille, q.engaged_squads, q.peak_joins_in_one_tick, q.joins
        );

        let ms = &self.mission_state;
        let _ = write!(
            s,
            ",\"mission_state\":{{\"idle\":{},\"moving\":{},\"deploying\":{},\"engaged\":{},\
             \"secured\":{},\"failed\":{},\"returning\":{}}}",
            ms.idle, ms.moving, ms.deploying, ms.engaged, ms.secured, ms.failed, ms.returning
        );

        let h = &self.hunt;
        let _ = write!(
            s,
            ",\"hunt\":{{\"tracking\":{},\"searching\":{},\"lost\":{},\"target_down\":{}}}}}",
            h.tracking, h.searching, h.lost, h.target_down
        );
        s
    }
}

/// tick ごとに `&World` を眺めて指標を積み上げる観測器。
///
/// 状態を持つのは「前 tick に動いていたか」などの差分を取るためだけで、
/// ワールドへは書き戻さない。
#[derive(Debug)]
pub struct BattlefieldObserver {
    cfg: MetricsConfig,
    ticks: u32,
    /// 前 tick に「動いている」と判定していたか（移動開始の検出用）。
    moving: Vec<bool>,
    /// 連続して止まっている tick 数。`move_settle_ticks` に達してから動き出した
    /// ものだけを「移動開始」と数える。
    stopped_ticks: Vec<u32>,
    /// 作業用（毎 tick 再利用してアロケーションを避ける）。
    mission: Vec<MissionKind>,
    attackers: Vec<u32>,
    heading_counts: Vec<u32>,
    slot_histogram: Vec<u32>,
    grid: VicinityGrid,

    starts: u32,
    peak_starts_in_one_tick: u32,
    same_heading_starts: u32,
    first_start_tick: Option<u32>,
    last_start_tick: u32,
    start_histogram: Vec<u32>,

    peak_idle_near_combat: u32,
    idle_near_combat_sum: u64,
    peak_slot_distance_max_mm: i32,
    peak_push_conflicts: u32,
    peak_max_attackers_per_enemy: u32,
    peak_guard_beyond_leash: u32,
    /// 前 tick までに迎撃へ移った延べ人数（差分から 1 tick の人数を出す）。
    last_joins: u32,
    peak_joins_in_one_tick: u32,

    last: MetricsReport,
}

impl Default for BattlefieldObserver {
    fn default() -> Self {
        BattlefieldObserver::new(MetricsConfig::default())
    }
}

impl BattlefieldObserver {
    pub fn new(cfg: MetricsConfig) -> Self {
        let buckets = cfg.start_histogram_buckets.max(1);
        BattlefieldObserver {
            cfg,
            ticks: 0,
            moving: Vec::new(),
            stopped_ticks: Vec::new(),
            mission: Vec::new(),
            attackers: Vec::new(),
            heading_counts: vec![0; cfg.heading_sectors.max(1) as usize],
            slot_histogram: vec![0; cfg.slot_distance_buckets.max(1)],
            grid: VicinityGrid::default(),
            starts: 0,
            peak_starts_in_one_tick: 0,
            same_heading_starts: 0,
            first_start_tick: None,
            last_start_tick: 0,
            start_histogram: vec![0; buckets],
            peak_idle_near_combat: 0,
            idle_near_combat_sum: 0,
            peak_slot_distance_max_mm: 0,
            peak_push_conflicts: 0,
            peak_max_attackers_per_enemy: 0,
            peak_guard_beyond_leash: 0,
            last_joins: 0,
            peak_joins_in_one_tick: 0,
            last: MetricsReport::default(),
        }
    }

    pub fn config(&self) -> &MetricsConfig {
        &self.cfg
    }

    /// 1 tick ぶんを観測する。`world` は読むだけ。
    pub fn observe(&mut self, world: &World) {
        let n = world.soldiers.len();
        self.moving.resize(n, false);
        // 初めて見る兵士は「ずっと止まっていた」ものとして数え始める。そうしないと
        // 観測開始直後の第一波——命令へ反応して動き出す最初の集団——を取り逃がす。
        self.stopped_ticks.resize(n, self.cfg.move_settle_ticks);
        self.mission.resize(n, MissionKind::NoUnit);
        self.attackers.resize(n, 0);
        self.mission[..n].fill(MissionKind::NoUnit);
        self.attackers[..n].fill(0);
        self.ticks = self.ticks.saturating_add(1);

        self.collect_missions(world, n);
        let movement = self.collect_movement(world, n);
        let vicinity = self.collect_vicinity(world, n);
        let (mission, alive) = self.collect_mission_actions(world, n);
        let formation = self.collect_formation(world, n);
        let targeting = self.collect_targeting(world, n);
        let area = self.collect_area(world);
        let awareness = self.collect_awareness(world, n);
        let squad = self.collect_squads(world);
        let mission_state = collect_mission_states(world);
        let hunt = collect_hunt(world);

        self.last = MetricsReport {
            ticks: self.ticks,
            soldiers: n as u32,
            alive,
            mission,
            movement,
            vicinity,
            formation,
            targeting,
            area,
            awareness,
            squad,
            mission_state,
            hunt,
        };
    }

    /// ここまでに観測した内容の報告。
    pub fn report(&self) -> MetricsReport {
        self.last.clone()
    }

    /// 兵士ごとの任務種別（デバッグ表示にも使う）。
    fn collect_missions(&mut self, world: &World, n: usize) {
        for node in &world.command.nodes {
            let Some(unit) = &node.unit else {
                continue;
            };
            let kind = MissionKind::of(node.objective.as_ref());
            for &id in &unit.soldiers {
                let i = id as usize;
                if i < n {
                    self.mission[i] = kind;
                }
            }
        }
    }

    fn collect_movement(&mut self, world: &World, n: usize) -> MovementMetrics {
        let start_speed = fx_from_mm(self.cfg.move_start_speed_mm_per_tick) as i64;
        let stop_speed = fx_from_mm(self.cfg.move_stop_speed_mm_per_tick) as i64;
        let sectors = self.cfg.heading_sectors.max(1);
        self.heading_counts.resize(sectors as usize, 0);
        self.heading_counts.fill(0);

        let mut starts_this_tick = 0u32;
        for i in 0..n {
            let vx = world.soldiers.hot.vel_x[i] as i64;
            let vy = world.soldiers.hot.vel_y[i] as i64;
            let speed_sq = vx * vx + vy * vy;
            if !world.soldiers.is_alive(i) {
                self.moving[i] = false;
                self.stopped_ticks[i] = 0;
                continue;
            }
            if self.moving[i] {
                if speed_sq < stop_speed * stop_speed {
                    self.moving[i] = false;
                    self.stopped_ticks[i] = 1;
                }
                continue;
            }
            if speed_sq < start_speed * start_speed {
                self.stopped_ticks[i] = self.stopped_ticks[i].saturating_add(1);
                continue;
            }
            self.moving[i] = true;
            // 十分に落ち着いていた兵士が動き出したときだけ「移動開始」と数える。
            if self.stopped_ticks[i] < self.cfg.move_settle_ticks {
                self.stopped_ticks[i] = 0;
                continue;
            }
            self.stopped_ticks[i] = 0;
            starts_this_tick += 1;
            let heading = atan2_fx(vy as Fx, vx as Fx);
            let sector = (heading as u32 * sectors) / 65_536;
            let slot = &mut self.heading_counts[(sector as usize).min(sectors as usize - 1)];
            *slot += 1;
        }

        if starts_this_tick > 0 {
            self.starts = self.starts.saturating_add(starts_this_tick);
            self.peak_starts_in_one_tick = self.peak_starts_in_one_tick.max(starts_this_tick);
            let largest = self.heading_counts.iter().copied().max().unwrap_or(0);
            self.same_heading_starts = self.same_heading_starts.saturating_add(largest);
            if self.first_start_tick.is_none() {
                self.first_start_tick = Some(world.tick);
            }
            self.last_start_tick = world.tick;
            let bucket = (world.tick / self.cfg.start_histogram_bucket_ticks.max(1)) as usize;
            let last = self.start_histogram.len() - 1;
            self.start_histogram[bucket.min(last)] += starts_this_tick;
        }

        MovementMetrics {
            starts: self.starts,
            peak_starts_in_one_tick: self.peak_starts_in_one_tick,
            same_heading_starts: self.same_heading_starts,
            same_heading_permille: permille(self.same_heading_starts, self.starts),
            first_start_tick: self.first_start_tick.unwrap_or(0),
            last_start_tick: self.last_start_tick,
            start_histogram: self.start_histogram.clone(),
        }
    }

    fn collect_vicinity(&mut self, world: &World, n: usize) -> VicinityMetrics {
        self.grid.rebuild(world, self.cfg.combat_vicinity_m);
        let radius = fx_from_mm(self.cfg.combat_vicinity_m.saturating_mul(1_000));
        let mut near = 0u32;
        let mut idle_near = 0u32;
        for i in 0..n {
            if !world.soldiers.is_alive(i) {
                continue;
            }
            if !self
                .grid
                .fighter_within(world, world.soldiers.pos(i), i, radius)
            {
                continue;
            }
            near += 1;
            if !is_committed_to_combat(world.soldiers.hot.state[i]) {
                idle_near += 1;
            }
        }
        self.peak_idle_near_combat = self.peak_idle_near_combat.max(idle_near);
        self.idle_near_combat_sum += idle_near as u64;
        VicinityMetrics {
            near_combat: near,
            idle_near_combat: idle_near,
            peak_idle_near_combat: self.peak_idle_near_combat,
            mean_idle_near_combat: (self.idle_near_combat_sum / self.ticks.max(1) as u64) as u32,
        }
    }

    fn collect_mission_actions(&mut self, world: &World, n: usize) -> (Vec<MissionMetrics>, u32) {
        let mut mission = vec![MissionMetrics::default(); MissionKind::COUNT];
        let mut alive = 0u32;
        for i in 0..n {
            if !world.soldiers.is_alive(i) {
                continue;
            }
            alive += 1;
            let m = &mut mission[self.mission[i] as usize];
            m.soldiers += 1;
            m.actions[world.soldier_ai.action(i as u32) as usize] += 1;
            let state = world.soldiers.hot.state[i];
            if state.is_fighting() {
                m.fighting += 1;
            }
            if state == State::Idle {
                m.idle += 1;
            }
        }
        (mission, alive)
    }

    fn collect_formation(&mut self, world: &World, n: usize) -> FormationMetrics {
        let bucket_mm = self.cfg.slot_distance_bucket_mm.max(1);
        self.slot_histogram
            .resize(self.cfg.slot_distance_buckets.max(1), 0);
        self.slot_histogram.fill(0);
        let mut in_units = 0u32;
        let mut sum_mm = 0i64;
        let mut max_mm = 0i32;
        for i in 0..n {
            if !world.soldiers.is_alive(i) || self.mission[i] == MissionKind::NoUnit {
                continue;
            }
            let slot = world.soldier_ai.formation_goal(i as u32);
            let mm = distance_mm(world.soldiers.pos(i), slot);
            in_units += 1;
            sum_mm += mm as i64;
            max_mm = max_mm.max(mm);
            let bucket = (mm / bucket_mm) as usize;
            let last = self.slot_histogram.len() - 1;
            self.slot_histogram[bucket.min(last)] += 1;
        }
        self.peak_slot_distance_max_mm = self.peak_slot_distance_max_mm.max(max_mm);
        self.peak_push_conflicts = self.peak_push_conflicts.max(world.push_conflicts);
        FormationMetrics {
            in_units,
            slot_distance_mean_mm: if in_units == 0 {
                0
            } else {
                (sum_mm / in_units as i64) as i32
            },
            slot_distance_p90_mm: histogram_quantile_mm(&self.slot_histogram, bucket_mm, 900),
            slot_distance_max_mm: max_mm,
            peak_slot_distance_max_mm: self.peak_slot_distance_max_mm,
            push_conflicts: world.push_conflicts,
            peak_push_conflicts: self.peak_push_conflicts,
        }
    }

    fn collect_targeting(&mut self, world: &World, n: usize) -> TargetingMetrics {
        let mut engaged = 0u32;
        for i in 0..n {
            if !world.soldiers.is_alive(i) {
                continue;
            }
            let melee = world.soldiers.target[i];
            if melee != NO_ID {
                engaged += 1;
                if let Some(t) = world.soldiers.index_if_present(melee) {
                    self.attackers[t] = self.attackers[t].saturating_add(1);
                }
            }
            if let Some(focus) = world.soldier_ai.focus(i as u32) {
                if focus != melee {
                    if let Some(t) = world.soldiers.index_if_present(focus) {
                        self.attackers[t] = self.attackers[t].saturating_add(1);
                    }
                }
            }
        }
        let mut focused = 0u32;
        let mut max_attackers = 0u32;
        let mut crowded = 0u32;
        for i in 0..n {
            let count = self.attackers[i];
            if count == 0 {
                continue;
            }
            focused += 1;
            max_attackers = max_attackers.max(count);
            if count > self.cfg.target_crowd_limit {
                crowded += 1;
            }
        }
        self.peak_max_attackers_per_enemy = self.peak_max_attackers_per_enemy.max(max_attackers);
        TargetingMetrics {
            engaged_soldiers: engaged,
            focused_enemies: focused,
            max_attackers_per_enemy: max_attackers,
            peak_max_attackers_per_enemy: self.peak_max_attackers_per_enemy,
            crowded_enemies: crowded,
        }
    }

    /// 局所知覚が何を捉えているか（B1）。知覚そのものは `PerceptionSystem` が
    /// 更新済みで、ここは数えるだけ。
    fn collect_awareness(&mut self, world: &World, n: usize) -> AwarenessMetrics {
        let mut m = AwarenessMetrics::default();
        let mut enemies_seen = 0u64;
        for i in 0..n {
            if !world.soldiers.is_alive(i) {
                continue;
            }
            let a = world.perception.awareness(i as u32);
            if a.sees_enemy() {
                m.sees_enemy += 1;
                enemies_seen += a.enemies as u64;
            }
            if a.threat_flank > 0 {
                m.threatened_from_flank += 1;
            }
            if a.threat_rear > 0 {
                m.threatened_from_rear += 1;
            }
            if a.can_support_fight() {
                m.can_support_fight += 1;
                if world.soldier_ai.focus(i as u32) == Some(a.supportable_enemy) {
                    m.supporting += 1;
                }
            }
        }
        m.enemies_seen_mean_permille = if m.sees_enemy == 0 {
            0
        } else {
            ((enemies_seen * 1_000) / m.sees_enemy as u64) as u32
        };
        m
    }

    /// 小集団の様子と、迎撃へ移る人数の広がり（B3）。
    fn collect_squads(&mut self, world: &World) -> SquadMetrics {
        let mut m = SquadMetrics::default();
        let mut members = 0u64;
        for squad in world.squads.squads() {
            if squad.members.is_empty() {
                continue;
            }
            m.squads += 1;
            members += squad.members.len() as u64;
            if squad.engaged > 0 {
                m.engaged_squads += 1;
            }
        }
        m.mean_size_permille = if m.squads == 0 {
            0
        } else {
            ((members * 1_000) / m.squads as u64) as u32
        };
        // 迎撃へ移った延べ人数の差分が、その tick に出た人数。
        let joins = world.soldier_ai.stats.joins;
        let new_joins = joins.saturating_sub(self.last_joins);
        self.last_joins = joins;
        self.peak_joins_in_one_tick = self.peak_joins_in_one_tick.max(new_joins);
        m.joins = joins;
        m.peak_joins_in_one_tick = self.peak_joins_in_one_tick;
        m
    }

    fn collect_area(&mut self, world: &World) -> AreaMetrics {
        let mut area = AreaMetrics::default();
        for node in &world.command.nodes {
            let Some(unit) = &node.unit else {
                continue;
            };
            match node.objective {
                Some(Intent::OccupyArea { center, radius_m }) => {
                    let radius = fx_from_mm(i32::from(radius_m).saturating_mul(1_000)) as i64;
                    for &id in &unit.soldiers {
                        let Some(i) = world.soldiers.index_if_present(id) else {
                            continue;
                        };
                        if !world.soldiers.is_alive(i) {
                            continue;
                        }
                        area.occupy_assigned += 1;
                        if dist_sq(world.soldiers.pos(i), center) <= radius * radius {
                            area.occupy_inside += 1;
                        }
                    }
                }
                Some(Intent::GuardArea {
                    center,
                    radius_m,
                    intercept_radius_m,
                }) => {
                    let post = fx_from_mm(i32::from(radius_m).saturating_mul(1_000)) as i64;
                    let leash =
                        fx_from_mm(i32::from(intercept_radius_m).saturating_mul(1_000)) as i64;
                    for &id in &unit.soldiers {
                        let Some(i) = world.soldiers.index_if_present(id) else {
                            continue;
                        };
                        if !world.soldiers.is_alive(i) {
                            continue;
                        }
                        area.guard_assigned += 1;
                        let d2 = dist_sq(world.soldiers.pos(i), center);
                        if d2 > post * post {
                            area.guard_outside_post += 1;
                        }
                        if d2 > leash * leash {
                            area.guard_beyond_leash += 1;
                        }
                    }
                }
                _ => {}
            }
        }
        area.occupy_permille = permille(area.occupy_inside, area.occupy_assigned);
        self.peak_guard_beyond_leash = self.peak_guard_beyond_leash.max(area.guard_beyond_leash);
        area.peak_guard_beyond_leash = self.peak_guard_beyond_leash;
        area
    }
}

/// 任務の段階ごとのノード数（B5）。
fn collect_mission_states(world: &World) -> MissionStateMetrics {
    let mut m = MissionStateMetrics::default();
    for node in &world.command.nodes {
        if node.unit.is_none() {
            continue;
        }
        match node.mission.state {
            MissionState::Idle => m.idle += 1,
            MissionState::Moving => m.moving += 1,
            MissionState::Deploying => m.deploying += 1,
            MissionState::Engaged => m.engaged += 1,
            MissionState::Secured => m.secured += 1,
            MissionState::Failed => m.failed += 1,
            MissionState::Returning => m.returning += 1,
        }
    }
    m
}

/// 追跡任務がいまどの段階にあるか（B1）。
fn collect_hunt(world: &World) -> HuntMetrics {
    let mut m = HuntMetrics::default();
    for node in &world.command.nodes {
        let Some(hunt) = node.hunt else {
            continue;
        };
        match hunt.state {
            HuntState::Tracking => m.tracking += 1,
            HuntState::Searching => m.searching += 1,
            HuntState::Lost => m.lost += 1,
            HuntState::TargetDown => m.target_down += 1,
        }
    }
    m
}

/// 「自分も戦闘に加わっている」とみなす状態。ここに入らない兵士が戦闘の
/// そばにいるなら、参加・援護・持ち場の維持・退避のどれでもない待機である。
fn is_committed_to_combat(state: State) -> bool {
    matches!(
        state,
        State::Engaged
            | State::Charging
            | State::Advancing
            | State::Shooting
            | State::Reloading
            | State::Broken
            | State::Rallying
    )
}

fn distance_mm(a: Vec2Fx, b: Vec2Fx) -> i32 {
    fx_to_mm(isqrt64(dist_sq(a, b) as u64) as Fx)
}

fn permille(part: u32, whole: u32) -> u32 {
    if whole == 0 {
        0
    } else {
        ((part as u64 * 1_000) / whole as u64) as u32
    }
}

/// ヒストグラムから分位点をバケット上端の mm で読む。
fn histogram_quantile_mm(histogram: &[u32], bucket_mm: i32, permille_target: u64) -> i32 {
    let total: u64 = histogram.iter().map(|&v| v as u64).sum();
    if total == 0 {
        return 0;
    }
    let threshold = (total * permille_target).div_ceil(1_000);
    let mut acc = 0u64;
    for (index, &count) in histogram.iter().enumerate() {
        acc += count as u64;
        if acc >= threshold {
            return (index as i32 + 1).saturating_mul(bucket_mm);
        }
    }
    (histogram.len() as i32).saturating_mul(bucket_mm)
}

/// 交戦中の兵士だけを入れた粗いセル索引。
///
/// `spatial::CoarseIndex` は近傍数に上限（`MAX_NEIGHBORS`）があり、密集した
/// 混戦では数え落とす。ここは計測器なので上限を持たず、「半径内に交戦中の
/// 兵士がいるか」を最初の 1 人で打ち切って調べる。
#[derive(Debug, Default)]
struct VicinityGrid {
    cell_m: i32,
    origin_x: i32,
    origin_y: i32,
    cols: i32,
    rows: i32,
    cell_start: Vec<u32>,
    entries: Vec<u32>,
}

impl VicinityGrid {
    fn rebuild(&mut self, world: &World, cell_m: i32) {
        let cell_m = cell_m.max(1);
        self.cell_m = cell_m;
        self.cell_start.clear();
        self.entries.clear();
        self.cols = 0;
        self.rows = 0;

        let soldiers = &world.soldiers;
        let n = soldiers.len();
        let (mut min_x, mut min_y) = (i32::MAX, i32::MAX);
        let (mut max_x, mut max_y) = (i32::MIN, i32::MIN);
        let mut any = false;
        for i in 0..n {
            if !soldiers.is_alive(i) || !soldiers.hot.state[i].is_fighting() {
                continue;
            }
            any = true;
            min_x = min_x.min(soldiers.hot.pos_x[i]);
            min_y = min_y.min(soldiers.hot.pos_y[i]);
            max_x = max_x.max(soldiers.hot.pos_x[i]);
            max_y = max_y.max(soldiers.hot.pos_y[i]);
        }
        if !any {
            return;
        }
        self.origin_x = fx_floor_int(min_x).div_euclid(cell_m) - 1;
        self.origin_y = fx_floor_int(min_y).div_euclid(cell_m) - 1;
        self.cols = fx_floor_int(max_x).div_euclid(cell_m) + 1 - self.origin_x + 1;
        self.rows = fx_floor_int(max_y).div_euclid(cell_m) + 1 - self.origin_y + 1;
        let cells = (self.cols as usize) * (self.rows as usize);

        let mut counts = vec![0u32; cells + 1];
        for i in 0..n {
            if !soldiers.is_alive(i) || !soldiers.hot.state[i].is_fighting() {
                continue;
            }
            counts[self.cell_of(soldiers.hot.pos_x[i], soldiers.hot.pos_y[i])] += 1;
        }
        self.cell_start = vec![0u32; cells + 1];
        let mut acc = 0u32;
        for (slot, &count) in counts.iter().take(cells).enumerate() {
            self.cell_start[slot] = acc;
            acc += count;
        }
        self.cell_start[cells] = acc;
        self.entries = vec![0u32; acc as usize];
        let mut cursor = self.cell_start.clone();
        for i in 0..n {
            if !soldiers.is_alive(i) || !soldiers.hot.state[i].is_fighting() {
                continue;
            }
            let cell = self.cell_of(soldiers.hot.pos_x[i], soldiers.hot.pos_y[i]);
            self.entries[cursor[cell] as usize] = i as u32;
            cursor[cell] += 1;
        }
    }

    fn cell_of(&self, x: Fx, y: Fx) -> usize {
        let cx = (fx_floor_int(x).div_euclid(self.cell_m) - self.origin_x).clamp(0, self.cols - 1);
        let cy = (fx_floor_int(y).div_euclid(self.cell_m) - self.origin_y).clamp(0, self.rows - 1);
        (cy * self.cols + cx) as usize
    }

    /// `pos` から半径 `radius` 以内に、自分以外の交戦中の兵士がいるか。
    fn fighter_within(&self, world: &World, pos: Vec2Fx, self_index: usize, radius: Fx) -> bool {
        if self.cols == 0 || self.rows == 0 {
            return false;
        }
        let r2 = (radius as i64) * (radius as i64);
        let cx = (fx_floor_int(pos.x).div_euclid(self.cell_m) - self.origin_x).clamp(-1, self.cols);
        let cy = (fx_floor_int(pos.y).div_euclid(self.cell_m) - self.origin_y).clamp(-1, self.rows);
        for gy in (cy - 1).max(0)..=(cy + 1).min(self.rows - 1) {
            for gx in (cx - 1).max(0)..=(cx + 1).min(self.cols - 1) {
                let cell = (gy * self.cols + gx) as usize;
                let from = self.cell_start[cell] as usize;
                let to = self.cell_start[cell + 1] as usize;
                for &id in &self.entries[from..to] {
                    let other = id as usize;
                    if other == self_index {
                        continue;
                    }
                    if dist_sq(pos, world.soldiers.pos(other)) <= r2 {
                        return true;
                    }
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::organization::Unit;
    use crate::soldiers::Attrs;
    use sim_math::fx;
    use sim_terrain::Terrain;

    /// 守備隊 1 部隊だけを置いた小さなワールド。
    fn guarded_world() -> (World, Vec2Fx) {
        let mut world = World::with_terrain(9, Terrain::flat(64, 2, 9));
        let post = Vec2Fx::new(fx(60), fx(60));
        let ids: Vec<u32> = (0..4)
            .map(|k| {
                world.spawn(
                    Vec2Fx::new(post.x + fx(k), post.y),
                    0,
                    0,
                    0,
                    Attrs::default(),
                    0,
                )
            })
            .collect();
        let unit = Unit {
            soldiers: ids.clone(),
            troop_type: 0,
            formation: crate::organization::FORMATION_LINE,
            formation_origin: post,
            formation_facing: 0,
            ranks: 1,
            file_spacing: fx_from_mm(900),
            rank_spacing: fx_from_mm(900),
            banner: None,
            formation_change: None,
            path: Vec::new(),
            path_final: post,
            pursuit_leash: None,
        };
        let node = world.add_command_node(None, 0, 0, ids[0], vec![], Some(unit));
        world.set_objective(
            node,
            Intent::GuardArea {
                center: post,
                radius_m: 5,
                intercept_radius_m: 10,
            },
        );
        (world, post)
    }

    #[test]
    fn area_metrics_count_soldiers_that_left_their_post() {
        let (mut world, post) = guarded_world();
        let mut observer = BattlefieldObserver::default();
        world.tick();
        observer.observe(&world);
        let report = observer.report();
        assert_eq!(report.mission[MissionKind::GuardArea as usize].soldiers, 4);
        assert_eq!(report.area.guard_assigned, 4);
        assert_eq!(report.area.guard_outside_post, 0);
        assert_eq!(report.area.guard_beyond_leash, 0);

        // 1 人だけ持ち場から 20 m 離す。守備範囲（5 m）も迎撃範囲（10 m）も外。
        world
            .soldiers
            .set_pos(0, Vec2Fx::new(post.x, post.y + fx(20)));
        world.tick();
        observer.observe(&world);
        let report = observer.report();
        assert_eq!(report.area.guard_outside_post, 1);
        assert_eq!(report.area.guard_beyond_leash, 1);
        assert_eq!(report.area.peak_guard_beyond_leash, 1);
    }

    /// 観測は状態を読むだけで、同じワールドを 2 回観測しても値が変わらない。
    #[test]
    fn observing_the_same_tick_twice_is_a_pure_read() {
        let (mut world, _) = guarded_world();
        world.tick();
        let hash_before = world.state_hash();
        let mut a = BattlefieldObserver::default();
        let mut b = BattlefieldObserver::default();
        a.observe(&world);
        b.observe(&world);
        assert_eq!(a.report(), b.report());
        assert_eq!(world.state_hash(), hash_before);
    }

    /// 移動開始は「十分に止まっていた兵士が動き出した」ときだけ数える。
    #[test]
    fn movement_starts_count_the_response_to_an_order() {
        let (mut world, post) = guarded_world();
        let mut observer = BattlefieldObserver::default();
        // 守備隊は最初に区域内へ展開するので、まず落ち着くまで回す。
        for _ in 0..120 {
            world.tick();
            observer.observe(&world);
        }
        let settled = observer.report().movement.starts;
        for _ in 0..40 {
            world.tick();
            observer.observe(&world);
        }
        assert_eq!(
            observer.report().movement.starts,
            settled,
            "持ち場に立っているだけで移動開始と数えている"
        );
        world.set_objective(
            0,
            Intent::MoveTo {
                pos: Vec2Fx::new(post.x, post.y + fx(30)),
                facing: 0,
                speed: crate::organization::MoveSpeed::Walk,
                formation: crate::organization::FORMATION_LINE,
            },
        );
        for _ in 0..60 {
            world.tick();
            observer.observe(&world);
        }
        let movement = observer.report().movement;
        assert!(movement.starts > settled, "命令に反応した兵士がいない");
        assert!(
            movement.last_start_tick > 160,
            "反応が命令より前に起きている"
        );
    }

    #[test]
    fn quantile_reads_the_bucket_that_crosses_the_threshold() {
        // 10 人：0〜250 mm に 9 人、250〜500 mm に 1 人。90 % は 1 段目の上端。
        let histogram = [9u32, 1, 0, 0];
        assert_eq!(histogram_quantile_mm(&histogram, 250, 900), 250);
        assert_eq!(histogram_quantile_mm(&histogram, 250, 1_000), 500);
        assert_eq!(histogram_quantile_mm(&[], 250, 900), 0);
    }

    #[test]
    fn mission_kind_ids_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for index in 0..MissionKind::COUNT {
            assert!(seen.insert(MissionKind::from_index(index).id()));
        }
    }
}
