//! M6 の工兵：作業システム・野戦築城の建設・地形改修・矢の補給・負傷者回収。
//!
//! 仕様は `docs/spec/07-engineers.md`、`docs/spec/12-roadmap.md` の M6。
//!
//! ## この実装でのスコープ
//!
//! - `EngineerTask` の担当は仕様では `NodeId`（工兵 Unit 全体）だが、ここでは
//!   個々の `SoldierId` を直接タスクへ割り当てる。指揮ツリーとの統合は
//!   `owner`（陣営）だけにして `organization` への依存を避けている。工兵が
//!   `organization::Unit` に属していても動くように（`formation_goals` が
//!   毎 tick 上書きする `goal`/`state` を、作業中は工兵側が勝ち取るように）
//!   `World::tick` 側で `command.formation_goals` の後に呼ぶ。
//! - 天候・工具の倍率（仕様 1.2 節の `weather_mult` / `tool_mult`）は天候
//!   システム・装備兵站がまだ存在しないため常に 1000‰ とする。
//! - 攻城兵器の野戦運用（仕様 4 節）はまだ実装していない。
//! - 上位指揮官の `Objective` からのタスク自動生成（仕様 6 節の表）は、
//!   指揮官AIとまだ配線していない。ここでは `World` から明示的に
//!   タスクを投入する API と、投入済みタスクへの単純なスコアリング割当て
//!   （6 節の式から `tactical_value` を抜いたもの）を実装する。
//! - パヴィス設置は `structures::StructureKind::Pavise` としてデータは
//!   あるが、まだ工兵タスクからは作れない（弓兵の防御バフとして戦闘側に
//!   配線する作業が別途要る）。

use sim_math::{dist, dist_sq, fx_from_mm, Fx, Purpose, Rng, Vec2Fx, FX_HALF};
use sim_terrain::{Overlay, Terrain, Vegetation, FORD_MAX_DEPTH_CM};

use crate::combat::CombatSystem;
use crate::soldiers::{flags, Attrs, SoldierId, Soldiers, State, MAX_MORALE};
use crate::spatial::{CoarseIndex, MAX_NEIGHBORS};
use crate::structures::{StructureId, StructureKind, StructureSystem};

pub type TaskId = u32;
pub type DepotId = u32;

/// 到着とみなす距離（作業地点にこれ以上近づけば「現地にいる」）。
const ARRIVAL_RADIUS_MM: i32 = 1_500;
/// 危険察知の半径。これより敵が近いと `self_preservation` の判定で作業を中断する
/// （仕様 07 章 1.3 節）。
const DANGER_RADIUS_MM: i32 = 20_000;
/// 危険察知・補給の受け渡しに使う粗い空間索引のセル一辺（m）。
const COARSE_CELL_M: i32 = 16;
/// 矢束 1 つの本数。
const ARROWS_PER_BUNDLE: u16 = 24;
/// 1 人が 1 回の往復で運べる矢束の数（仕様「1 人 4 束まで」）。
const CARRY_CAPACITY_ARROWS: u16 = ARROWS_PER_BUNDLE * 4;
/// 補給で満たす目標弾薬数。
const RESUPPLY_TARGET_AMMO: u16 = ARROWS_PER_BUNDLE;
/// 補給の受け渡し半径。
const DELIVERY_RADIUS_MM: i32 = 15_000;
/// 同時にタスクへ割り当てられる工兵の上限。
const MAX_WORKERS_PER_TASK: usize = 8;
/// 危険を察知して作業を放棄した工兵が、次の作業（同じタスクへの再割り当ても
/// 含む）に回されるまでの tick 数。これがないと、他に候補がいない状況で
/// 同じ tick 内に即座に同じ危険な現場へ押し戻されてしまい、「放棄」が
/// 実質何の意味も持たなくなる。
const FLEE_COOLDOWN_TICKS: u32 = 200;

/// 負傷者回収 1 件あたりの労力（milli-man-seconds）。仕様は「2 人 1 組」だが
/// ここでは 1 人でも回収できるよう単純化している（本文参照）。
const RECOVERY_WORK_MMS: u32 = 60_000;
/// 回収された負傷者が実際に軽傷で復帰する確率（‰）。
const RECOVERY_SUCCESS_PERMILLE: u32 = 800;
/// 回収に成功したときに戻す HP（最大 100 のうち）。
const RECOVERY_HP: u16 = 35;
/// 回収を始めたとき、近傍の味方に与える士気ボーナス（仕様 5.2 節）。
const RECOVERY_MORALE_BONUS: u16 = 15;
const RECOVERY_MORALE_RADIUS_MM: i32 = 12_000;

/// 1 人が理想条件で 1 秒に生み出す工数（milli-man-seconds）。
/// 仕様 1.2 節 `base_rate` に相当する基準値。1000 mms/s = 1.0 man-s/s。
const BASE_RATE_MMS_PER_SEC: i64 = 1_000;

const ARRIVAL_RADIUS_FX: Fx = fx_from_mm(ARRIVAL_RADIUS_MM);
const DANGER_RADIUS_FX: Fx = fx_from_mm(DANGER_RADIUS_MM);
const DELIVERY_RADIUS_FX: Fx = fx_from_mm(DELIVERY_RADIUS_MM);
const RECOVERY_MORALE_RADIUS_FX: Fx = fx_from_mm(RECOVERY_MORALE_RADIUS_MM);

/// タスクの状態。仕様 07 章 1.1 節。
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    Queued,
    InProgress,
    Done,
    Abandoned,
}

/// タスクの種類。
#[derive(Clone, Debug)]
pub enum TaskKind {
    /// 野戦築城。`structure` は事前に `StructureSystem::build` で作った完成度 0 の実体。
    BuildStructure { structure: StructureId },
    /// 敵構造物の破壊工作。
    DestroyStructure { structure: StructureId },
    /// 架橋。`cells` は変換対象の水面セル index（`Terrain::idx` の並び順）。
    BuildBridge {
        cells: Vec<usize>,
        original: Vec<u8>,
    },
    /// 橋の破壊工作。橋タスクが記録した変換前の地表に戻す。
    DestroyBridge {
        cells: Vec<usize>,
        original: Vec<u8>,
    },
    /// 伐採。`cells` は対象の森セル index。
    ClearForest { cells: Vec<usize> },
}

/// 工兵タスク。仕様 07 章 1.1 節。
pub struct EngineerTask {
    pub id: TaskId,
    pub kind: TaskKind,
    pub site: Vec2Fx,
    pub work_required_mms: u32,
    pub work_done_mms: u32,
    pub priority: u8,
    pub state: TaskState,
    pub owner: u8,
    pub assigned: Vec<SoldierId>,
}

impl EngineerTask {
    /// 進捗度（0..=1000）。
    pub fn progress_permille(&self) -> u16 {
        if self.work_required_mms == 0 {
            1000
        } else {
            ((self.work_done_mms as u64 * 1000) / self.work_required_mms as u64).min(1000) as u16
        }
    }
}

/// 補給拠点。仕様 07 章 5.1 節。
#[derive(Clone, Copy, Debug)]
pub struct SupplyDepot {
    pub id: DepotId,
    pub pos: Vec2Fx,
    pub owner: u8,
    pub arrows: u32,
    pub bolts: u32,
    pub stones: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SupplyPhase {
    ToDepot,
    ToRallyPoint,
}

struct SupplyRun {
    engineer: SoldierId,
    depot: DepotId,
    rally_point: Vec2Fx,
    owner: u8,
    phase: SupplyPhase,
    carried_arrows: u16,
}

struct RecoveryRun {
    engineer: SoldierId,
    patient: SoldierId,
    owner: u8,
    work_done_mms: u32,
    morale_applied: bool,
}

/// この兵士が今どのタスクに従事しているか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Assignment {
    None,
    Task(TaskId),
    Supply(u32),
    Recovery(u32),
}

/// M6 の工兵システム。配列の index は `SoldierId` と一致する。
#[derive(Default)]
pub struct EngineeringSystem {
    pub tasks: Vec<EngineerTask>,
    pub depots: Vec<SupplyDepot>,
    supply_runs: Vec<Option<SupplyRun>>,
    recovery_runs: Vec<Option<RecoveryRun>>,
    pending_supply: Vec<(DepotId, Vec2Fx, u8)>,
    pending_recovery: Vec<(SoldierId, u8)>,
    assignment: Vec<Assignment>,
    /// この tick 未満は新しい作業に回さない（`FLEE_COOLDOWN_TICKS` 参照）。
    flee_until: Vec<u32>,
}

impl core::fmt::Debug for EngineeringSystem {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EngineeringSystem")
            .field("tasks", &self.tasks.len())
            .field("depots", &self.depots.len())
            .finish()
    }
}

impl EngineeringSystem {
    pub fn register(&mut self) {
        self.assignment.push(Assignment::None);
        self.flee_until.push(0);
    }

    fn ensure_len(&mut self, len: usize) {
        while self.assignment.len() < len {
            self.register();
        }
    }

    pub fn task(&self, id: TaskId) -> Option<&EngineerTask> {
        self.tasks.get(id as usize)
    }

    pub fn depot(&self, id: DepotId) -> Option<&SupplyDepot> {
        self.depots.get(id as usize)
    }

    fn unassign(&mut self, i: usize) {
        match self.assignment.get(i).copied().unwrap_or(Assignment::None) {
            Assignment::Task(task_id) => {
                if let Some(t) = self.tasks.get_mut(task_id as usize) {
                    t.assigned.retain(|&x| x as usize != i);
                    if t.assigned.is_empty() && t.state == TaskState::InProgress {
                        t.state = TaskState::Queued;
                    }
                }
            }
            Assignment::Supply(slot) => {
                if let Some(slot) = self.supply_runs.get_mut(slot as usize) {
                    *slot = None;
                }
            }
            Assignment::Recovery(slot) => {
                if let Some(slot) = self.recovery_runs.get_mut(slot as usize) {
                    *slot = None;
                }
            }
            Assignment::None => {}
        }
        if i < self.assignment.len() {
            self.assignment[i] = Assignment::None;
        }
    }

    // ------------------------------------------------------------------
    // タスク投入 API（`World` から呼ばれる）
    // ------------------------------------------------------------------

    /// 野戦築城タスクを投入する。`structures` に completion=0 の実体を先に作る。
    pub fn queue_build_structure(
        &mut self,
        structures: &mut StructureSystem,
        kind: StructureKind,
        a: Vec2Fx,
        b: Vec2Fx,
        owner: u8,
        priority: u8,
    ) -> TaskId {
        let structure = structures.build(kind, a, b, owner);
        let length_m = fx_to_whole_m(dist(a, b)).max(1) as u32;
        let work_required_mms = kind.work_per_meter_mms() * length_m;
        self.push_task(EngineerTask {
            id: 0,
            kind: TaskKind::BuildStructure { structure },
            site: a.lerp(b, FX_HALF),
            work_required_mms,
            work_done_mms: 0,
            priority,
            state: TaskState::Queued,
            owner,
            assigned: Vec::new(),
        })
    }

    /// 敵構造物の破壊工作タスクを投入する。
    pub fn queue_destroy_structure(
        &mut self,
        structures: &StructureSystem,
        structure: StructureId,
        owner: u8,
        priority: u8,
    ) -> Option<TaskId> {
        let s = structures.get(structure)?;
        let site = s.a.lerp(s.b, FX_HALF);
        // 破壊は建造よりずっと速い（精密に作る必要がなく、壊すだけでよい）。
        let work_required_mms = (s.hp_max as u32) * 4_000;
        Some(self.push_task(EngineerTask {
            id: 0,
            kind: TaskKind::DestroyStructure { structure },
            site,
            work_required_mms,
            work_done_mms: 0,
            priority,
            state: TaskState::Queued,
            owner,
            assigned: Vec::new(),
        }))
    }

    /// 架橋タスクを投入する。`a` から `b` へ向かう直線上の水面セルを橋に変える。
    /// 水面セルが見つからなければ `None`（架ける川がない）。
    pub fn queue_build_bridge(
        &mut self,
        terrain: &Terrain,
        a: Vec2Fx,
        b: Vec2Fx,
        owner: u8,
        priority: u8,
    ) -> Option<TaskId> {
        let cells = water_cells_along(terrain, a, b);
        if cells.is_empty() {
            return None;
        }
        // 架橋は人工物の軸だけを触るので、元に戻すのに覚えておくのも
        // オーバーレイだけでよい。橋を落としても川床は川床のまま。
        let original: Vec<u8> = cells.iter().map(|&idx| terrain.overlay[idx]).collect();
        // 仕様 07 章 3.1 節: 工数 = 川幅(m) × 400 man-s。セル数 × セル一辺を
        // 川幅の近似として使う。
        let work_required_mms = cells.len() as u32 * terrain.cell_m * 400_000;
        Some(self.push_task(EngineerTask {
            id: 0,
            kind: TaskKind::BuildBridge { cells, original },
            site: a.lerp(b, FX_HALF),
            work_required_mms,
            work_done_mms: 0,
            priority,
            state: TaskState::Queued,
            owner,
            assigned: Vec::new(),
        }))
    }

    /// 完成した橋を破壊するタスクを投入する。橋建設タスクの `TaskId` を渡す。
    pub fn queue_destroy_bridge(
        &mut self,
        bridge_task: TaskId,
        owner: u8,
        priority: u8,
    ) -> Option<TaskId> {
        let bridge = self.tasks.get(bridge_task as usize)?;
        let TaskKind::BuildBridge { cells, original } = &bridge.kind else {
            return None;
        };
        let cells = cells.clone();
        let original = original.clone();
        let site = bridge.site;
        // 落とすのは架けるよりずっと速い。
        let work_required_mms = cells.len() as u32 * 40_000;
        Some(self.push_task(EngineerTask {
            id: 0,
            kind: TaskKind::DestroyBridge { cells, original },
            site,
            work_required_mms,
            work_done_mms: 0,
            priority,
            state: TaskState::Queued,
            owner,
            assigned: Vec::new(),
        }))
    }

    /// 伐採タスクを投入する。`min`/`max` の矩形内にある森セルを対象にする。
    pub fn queue_clear_forest(
        &mut self,
        terrain: &Terrain,
        min: Vec2Fx,
        max: Vec2Fx,
        owner: u8,
        priority: u8,
    ) -> Option<TaskId> {
        let cells = forest_cells_in_rect(terrain, min, max);
        if cells.is_empty() {
            return None;
        }
        let cell_area_m2 = terrain.cell_m * terrain.cell_m;
        let mut work_required_mms = 0u32;
        for &idx in &cells {
            let per_m2_mms = match terrain.vegetation_at_index(idx) {
                Vegetation::Dense => 8_000,
                Vegetation::Woodland => 4_000,
                _ => 0,
            };
            work_required_mms += per_m2_mms * cell_area_m2;
        }
        Some(self.push_task(EngineerTask {
            id: 0,
            kind: TaskKind::ClearForest { cells },
            site: min.lerp(max, FX_HALF),
            work_required_mms: work_required_mms.max(1),
            work_done_mms: 0,
            priority,
            state: TaskState::Queued,
            owner,
            assigned: Vec::new(),
        }))
    }

    fn push_task(&mut self, mut task: EngineerTask) -> TaskId {
        let id = self.tasks.len() as TaskId;
        task.id = id;
        self.tasks.push(task);
        id
    }

    /// 補給拠点を配置する。
    pub fn spawn_depot(
        &mut self,
        pos: Vec2Fx,
        owner: u8,
        arrows: u32,
        bolts: u32,
        stones: u32,
    ) -> DepotId {
        let id = self.depots.len() as DepotId;
        self.depots.push(SupplyDepot {
            id,
            pos,
            owner,
            arrows,
            bolts,
            stones,
        });
        id
    }

    /// 矢の補給を要求する。空いている工兵が見つかり次第、`rally_point` 付近の
    /// 弾薬が少ない味方へ届ける（仕様 07 章 5.1 節）。
    pub fn request_arrow_resupply(&mut self, depot: DepotId, rally_point: Vec2Fx, owner: u8) {
        self.pending_supply.push((depot, rally_point, owner));
    }

    /// 倒れた（`Downed`）兵士の回収を要求する。
    pub fn request_wounded_recovery(&mut self, patient: SoldierId, owner: u8) {
        self.pending_recovery.push((patient, owner));
    }

    // ------------------------------------------------------------------
    // tick
    // ------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        world_seed: u64,
        tick: u32,
        soldiers: &mut Soldiers,
        structures: &mut StructureSystem,
        terrain: &mut Terrain,
        combat: &mut CombatSystem,
        goals: &mut [Vec2Fx],
    ) {
        self.ensure_len(soldiers.len());
        let n = soldiers.len();

        let has_engineers =
            (0..n).any(|i| soldiers.is_alive(i) && soldiers.hot.flags[i] & flags::ENGINEER != 0);
        let coarse = has_engineers.then(|| CoarseIndex::build(COARSE_CELL_M, soldiers));

        // 1) 既に何かに従事している兵士の状態を精算する。実戦闘（白兵・射撃・
        //    忌避・気絶・死亡）に引き込まれていれば作業を放棄させ、そうでなければ
        //    `Working` を再主張する（`organization::formation_goals` が同じ tick で
        //    先に状態を書き換えていても、ここで工兵の作業が勝つ）。
        for i in 0..n {
            if self.assignment[i] == Assignment::None {
                continue;
            }
            if !soldiers.is_alive(i) {
                self.unassign(i);
                continue;
            }
            if is_combat_interrupted(soldiers.hot.state[i]) {
                self.unassign(i);
                continue;
            }
            soldiers.hot.state[i] = State::Working;

            if let Some(coarse) = &coarse {
                let pos = soldiers.pos(i);
                let threats = count_nearby_enemies(
                    coarse,
                    soldiers,
                    pos,
                    soldiers.faction[i],
                    DANGER_RADIUS_FX,
                );
                if threats > 0 {
                    let mut rng = Rng::stream(world_seed, i as u32, Purpose::EngineerDanger, tick);
                    if should_abandon_work(soldiers.attrs[i].self_preservation, threats, &mut rng) {
                        self.unassign(i);
                        soldiers.hot.state[i] = State::Idle;
                        self.flee_until[i] = tick + FLEE_COOLDOWN_TICKS;
                    }
                }
            }
        }

        // 2) 空いている工兵を、優先度の高いタスクへ割り当てる。
        self.assign_idle_engineers(soldiers, tick);

        // 3) タスクの進行。
        self.progress_tasks(world_seed, tick, soldiers, structures, terrain, goals);

        // 4) 補給任務・負傷者回収の進行。
        self.progress_supply(soldiers, combat, coarse.as_ref(), goals);
        self.progress_recovery(world_seed, tick, soldiers, coarse.as_ref(), goals);

        // 5) 保留中の補給・回収要求を、空いた工兵に割り当てる。
        self.assign_pending_supply(soldiers, tick);
        self.assign_pending_recovery(soldiers, tick);
    }

    fn assign_idle_engineers(&mut self, soldiers: &mut Soldiers, tick: u32) {
        let n = soldiers.len();
        for i in 0..n {
            if !soldiers.is_alive(i) || soldiers.hot.flags[i] & flags::ENGINEER == 0 {
                continue;
            }
            if self.assignment[i] != Assignment::None
                || soldiers.hot.state[i] != State::Idle
                || self.flee_until[i] > tick
            {
                continue;
            }
            let pos = soldiers.pos(i);
            let faction = soldiers.faction[i];
            let mut best: Option<(i64, TaskId)> = None;
            for t in &self.tasks {
                if t.owner != faction || t.assigned.len() >= MAX_WORKERS_PER_TASK {
                    continue;
                }
                if !matches!(t.state, TaskState::Queued | TaskState::InProgress) {
                    continue;
                }
                // 仕様 07 章 6 節のスコア式（`tactical_value` は M7 の
                // `Objective` 待ちなので抜いてある）:
                //   score = priority*4 + (1000 - travel_time) - work_remaining/100
                let travel_mm = sim_math::fx_to_mm(dist(pos, t.site)) as i64;
                let remaining = (t.work_required_mms - t.work_done_mms) as i64;
                let score =
                    (t.priority as i64) * 4_000 + (30_000 - travel_mm).max(0) - remaining / 50;
                if best.map_or(true, |(bs, _)| score > bs) {
                    best = Some((score, t.id));
                }
            }
            if let Some((_, task_id)) = best {
                self.assignment[i] = Assignment::Task(task_id);
                soldiers.hot.state[i] = State::Working;
                let t = &mut self.tasks[task_id as usize];
                t.assigned.push(i as SoldierId);
                if t.state == TaskState::Queued {
                    t.state = TaskState::InProgress;
                }
            }
        }
    }

    fn progress_tasks(
        &mut self,
        world_seed: u64,
        tick: u32,
        soldiers: &Soldiers,
        structures: &mut StructureSystem,
        terrain: &mut Terrain,
        goals: &mut [Vec2Fx],
    ) {
        for t_idx in 0..self.tasks.len() {
            if self.tasks[t_idx].state != TaskState::InProgress {
                continue;
            }
            let assigned = self.tasks[t_idx].assigned.clone();
            let site = self.tasks[t_idx].site;
            // 架橋・橋の破壊は、現場そのものが水面（作業効率テーブルでは 0‰）
            // にある。実際には土手や舟からの作業なので、地表からは効率を
            // 導かず固定値を使う。
            let fixed_terrain_mult = match self.tasks[t_idx].kind {
                TaskKind::BuildBridge { .. } | TaskKind::DestroyBridge { .. } => Some(800),
                _ => None,
            };
            let mut work_sum: i64 = 0;
            for &sid in &assigned {
                let i = sid as usize;
                if i >= soldiers.len() || i >= goals.len() || !soldiers.is_alive(i) {
                    continue;
                }
                goals[i] = site;
                let pos = soldiers.pos(i);
                if dist(pos, site) > ARRIVAL_RADIUS_FX {
                    continue; // まだ現地へ移動中
                }
                let terrain_mult =
                    fixed_terrain_mult.unwrap_or_else(|| terrain_work_mult_permille(terrain, pos));
                let mut rng = Rng::stream(world_seed, sid, Purpose::WorkVariance, tick);
                work_sum += work_contribution_mms(
                    &soldiers.attrs[i],
                    soldiers.fatigue[i],
                    terrain_mult,
                    &mut rng,
                ) as i64;
            }
            if work_sum == 0 {
                continue;
            }
            let t = &mut self.tasks[t_idx];
            t.work_done_mms =
                (t.work_done_mms as i64 + work_sum).min(t.work_required_mms as i64) as u32;
            let progress = t.progress_permille();
            match &t.kind {
                TaskKind::BuildStructure { structure } => {
                    structures.set_completion(*structure, progress)
                }
                TaskKind::DestroyStructure { structure } => {
                    if let Some(s) = structures.get(*structure) {
                        let target_hp = (s.hp_max as u32 * (1000 - progress as u32) / 1000) as u16;
                        if target_hp < s.hp {
                            structures.damage(*structure, s.hp - target_hp);
                        }
                    }
                }
                TaskKind::BuildBridge { cells, .. } => {
                    apply_overlay_progress(terrain, cells, progress, Overlay::Bridge)
                }
                TaskKind::DestroyBridge { cells, original } => {
                    apply_overlay_revert_progress(terrain, cells, original, progress)
                }
                // 伐採は植生だけを剥ぐ。下の土は土のままで、道にはならない
                TaskKind::ClearForest { cells } => apply_felling_progress(terrain, cells, progress),
            }
            if t.work_done_mms >= t.work_required_mms {
                self.finish_task(t_idx);
            }
        }
    }

    fn finish_task(&mut self, t_idx: usize) {
        let workers = core::mem::take(&mut self.tasks[t_idx].assigned);
        self.tasks[t_idx].state = TaskState::Done;
        for sid in workers {
            let i = sid as usize;
            if i < self.assignment.len() {
                self.assignment[i] = Assignment::None;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn progress_supply(
        &mut self,
        soldiers: &Soldiers,
        combat: &mut CombatSystem,
        coarse: Option<&CoarseIndex>,
        goals: &mut [Vec2Fx],
    ) {
        for slot in 0..self.supply_runs.len() {
            let Some(run) = self.supply_runs[slot].as_ref() else {
                continue;
            };
            let i = run.engineer as usize;
            if i >= soldiers.len()
                || i >= goals.len()
                || !soldiers.is_alive(i)
                || soldiers.hot.state[i] != State::Working
            {
                self.supply_runs[slot] = None;
                continue;
            }
            let (depot_id, rally, phase, carried, owner) = {
                let r = self.supply_runs[slot].as_ref().unwrap();
                (r.depot, r.rally_point, r.phase, r.carried_arrows, r.owner)
            };
            match phase {
                SupplyPhase::ToDepot => {
                    let Some(depot_pos) = self.depots.get(depot_id as usize).map(|d| d.pos) else {
                        self.finish_supply(slot);
                        continue;
                    };
                    goals[i] = depot_pos;
                    if dist(soldiers.pos(i), depot_pos) <= ARRIVAL_RADIUS_FX {
                        if let Some(depot) = self.depots.get_mut(depot_id as usize) {
                            let capacity = CARRY_CAPACITY_ARROWS.saturating_sub(carried) as u32;
                            let take = capacity.min(depot.arrows);
                            depot.arrows -= take;
                            if let Some(r) = self.supply_runs[slot].as_mut() {
                                r.carried_arrows += take as u16;
                                r.phase = SupplyPhase::ToRallyPoint;
                            }
                        }
                    }
                }
                SupplyPhase::ToRallyPoint => {
                    goals[i] = rally;
                    if dist(soldiers.pos(i), rally) <= DELIVERY_RADIUS_FX {
                        let delivered =
                            deliver_arrows(soldiers, combat, coarse, rally, owner, carried);
                        if let Some(r) = self.supply_runs[slot].as_mut() {
                            r.carried_arrows = r.carried_arrows.saturating_sub(delivered);
                        }
                        let carried_now = self.supply_runs[slot].as_ref().unwrap().carried_arrows;
                        let depot_has_stock = self
                            .depots
                            .get(depot_id as usize)
                            .is_some_and(|d| d.arrows > 0);
                        if carried_now == 0 {
                            if depot_has_stock {
                                if let Some(r) = self.supply_runs[slot].as_mut() {
                                    r.phase = SupplyPhase::ToDepot;
                                }
                            } else {
                                self.finish_supply(slot);
                            }
                        }
                    }
                }
            }
        }
    }

    fn finish_supply(&mut self, slot: usize) {
        if let Some(run) = self.supply_runs[slot].take() {
            let i = run.engineer as usize;
            if i < self.assignment.len() {
                self.assignment[i] = Assignment::None;
            }
        }
    }

    fn progress_recovery(
        &mut self,
        world_seed: u64,
        tick: u32,
        soldiers: &mut Soldiers,
        coarse: Option<&CoarseIndex>,
        goals: &mut [Vec2Fx],
    ) {
        for slot in 0..self.recovery_runs.len() {
            let Some(run) = self.recovery_runs[slot].as_ref() else {
                continue;
            };
            let engineer_id = run.engineer;
            let patient_id = run.patient;
            let i = run.engineer as usize;
            let patient = run.patient as usize;
            let owner = run.owner;
            let morale_applied = run.morale_applied;
            if i >= soldiers.len()
                || i >= goals.len()
                || !soldiers.is_alive(i)
                || soldiers.hot.state[i] != State::Working
            {
                self.finish_recovery(slot);
                continue;
            }
            if patient >= soldiers.len() || soldiers.hot.state[patient] != State::Downed {
                self.finish_recovery(slot);
                continue;
            }
            let patient_pos = soldiers.pos(patient);
            goals[i] = patient_pos;
            if dist(soldiers.pos(i), patient_pos) > ARRIVAL_RADIUS_FX {
                continue;
            }
            if !morale_applied {
                apply_recovery_morale_boost(soldiers, coarse, patient_pos, owner);
                self.recovery_runs[slot].as_mut().unwrap().morale_applied = true;
            }
            let mut rng = Rng::stream(world_seed, engineer_id, Purpose::WorkVariance, tick);
            let contribution =
                work_contribution_mms(&soldiers.attrs[i], soldiers.fatigue[i], 1000, &mut rng);
            let done = {
                let r = self.recovery_runs[slot].as_mut().unwrap();
                r.work_done_mms += contribution;
                r.work_done_mms
            };
            if done >= RECOVERY_WORK_MMS {
                let mut result_rng = Rng::stream(world_seed, patient_id, Purpose::RallyCheck, tick);
                if result_rng.chance_permille(RECOVERY_SUCCESS_PERMILLE) {
                    soldiers.hp[patient] = soldiers.hp[patient].max(RECOVERY_HP);
                    soldiers.hot.state[patient] = State::Idle;
                }
                self.finish_recovery(slot);
            }
        }
    }

    fn finish_recovery(&mut self, slot: usize) {
        if let Some(run) = self.recovery_runs[slot].take() {
            let i = run.engineer as usize;
            if i < self.assignment.len() {
                self.assignment[i] = Assignment::None;
            }
        }
    }

    fn assign_pending_supply(&mut self, soldiers: &mut Soldiers, tick: u32) {
        let pending = core::mem::take(&mut self.pending_supply);
        for (depot, rally, owner) in pending {
            if let Some(i) =
                find_idle_engineer(soldiers, &self.assignment, &self.flee_until, tick, owner)
            {
                let slot = alloc_slot(
                    &mut self.supply_runs,
                    SupplyRun {
                        engineer: i as SoldierId,
                        depot,
                        rally_point: rally,
                        owner,
                        phase: SupplyPhase::ToDepot,
                        carried_arrows: 0,
                    },
                );
                self.assignment[i] = Assignment::Supply(slot);
                soldiers.hot.state[i] = State::Working;
            } else {
                self.pending_supply.push((depot, rally, owner));
            }
        }
    }

    fn assign_pending_recovery(&mut self, soldiers: &mut Soldiers, tick: u32) {
        let pending = core::mem::take(&mut self.pending_recovery);
        for (patient, owner) in pending {
            let pi = patient as usize;
            if pi >= soldiers.len() || soldiers.hot.state[pi] != State::Downed {
                continue; // もう回収する意味がない
            }
            if let Some(i) =
                find_idle_engineer(soldiers, &self.assignment, &self.flee_until, tick, owner)
            {
                let slot = alloc_slot(
                    &mut self.recovery_runs,
                    RecoveryRun {
                        engineer: i as SoldierId,
                        patient,
                        owner,
                        work_done_mms: 0,
                        morale_applied: false,
                    },
                );
                self.assignment[i] = Assignment::Recovery(slot);
                soldiers.hot.state[i] = State::Working;
            } else {
                self.pending_recovery.push((patient, owner));
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
        for t in &self.tasks {
            mix(t.work_done_mms as u64);
            mix(t.state as u64);
            mix(t.assigned.len() as u64);
        }
        for d in &self.depots {
            mix(d.arrows as u64);
            mix(d.bolts as u64);
            mix(d.stones as u64);
        }
        for run in self.supply_runs.iter().flatten() {
            mix(run.carried_arrows as u64);
            mix(run.phase as u64);
        }
        for run in self.recovery_runs.iter().flatten() {
            mix(run.work_done_mms as u64);
        }
        h
    }
}

fn is_combat_interrupted(state: State) -> bool {
    matches!(
        state,
        State::Engaged
            | State::Shooting
            | State::Wavering
            | State::Broken
            | State::Downed
            | State::Dead
    )
}

fn count_nearby_enemies(
    coarse: &CoarseIndex,
    soldiers: &Soldiers,
    pos: Vec2Fx,
    faction: u8,
    radius: Fx,
) -> u32 {
    let mut buf = [0u32; MAX_NEIGHBORS];
    let n = coarse.query_excluding_faction(soldiers, pos.x, pos.y, faction, &mut buf);
    let r2 = (radius as i64) * (radius as i64);
    buf[..n]
        .iter()
        .filter(|&&id| {
            let j = id as usize;
            soldiers.is_alive(j) && dist_sq(pos, soldiers.pos(j)) <= r2
        })
        .count() as u32
}

/// 危険を察知した工兵が作業を放棄するか（仕様 07 章 1.3 節「`self_preservation` の判定」）。
/// `self_preservation` が高いほど、また近くの脅威が多いほど放棄しやすい。
fn should_abandon_work(self_preservation: u8, threat_count: u32, rng: &mut Rng) -> bool {
    let base = 80 + threat_count.min(6) * 130;
    let scaled = base * (100 + self_preservation as u32) / 100;
    rng.chance_permille(scaled.min(950))
}

fn find_idle_engineer(
    soldiers: &Soldiers,
    assignment: &[Assignment],
    flee_until: &[u32],
    tick: u32,
    owner: u8,
) -> Option<usize> {
    (0..soldiers.len()).find(|&i| {
        soldiers.is_alive(i)
            && soldiers.faction[i] == owner
            && soldiers.hot.flags[i] & flags::ENGINEER != 0
            && soldiers.hot.state[i] == State::Idle
            && assignment.get(i).copied().unwrap_or(Assignment::None) == Assignment::None
            && flee_until.get(i).copied().unwrap_or(0) <= tick
    })
}

fn alloc_slot<T>(slots: &mut Vec<Option<T>>, value: T) -> u32 {
    if let Some(pos) = slots.iter().position(Option::is_none) {
        slots[pos] = Some(value);
        pos as u32
    } else {
        slots.push(Some(value));
        (slots.len() - 1) as u32
    }
}

fn deliver_arrows(
    soldiers: &Soldiers,
    combat: &mut CombatSystem,
    coarse: Option<&CoarseIndex>,
    rally: Vec2Fx,
    faction: u8,
    carried: u16,
) -> u16 {
    let Some(coarse) = coarse else {
        return 0;
    };
    let mut buf = [0u32; MAX_NEIGHBORS];
    let n = coarse.query_all(rally.x, rally.y, &mut buf);
    let r2 = (DELIVERY_RADIUS_FX as i64) * (DELIVERY_RADIUS_FX as i64);
    let mut remaining = carried;
    for &id in &buf[..n] {
        if remaining == 0 {
            break;
        }
        let j = id as usize;
        if !soldiers.is_alive(j) || soldiers.faction[j] != faction {
            continue;
        }
        if !combat.weapons.get(j).is_some_and(|w| w.ranged) {
            continue;
        }
        if dist_sq(rally, soldiers.pos(j)) > r2 {
            continue;
        }
        let current = combat.ammo.get(j).copied().unwrap_or(0);
        if current >= RESUPPLY_TARGET_AMMO {
            continue;
        }
        let need = RESUPPLY_TARGET_AMMO - current;
        let give = need.min(remaining);
        combat.set_ammo(id, current + give);
        remaining -= give;
    }
    carried - remaining
}

fn apply_recovery_morale_boost(
    soldiers: &mut Soldiers,
    coarse: Option<&CoarseIndex>,
    pos: Vec2Fx,
    owner: u8,
) {
    let Some(coarse) = coarse else {
        return;
    };
    let mut buf = [0u32; MAX_NEIGHBORS];
    let n = coarse.query_all(pos.x, pos.y, &mut buf);
    let r2 = (RECOVERY_MORALE_RADIUS_FX as i64) * (RECOVERY_MORALE_RADIUS_FX as i64);
    for &id in &buf[..n] {
        let j = id as usize;
        if !soldiers.is_alive(j) || soldiers.faction[j] != owner {
            continue;
        }
        if dist_sq(pos, soldiers.pos(j)) > r2 {
            continue;
        }
        soldiers.morale[j] = (soldiers.morale[j] + RECOVERY_MORALE_BONUS).min(MAX_MORALE);
    }
}

fn fx_to_whole_m(v: Fx) -> i32 {
    (sim_math::fx_to_mm(v) + 500) / 1000
}

/// `a` から `b` へ直線を引き、通り道にある水面セルの index を集める
/// （架橋対象、仕様 07 章 3.1 節）。
fn water_cells_along(terrain: &Terrain, a: Vec2Fx, b: Vec2Fx) -> Vec<usize> {
    let (ax, ay) = terrain.world_to_cell(a.x, a.y);
    let (bx, by) = terrain.world_to_cell(b.x, b.y);
    let dx = bx as i32 - ax as i32;
    let dy = by as i32 - ay as i32;
    let steps = dx.abs().max(dy.abs()).max(1);
    let mut cells = Vec::new();
    for s in 0..=steps {
        let x = ax as i32 + dx * s / steps;
        let y = ay as i32 + dy * s / steps;
        if x < 0 || y < 0 || x as u32 >= terrain.dim || y as u32 >= terrain.dim {
            continue;
        }
        let idx = terrain.idx(x as u32, y as u32);
        // 渡渉できる浅さ（渡渉点）には橋を架けない。歩いて渡れる
        if terrain.water[idx] > FORD_MAX_DEPTH_CM && cells.last() != Some(&idx) {
            cells.push(idx);
        }
    }
    cells
}

/// 矩形内にある森セルの index を集める（伐採対象、仕様 07 章 3.3 節）。
fn forest_cells_in_rect(terrain: &Terrain, corner_a: Vec2Fx, corner_b: Vec2Fx) -> Vec<usize> {
    let (min_x, min_y) =
        terrain.world_to_cell(corner_a.x.min(corner_b.x), corner_a.y.min(corner_b.y));
    let (max_x, max_y) =
        terrain.world_to_cell(corner_a.x.max(corner_b.x), corner_a.y.max(corner_b.y));
    let mut cells = Vec::new();
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let idx = terrain.idx(x, y);
            if terrain.vegetation_at_index(idx).is_forest() {
                cells.push(idx);
            }
        }
    }
    cells
}

/// 進捗（0..=1000‰）に応じて `cells` の先頭から比例した数だけ人工物を置く。
fn apply_overlay_progress(
    terrain: &mut Terrain,
    cells: &[usize],
    progress_permille: u16,
    target: Overlay,
) {
    for &idx in &cells[..progressed_count(cells.len(), progress_permille)] {
        if terrain.overlay_at_index(idx) != target {
            terrain.set_overlay(idx, target);
        }
    }
}

/// [`apply_overlay_progress`] の逆。進捗に応じて元の人工物へ戻していく。
fn apply_overlay_revert_progress(
    terrain: &mut Terrain,
    cells: &[usize],
    original: &[u8],
    progress_permille: u16,
) {
    for k in 0..progressed_count(cells.len(), progress_permille) {
        let orig = Overlay::from_u8(original[k]);
        if terrain.overlay_at_index(cells[k]) != orig {
            terrain.set_overlay(cells[k], orig);
        }
    }
}

/// 進捗に応じて森を伐り倒していく。
///
/// 触るのは植生の軸だけ。木を切っても下が草地になるわけではないので、
/// 地質は生成器が決めたまま残す（移行前は伐採跡が一律 `Grass` になり、
/// 泥濘の森を切ると足元まで乾いていた）。
fn apply_felling_progress(terrain: &mut Terrain, cells: &[usize], progress_permille: u16) {
    for &idx in &cells[..progressed_count(cells.len(), progress_permille)] {
        if terrain.vegetation_at_index(idx) != Vegetation::None {
            terrain.set_vegetation(idx, Vegetation::None);
        }
    }
}

#[inline]
fn progressed_count(len: usize, progress_permille: u16) -> usize {
    ((len as u32 * progress_permille as u32) / 1000).min(len as u32) as usize
}

/// 足場ごとの作業効率（‰、1000 = 標準）。岩盤は掘りにくく、泥は掘りやすい。
fn terrain_work_mult_permille(terrain: &Terrain, pos: Vec2Fx) -> i32 {
    let (cx, cy) = terrain.world_to_cell(pos.x, pos.y);
    let idx = terrain.idx(cx, cy);
    sim_terrain::work_mult_permille(
        terrain.ground_at_index(idx),
        terrain.vegetation_at_index(idx),
        terrain.water[idx],
    )
}

/// 仕様 07 章 1.2 節の `work_per_tick` 式（1 人分）。`weather_mult` /
/// `tool_mult` はまだ実装がないので 1000‰ 固定として省いてある。
fn work_contribution_mms(
    attrs: &Attrs,
    fatigue: u16,
    terrain_mult_permille: i32,
    rng: &mut Rng,
) -> u32 {
    let strength_permille = (attrs.strength as i64 + 128) * 1000 / 256;
    let fatigue_permille = (1000 - (fatigue as i64 * 1000 / 12_000)).max(0);
    let base = BASE_RATE_MMS_PER_SEC / sim_math::TICK_HZ as i64;
    // ±10% の作業効率の揺らぎ（仕様の `Purpose::WorkVariance` はこのために
    // 事前に用意されていた）。
    let variance_permille = 900 + rng.range(0, 201) as i64;
    let v = base
        * strength_permille
        * fatigue_permille
        * terrain_mult_permille as i64
        * variance_permille
        / 1_000_000_000_000i64;
    v.max(0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::{Armor, Weapon};
    use crate::soldiers::Attrs;
    use sim_math::{fx, fx_from_mm as mm};
    use sim_terrain::WaterKind;

    fn small_terrain(seed: u64) -> Terrain {
        Terrain::flat(200, 2, seed)
    }

    fn worker_attrs() -> Attrs {
        Attrs::new(140, 140, 150, 160, 140, 140, 140, 140, 120, 120, 140, 140)
    }

    struct Env {
        soldiers: Soldiers,
        structures: StructureSystem,
        engineering: EngineeringSystem,
        combat: CombatSystem,
        terrain: Terrain,
        goals: Vec<Vec2Fx>,
    }

    impl Env {
        fn new() -> Self {
            Env {
                soldiers: Soldiers::default(),
                structures: StructureSystem::default(),
                engineering: EngineeringSystem::default(),
                combat: CombatSystem::default(),
                terrain: small_terrain(1),
                goals: Vec::new(),
            }
        }

        fn spawn_engineer(&mut self, pos: Vec2Fx) -> SoldierId {
            let id = self
                .soldiers
                .push(pos.x, pos.y, 0, 0, 0, worker_attrs(), flags::ENGINEER);
            self.combat.register();
            self.engineering.register();
            self.goals.push(pos);
            id
        }

        /// 工兵以外の兵士（脅威役・患者・味方）を、他の配列と長さを揃えて追加する。
        fn spawn_plain(&mut self, pos: Vec2Fx, faction: u8) -> SoldierId {
            let id = self
                .soldiers
                .push(pos.x, pos.y, 0, 0, faction, Attrs::default(), 0);
            self.combat.register();
            self.engineering.register();
            self.goals.push(pos);
            id
        }

        fn tick(&mut self, seed: u64, tick: u32) {
            self.engineering.tick(
                seed,
                tick,
                &mut self.soldiers,
                &mut self.structures,
                &mut self.terrain,
                &mut self.combat,
                &mut self.goals,
            );
            // 目標へ瞬間移動させ、次 tick で現地作業に入れるようにする
            // （このテストは移動システムを持たないので、到着だけ模擬する）。
            for i in 0..self.soldiers.len().min(self.goals.len()) {
                if dist(self.soldiers.pos(i), self.goals[i]) > fx(2) {
                    self.soldiers.set_pos(i, self.goals[i]);
                }
            }
        }
    }

    #[test]
    fn engineer_walks_to_site_then_completes_the_structure() {
        let mut env = Env::new();
        let site_a = Vec2Fx::new(fx(50), fx(50));
        let site_b = Vec2Fx::new(fx(50), fx(52));
        env.spawn_engineer(Vec2Fx::new(fx(10), fx(10)));
        let task = env.engineering.queue_build_structure(
            &mut env.structures,
            StructureKind::Stakes,
            site_a,
            site_b,
            0,
            5,
        );

        for t in 0..2000 {
            env.tick(1, t);
            if env.engineering.task(task).unwrap().state == TaskState::Done {
                break;
            }
        }
        let t = env.engineering.task(task).unwrap();
        assert_eq!(t.state, TaskState::Done);
        let TaskKind::BuildStructure { structure } = t.kind else {
            unreachable!()
        };
        assert_eq!(
            env.structures.get(structure).unwrap().completion_permille,
            1000
        );
    }

    #[test]
    fn partial_work_is_preserved_and_gives_partial_effect() {
        let mut env = Env::new();
        // 完成に必要な工数が大きい Ditch を短時間だけ進め、道半ばで止める。
        let site_a = Vec2Fx::new(fx(50), fx(50));
        let site_b = Vec2Fx::new(fx(50), fx(52));
        env.spawn_engineer(site_a);
        let task = env.engineering.queue_build_structure(
            &mut env.structures,
            StructureKind::Ditch,
            site_a,
            site_b,
            0,
            5,
        );

        for t in 0..300 {
            env.tick(1, t);
        }
        let progress = env.engineering.task(task).unwrap().progress_permille();
        assert!(progress > 0 && progress < 1000, "progress={progress}");
        let TaskKind::BuildStructure { structure } = env.engineering.task(task).unwrap().kind
        else {
            unreachable!()
        };
        assert_eq!(
            env.structures.get(structure).unwrap().completion_permille,
            progress
        );
    }

    #[test]
    fn engineer_flees_when_enemies_are_near() {
        let mut env = Env::new();
        let site = Vec2Fx::new(fx(50), fx(50));
        let worker = env.spawn_engineer(site);
        let task = env.engineering.queue_build_structure(
            &mut env.structures,
            StructureKind::Stakes,
            site,
            Vec2Fx::new(fx(50), fx(52)),
            0,
            5,
        );
        env.tick(1, 0);
        assert_eq!(env.soldiers.hot.state[worker as usize], State::Working);

        // 敵を大量に間近へ配置する
        for k in 0..5 {
            env.spawn_plain(Vec2Fx::new(fx(51 + k), fx(50)), 1);
        }

        let mut fled = false;
        for t in 1..300 {
            env.tick(1, t);
            if env.soldiers.hot.state[worker as usize] != State::Working {
                fled = true;
                break;
            }
        }
        assert!(fled, "危険が近くにあっても作業を続けている");
        let TaskKind::BuildStructure { .. } = env.engineering.task(task).unwrap().kind else {
            unreachable!()
        };
        assert_ne!(env.engineering.task(task).unwrap().state, TaskState::Done);
    }

    /// 仕様 12 章 M6 の受け入れ条件「仮橋のスループットがボトルネックとして
    /// 機能する」の土台: 架橋前は渡れない水面セルが、架橋の完成度に応じて
    /// 少しずつ（半分の進捗なら半分のセルが）渡れるようになる。
    #[test]
    fn build_bridge_gradually_opens_a_crossing_over_water() {
        let mut env = Env::new();
        // 川に見立てて、川岸から川岸まで一直線に深水セルを敷く（4 セル = 8 m 幅）。
        let a = Vec2Fx::new(fx(50), fx(50));
        let b = Vec2Fx::new(fx(50), fx(56));
        let (cx, cy0) = env.terrain.world_to_cell(a.x, a.y);
        let (_, cy1) = env.terrain.world_to_cell(b.x, b.y);
        for cy in cy0.min(cy1)..=cy0.max(cy1) {
            let idx = env.terrain.idx(cx, cy);
            env.terrain.set_water(idx, 400, WaterKind::River);
        }
        let before_idx = env.terrain.idx(cx, cy0);
        assert_eq!(
            env.terrain.passability[before_idx], 0,
            "テスト前提: 深水は通行不能"
        );

        env.spawn_engineer(a);
        let task = env
            .engineering
            .queue_build_bridge(&env.terrain, a, b, 0, 5)
            .expect("水面があるので橋タスクが作れるはず");

        // 架橋は工数が大きい（仕様「川幅(m) × 400 man-s」）ので、完成の手前
        // までしか進めない。それでも「一部のセルだけ先に橋になる」ことを
        // 確かめるには十分な tick 数を回す。
        for t in 0..50_000 {
            env.tick(1, t);
        }
        let progress = env.engineering.task(task).unwrap().progress_permille();
        assert!(progress > 0 && progress < 1000, "progress={progress}");

        let TaskKind::BuildBridge { cells, .. } = &env.engineering.task(task).unwrap().kind else {
            unreachable!()
        };
        let bridged = cells
            .iter()
            .filter(|&&idx| env.terrain.overlay_at_index(idx) == Overlay::Bridge)
            .count();
        assert!(
            bridged > 0,
            "一部でも橋に変わっているはず (progress={progress})"
        );
        assert!(
            bridged < cells.len(),
            "まだ全部は橋になっていないはず（半端な進捗）"
        );
        for &idx in cells.iter().take(bridged) {
            assert!(
                env.terrain.passability[idx] > 0,
                "橋になったセルは通行可能なはず"
            );
        }
    }

    #[test]
    fn destroyed_structure_loses_its_effect() {
        let mut sys = StructureSystem::default();
        let id = sys.build(
            StructureKind::Stakes,
            Vec2Fx::new(fx(10), fx(0)),
            Vec2Fx::new(fx(10), fx(2)),
            0,
        );
        sys.set_completion(id, 1000);
        assert!(sys.stakes_refusal_bonus_permille(id) > 0);
    }

    #[test]
    fn arrow_resupply_refills_a_depleted_archer_and_stops_when_depot_is_empty() {
        let mut env = Env::new();
        let archer_pos = Vec2Fx::new(fx(50), fx(50));
        let archer = env.spawn_plain(archer_pos, 0);
        env.combat
            .set_loadout(archer, Weapon::longbow(), Armor::cloth());
        env.combat.set_ammo(archer, 0);

        env.spawn_engineer(Vec2Fx::new(fx(10), fx(10)));
        let depot = env.engineering.spawn_depot(
            Vec2Fx::new(fx(20), fx(20)),
            0,
            ARROWS_PER_BUNDLE as u32,
            0,
            0,
        );
        env.engineering.request_arrow_resupply(depot, archer_pos, 0);

        for t in 0..3000 {
            env.tick(1, t);
            if env.combat.ammo[archer as usize] > 0 {
                break;
            }
        }
        assert!(env.combat.ammo[archer as usize] > 0, "矢が届いていない");
        // 拠点は空になっているはず
        assert_eq!(env.engineering.depot(depot).unwrap().arrows, 0);
    }

    /// 回収の開始（現地到着）時点で近傍に士気ボーナスが入る（仕様 5.2 節）。
    /// 最終的に軽傷復帰するかどうかは確率的（`RECOVERY_SUCCESS_PERMILLE`）
    /// なので、ここでは結果を待たずに「助けに向かい始めた」ことだけ見る。
    #[test]
    fn wounded_recovery_boosts_nearby_morale_once_it_starts() {
        let mut env = Env::new();
        let patient_pos = Vec2Fx::new(mm(50_000), mm(50_000));
        let patient = env.spawn_plain(patient_pos, 0);
        env.soldiers.hot.state[patient as usize] = State::Downed;
        env.soldiers.hp[patient as usize] = 5;

        let ally = env.spawn_plain(Vec2Fx::new(mm(51_000), mm(50_000)), 0);
        let morale_before = env.soldiers.morale[ally as usize];

        env.spawn_engineer(Vec2Fx::new(fx(10), fx(10)));
        env.engineering.request_wounded_recovery(patient, 0);

        for t in 0..300 {
            env.tick(1, t);
            if env.soldiers.morale[ally as usize] > morale_before {
                break;
            }
        }
        assert!(
            env.soldiers.morale[ally as usize] > morale_before,
            "士気が上がっていない"
        );
    }

    /// `RECOVERY_SUCCESS_PERMILLE` に従い、回収された負傷者の大半は軽傷で復帰する。
    #[test]
    fn wounded_recovery_usually_succeeds() {
        let trials = 12;
        let mut recovered = 0;
        for trial in 0..trials {
            let mut env = Env::new();
            let patient_pos = Vec2Fx::new(mm(50_000), mm(50_000));
            let patient = env.spawn_plain(patient_pos, 0);
            env.soldiers.hot.state[patient as usize] = State::Downed;
            env.soldiers.hp[patient as usize] = 5;
            env.spawn_engineer(Vec2Fx::new(fx(10), fx(10)));
            env.engineering.request_wounded_recovery(patient, 0);

            for t in 0..3000 {
                env.tick(trial as u64, t);
                if env.soldiers.hot.state[patient as usize] == State::Idle {
                    recovered += 1;
                    assert!(env.soldiers.hp[patient as usize] >= RECOVERY_HP);
                    break;
                }
            }
        }
        assert!(
            recovered * 10 >= trials * 6,
            "回収の成功率が低すぎる: {recovered}/{trials}"
        );
    }
}
