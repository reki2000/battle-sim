//! 編成・指揮・陣形（M3）。
//!
//! 指揮ツリーは階層の名前を知らない汎用エンジンとして実装する。シナリオ側は
//! `CommandNode` を任意の深さで構成し、葉ノードに `Unit` を接続する。命令は
//! 伝令ごとに距離から遅延を計算するため、発令順や走査順に結果が依存しない。

use sim_math::{
    dist, fx, fx_from_mm, fx_mul, ms_to_ticks, per_sec_to_per_tick, Brad, Fx, Rng, Vec2Fx, FX_ONE,
};
use sim_terrain::Terrain;

use crate::pathing;
use crate::soldiers::{flags, SoldierId, Soldiers, State};
use crate::spatial::{SpatialHash, MAX_NEIGHBORS};

pub type NodeId = u32;
pub type OrderId = u32;
pub type FactionId = u8;
pub type FormationId = u8;

pub const FORMATION_LINE: FormationId = 0;
pub const FORMATION_SHIELDWALL: FormationId = 1;
pub const FORMATION_COLUMN: FormationId = 2;
pub const FORMATION_WEDGE: FormationId = 3;
pub const FORMATION_SCHILTRON: FormationId = 4;
pub const FORMATION_PIKE_SQUARE: FormationId = 5;
pub const FORMATION_SKIRMISH: FormationId = 6;
pub const FORMATION_PAVISE_LINE: FormationId = 7;
pub const FORMATION_ECHELON: FormationId = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormationShape {
    Rect,
    Wedge,
    Circle,
    Loose,
    Line,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormationDef {
    pub id: FormationId,
    pub file_spacing: Fx,
    pub rank_spacing: Fx,
    pub default_ranks: u16,
    pub shape: FormationShape,
    pub def_front: u16,
    pub def_flank: u16,
    pub def_rear: u16,
    pub move_mult: u16,
    pub turn_mult: u16,
    pub cohesion_req: u16,
    pub anti_cavalry: u16,
    pub allow_shoot: bool,
    pub change_time_base_s: u16,
}

/// 組み込みの中世プリセット。後続の `sim-data` では同じ値をデータから供給する。
pub const fn formation_def(id: FormationId) -> FormationDef {
    match id {
        FORMATION_SHIELDWALL => FormationDef {
            id,
            file_spacing: fx_from_mm(500),
            rank_spacing: fx_from_mm(700),
            default_ranks: 5,
            shape: FormationShape::Line,
            def_front: 1500,
            def_flank: 750,
            def_rear: 650,
            move_mult: 550,
            turn_mult: 500,
            cohesion_req: 150,
            anti_cavalry: 1300,
            allow_shoot: false,
            change_time_base_s: 45,
        },
        FORMATION_COLUMN => FormationDef {
            id,
            file_spacing: fx_from_mm(900),
            rank_spacing: fx_from_mm(900),
            default_ranks: 8,
            shape: FormationShape::Rect,
            def_front: 900,
            def_flank: 900,
            def_rear: 800,
            move_mult: 1200,
            turn_mult: 1300,
            cohesion_req: 100,
            anti_cavalry: 800,
            allow_shoot: false,
            change_time_base_s: 20,
        },
        FORMATION_WEDGE => FormationDef {
            id,
            file_spacing: fx_from_mm(1200),
            rank_spacing: fx_from_mm(1000),
            default_ranks: 3,
            shape: FormationShape::Wedge,
            def_front: 1300,
            def_flank: 800,
            def_rear: 700,
            move_mult: 1000,
            turn_mult: 700,
            cohesion_req: 130,
            anti_cavalry: 900,
            allow_shoot: false,
            change_time_base_s: 30,
        },
        FORMATION_SCHILTRON => FormationDef {
            id,
            file_spacing: fx_from_mm(500),
            rank_spacing: fx_from_mm(700),
            default_ranks: 4,
            shape: FormationShape::Circle,
            def_front: 1200,
            def_flank: 1200,
            def_rear: 1200,
            move_mult: 300,
            turn_mult: 200,
            cohesion_req: 150,
            anti_cavalry: 1800,
            allow_shoot: false,
            change_time_base_s: 60,
        },
        FORMATION_PIKE_SQUARE => FormationDef {
            id,
            file_spacing: fx_from_mm(600),
            rank_spacing: fx_from_mm(700),
            default_ranks: 6,
            shape: FormationShape::Circle,
            def_front: 1600,
            def_flank: 500,
            def_rear: 500,
            move_mult: 500,
            turn_mult: 250,
            cohesion_req: 170,
            anti_cavalry: 2000,
            allow_shoot: false,
            change_time_base_s: 60,
        },
        FORMATION_SKIRMISH => FormationDef {
            id,
            file_spacing: fx_from_mm(2500),
            rank_spacing: fx_from_mm(1500),
            default_ranks: 2,
            shape: FormationShape::Loose,
            def_front: 600,
            def_flank: 600,
            def_rear: 600,
            move_mult: 1150,
            turn_mult: 1400,
            cohesion_req: 60,
            anti_cavalry: 500,
            allow_shoot: true,
            change_time_base_s: 15,
        },
        FORMATION_PAVISE_LINE => FormationDef {
            id,
            file_spacing: fx_from_mm(1000),
            rank_spacing: fx_from_mm(900),
            default_ranks: 2,
            shape: FormationShape::Line,
            def_front: 1400,
            def_flank: 600,
            def_rear: 600,
            move_mult: 400,
            turn_mult: 600,
            cohesion_req: 120,
            anti_cavalry: 700,
            allow_shoot: true,
            change_time_base_s: 30,
        },
        FORMATION_ECHELON => FormationDef {
            id,
            file_spacing: fx_from_mm(800),
            rank_spacing: fx_from_mm(800),
            default_ranks: 3,
            shape: FormationShape::Line,
            def_front: 1000,
            def_flank: 850,
            def_rear: 800,
            move_mult: 950,
            turn_mult: 900,
            cohesion_req: 110,
            anti_cavalry: 1000,
            allow_shoot: false,
            change_time_base_s: 30,
        },
        _ => FormationDef {
            id: FORMATION_LINE,
            file_spacing: fx_from_mm(800),
            rank_spacing: fx_from_mm(800),
            default_ranks: 4,
            shape: FormationShape::Line,
            def_front: 1000,
            def_flank: 700,
            def_rear: 650,
            move_mult: 1000,
            turn_mult: 1000,
            cohesion_req: 100,
            anti_cavalry: 1000,
            allow_shoot: false,
            change_time_base_s: 30,
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandState {
    Commanded,
    Leaderless,
    Succeeding,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    Routine,
    Urgent,
    Absolute,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveSpeed {
    Cautious,
    Walk,
    Quick,
    Run,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApproachStyle {
    Deliberate,
    Aggressive,
    Cautious,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShootMode {
    Volley,
    AtWill,
    Hold,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    MoveTo {
        pos: Vec2Fx,
        facing: Brad,
        speed: MoveSpeed,
        formation: FormationId,
    },
    Hold {
        pos: Vec2Fx,
        facing: Brad,
        allow_pursuit: bool,
    },
    Attack {
        target: NodeId,
        approach: ApproachStyle,
    },
    Charge {
        target: NodeId,
    },
    Flank {
        target: NodeId,
        side: Side,
    },
    Envelop {
        target: NodeId,
    },
    Screen {
        protect: NodeId,
        side: Side,
    },
    Reserve {
        rally_pos: Vec2Fx,
    },
    Withdraw {
        to: Vec2Fx,
        fighting: bool,
    },
    Pursue {
        target: NodeId,
        max_distance_m: u16,
    },
    ShootAt {
        target: NodeId,
        mode: ShootMode,
    },
    /// 特定の兵士（通常は指揮官）を追跡し、行動不能になるまで捕捉する。
    HuntPerson {
        target: SoldierId,
    },
    /// 円形の区域へ進出し、兵士を区域内へ分散させて占拠する。
    OccupyArea {
        center: Vec2Fx,
        radius_m: u16,
    },
    /// 円形の区域を守り、`intercept_radius_m` 内へ入った敵を局所迎撃する。
    GuardArea {
        center: Vec2Fx,
        radius_m: u16,
        intercept_radius_m: u16,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Order {
    pub id: OrderId,
    pub issued_tick: u32,
    pub issuer: NodeId,
    pub target: NodeId,
    pub intent: Intent,
    pub priority: Priority,
    pub expires_tick: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryMethod {
    Messenger,
    Flag,
    Acoustic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessengerState {
    Riding,
    Delivering,
    Delivered,
    Lost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Messenger {
    pub soldier: SoldierId,
    pub order: Order,
    pub from: NodeId,
    pub to: NodeId,
    pub state: MessengerState,
    pub method: DeliveryMethod,
    pub remaining_ticks: u32,
    pub total_ticks: u32,
    pub position: Vec2Fx,
    pub destination: Vec2Fx,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InFlightOrder {
    pub order: Order,
    pub messenger: usize,
    pub next_hop: NodeId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compliance {
    Obeyed,
    Partial,
    Ignored,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandEventKind {
    Issued,
    Received,
    Obeyed,
    Partial,
    Ignored,
    MessengerLost,
    LeaderLost,
    SuccessionStarted,
    SuccessionCompleted,
    FormationChangeStarted,
    FormationChanged,
    /// M7: 命令に反して指揮官が独断で行動した（仕様 05 章「ambition の高い
    /// 騎士が命令を無視して突撃する」）。
    Insubordination,
    /// 疲れた前列を後列と入れ替えた（仕様 06 章 6 節）。
    RanksRotated,
    /// B5: 任務の段階が変わった（条件付き命令の引き金にできる）。
    MissionStateChanged,
    /// B1: 任務を達成した（追跡していた人物が倒れた等）。
    ObjectiveComplete,
    /// B1: 任務を打ち切った（見失った人物を探しても見つからなかった等）。
    /// 新しい末尾へ足すこと。値は `commandEvents` として UI へそのまま渡る。
    ObjectiveAbandoned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandEvent {
    pub tick: u32,
    pub node: NodeId,
    pub order: Option<OrderId>,
    pub kind: CommandEventKind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NodeStats {
    pub centroid: Vec2Fx,
    pub facing: Brad,
    pub frontage_m: Fx,
    pub alive: u32,
    pub downed: u32,
    pub dead: u32,
    pub broken: u32,
    pub avg_morale: u16,
    pub avg_fatigue: u16,
    pub cohesion: u16,
    pub engaged_ratio: u16,
}

// M7 の指揮官 AI（`docs/spec/05-ai.md` 5〜7 節）。データ型はここに置き、
// 判断ロジック（視認・評価・目的選択・独断専行）は `crate::commander_ai` に
// 分離する（`cavalry`/`engineering` が `soldiers`/`combat`/`structures` の
// データを外から動かすのと同じ構成）。

/// 指揮官の性格パラメータ。兵士共通の `Attrs` とは別軸で、戦略層の判断だけに効く。
/// 仕様 05 章 5.4 節。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommanderAttrs {
    pub boldness: u8,
    pub caution: u8,
    pub initiative: u8,
    pub obedience: u8,
    pub tactical_skill: u8,
    pub ambition: u8,
    pub charisma: u8,
    pub flexibility: u8,
    pub patience: u8,
    pub ruthlessness: u8,
}

/// 敵（または味方）部隊についての推定情報。仕様 05 章 5.1 節。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KnownForce {
    pub node: NodeId,
    pub est_pos: Vec2Fx,
    pub est_strength: u32,
    pub est_troop_type: u16,
    pub est_morale: u16,
    /// 0..1000。古い情報ほど低い。
    pub confidence: u16,
    pub observed_tick: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainNoteKind {
    HighGround,
    Ford,
    Forest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainNote {
    pub pos: Vec2Fx,
    pub kind: TerrainNoteKind,
}

/// 指揮官が「自分が知っていることだけ」で判断するための情報源。仕様 05 章 5.1 節。
///
/// `own_forces` と `terrain_notes` は現行実装では埋めない。自軍の戦力は
/// `NodeStats`から直接読める前提で扱う。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blackboard {
    pub own_forces: Vec<KnownForce>,
    pub enemy_forces: Vec<KnownForce>,
    pub terrain_notes: Vec<TerrainNote>,
    pub last_updated: u32,
}

/// 戦況評価。仕様05章5.2節。`critical_points`はまだ未実装。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SituationAssessment {
    /// 自軍 ÷ 敵軍を 1000 分率で表す（1000 = 互角）。
    pub force_ratio_permille: i32,
    /// 押しているか押されているか（自軍と敵軍の士気差から近似する。本文参照）。
    pub momentum: i32,
    pub flank_threats: [u16; 2],
    pub rear_threat: u16,
    pub reserve_available: u32,
    pub terrain_advantage: i32,
    pub time_pressure: i32,
}

/// 戦略層が選ぶ目的。仕様 05 章 5.3 節（3 種を実装。表の残りは将来の拡張）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Objective {
    Envelop { target: NodeId },
    HoldHighGround,
    CommitReserve,
}

/// `Objective` を決定論検証用ハッシュに混ぜるための下請け。
fn objective_hash(o: Objective) -> u64 {
    match o {
        Objective::Envelop { target } => 0x1_0000_0000 | target as u64,
        Objective::HoldHighGround => 1,
        Objective::CommitReserve => 2,
    }
}

/// 葉ノードの現在任務を決定論検証へ含める。局所兵士 AI は Hold / Reserve / Attack
/// などで迎撃半径を変えるため、命令 ID だけでなく解釈後の Intent 自体が状態である。
fn intent_hash(intent: Intent) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |v: u64| {
        h ^= v;
        h = h.wrapping_mul(0x0100_0000_01b3);
    };
    match intent {
        Intent::MoveTo {
            pos,
            facing,
            speed,
            formation,
        } => {
            mix(0);
            mix(pos.x as u32 as u64);
            mix(pos.y as u32 as u64);
            mix(facing as u64);
            mix(speed as u64);
            mix(formation as u64);
        }
        Intent::Hold {
            pos,
            facing,
            allow_pursuit,
        } => {
            mix(1);
            mix(pos.x as u32 as u64);
            mix(pos.y as u32 as u64);
            mix(facing as u64);
            mix(allow_pursuit as u64);
        }
        Intent::Attack { target, approach } => {
            mix(2);
            mix(target as u64);
            mix(approach as u64);
        }
        Intent::Charge { target } => {
            mix(3);
            mix(target as u64);
        }
        Intent::Flank { target, side } => {
            mix(4);
            mix(target as u64);
            mix(side as u64);
        }
        Intent::Envelop { target } => {
            mix(5);
            mix(target as u64);
        }
        Intent::Screen { protect, side } => {
            mix(6);
            mix(protect as u64);
            mix(side as u64);
        }
        Intent::Reserve { rally_pos } => {
            mix(7);
            mix(rally_pos.x as u32 as u64);
            mix(rally_pos.y as u32 as u64);
        }
        Intent::Withdraw { to, fighting } => {
            mix(8);
            mix(to.x as u32 as u64);
            mix(to.y as u32 as u64);
            mix(fighting as u64);
        }
        Intent::Pursue {
            target,
            max_distance_m,
        } => {
            mix(9);
            mix(target as u64);
            mix(max_distance_m as u64);
        }
        Intent::ShootAt { target, mode } => {
            mix(10);
            mix(target as u64);
            mix(mode as u64);
        }
        Intent::HuntPerson { target } => {
            mix(11);
            mix(target as u64);
        }
        Intent::OccupyArea { center, radius_m } => {
            mix(12);
            mix(center.x as u32 as u64);
            mix(center.y as u32 as u64);
            mix(radius_m as u64);
        }
        Intent::GuardArea {
            center,
            radius_m,
            intercept_radius_m,
        } => {
            mix(13);
            mix(center.x as u32 as u64);
            mix(center.y as u32 as u64);
            mix(radius_m as u64);
            mix(intercept_radius_m as u64);
        }
    }
    h
}

/// 軍全体の会戦プラン。仕様 05 章 6 節。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BattlePlan {
    CenterPush,
    DoubleEnvelopment,
    RefusedFlank,
    DefendHighGround,
    FeignedRetreat,
    EchelonAttack,
    CavalryReserve,
    Delay,
}

/// 「なぜそう判断したか」の記録 1 件。仕様 05 章 7 節。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionRecord {
    pub tick: u32,
    pub chosen: &'static str,
    pub chosen_score: i32,
    /// 上位候補（選ばれたものを含む）とスコア。
    pub candidates: Vec<(&'static str, i32)>,
    /// 選ばれた候補のスコア内訳。
    pub breakdown: Vec<(&'static str, i32)>,
}

const DECISION_LOG_CAPACITY: usize = 8;

/// 直近 N 件の判断を保持するリングバッファ。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DecisionLog {
    records: std::collections::VecDeque<DecisionRecord>,
}

impl DecisionLog {
    pub fn push(&mut self, record: DecisionRecord) {
        self.records.push_back(record);
        if self.records.len() > DECISION_LOG_CAPACITY {
            self.records.pop_front();
        }
    }

    pub fn latest(&self) -> Option<&DecisionRecord> {
        self.records.back()
    }

    pub fn iter(&self) -> impl Iterator<Item = &DecisionRecord> {
        self.records.iter()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    pub soldiers: Vec<SoldierId>,
    pub troop_type: u16,
    pub formation: FormationId,
    /// 経路上を連続的に進む現在の隊列アンカー。最終目的地は `path_final`。
    pub formation_origin: Vec2Fx,
    pub formation_facing: Brad,
    pub ranks: u16,
    pub file_spacing: Fx,
    pub rank_spacing: Fx,
    pub banner: Option<u16>,
    pub formation_change: Option<FormationChange>,
    /// 残りの経路ウェイポイント（アンカーが次に目指す点が先頭）。
    pub path: Vec<Vec2Fx>,
    /// 経路探索の最終目的地。到達判定や再計算の要否に使う。
    pub path_final: Vec2Fx,
    /// 追撃中なら、その上限距離と起点（仕様 12 章 M5）。
    pub pursuit_leash: Option<PursuitLeash>,
}

/// もっとも近い敵部隊の方角。守備の正面を決めるのに使う（B5）。
///
/// 部隊の重心（`NodeStats`）だけを見るので、兵士一人ひとりを走査しない。
fn nearest_enemy_bearing(
    nodes: &[CommandNode],
    faction: FactionId,
    center: Vec2Fx,
) -> Option<Brad> {
    let mut best: Option<(i64, Vec2Fx)> = None;
    for node in nodes {
        if node.faction == faction || node.unit.is_none() || node.stats.alive == 0 {
            continue;
        }
        let d2 = sim_math::dist_sq(center, node.stats.centroid);
        if best.map_or(true, |(best_d2, _)| d2 < best_d2) {
            best = Some((d2, node.stats.centroid));
        }
    }
    best.map(|(_, centroid)| {
        // `formation_facing` はランクの伸びる向きで、正面はその 90 度手前。
        centroid
            .sub(center)
            .angle()
            .wrapping_sub(sim_math::BRAD_QUARTER as Brad)
    })
}

/// 区域の中にいる敵の人数。
fn count_enemies_inside(
    soldiers: &Soldiers,
    faction: FactionId,
    center: Vec2Fx,
    radius: i64,
) -> u16 {
    let mut count = 0u16;
    for i in 0..soldiers.len() {
        if !soldiers.is_alive(i) || soldiers.faction[i] == faction {
            continue;
        }
        if sim_math::dist_sq(soldiers.pos(i), center) <= radius * radius {
            count = count.saturating_add(1);
        }
    }
    count
}

/// 兵種でスロットを並べ替える（B4）。
///
/// スロット番号が大きいほど前列なので、射撃兵を小さい番号（後列）へ、
/// 近接兵を大きい番号（前列）へ寄せる。騎兵は隊列の中では前列側に置く。
/// 元の並びは同じ兵種の中で保つ（安定ソート）ので、隣同士の関係は崩れない。
fn sort_slots_by_role(unit: &mut Unit, soldiers: &Soldiers) {
    let rank_of = |id: &SoldierId| -> u8 {
        let Some(i) = soldiers.index_if_present(*id) else {
            return 1;
        };
        let flags = soldiers.hot.flags[i];
        if flags & crate::soldiers::flags::MISSILE_TROOP != 0 {
            0
        } else if flags & crate::soldiers::flags::ENGINEER != 0 {
            // 工兵は戦列の前へ出さない。
            0
        } else {
            1
        }
    };
    unit.soldiers.sort_by_key(rank_of);
}

/// 整列間隔と歩行間隔を `blend`（permille）で混ぜる。
fn blend_spacing(still: Fx, marching: Fx, blend: u16) -> Fx {
    still + ((marching - still) as i64 * blend as i64 / 1_000) as Fx
}

/// 穴へ踏み出すまでの遅れ（何回に 1 回動くか）。規律が高く、疲れておらず、
/// 周りが空いているほど短い。1 なら毎回動く。
fn step_out_delay(soldiers: &Soldiers, i: usize, hash: &SpatialHash) -> u32 {
    let attrs = soldiers.attrs[i];
    let mut delay = 1 + (255 - attrs.discipline as u32) / 96;
    delay += soldiers.fatigue[i] as u32 / 5_000;
    let mut neighbors = [0u32; MAX_NEIGHBORS];
    let pos = soldiers.pos(i);
    let crowd = hash.query_radius(soldiers, pos.x, pos.y, fx_from_mm(1_200), &mut neighbors);
    if crowd >= MAX_NEIGHBORS - 2 {
        delay += 1;
    }
    delay.clamp(1, 4)
}

/// `from` から `to` へ、他人をすり抜けずに通れるか。
///
/// 間の 1 点だけを見る近似。スロットの間隔は 1 m 前後なので、真ん中に体が
/// 入っていれば通れないとみなすには十分。
fn lane_is_clear(
    soldiers: &Soldiers,
    hash: &SpatialHash,
    from: Vec2Fx,
    to: Vec2Fx,
    mover: SoldierId,
) -> bool {
    let midpoint = Vec2Fx::new((from.x + to.x) / 2, (from.y + to.y) / 2);
    let mut neighbors = [0u32; MAX_NEIGHBORS];
    let count = hash.query_radius(
        soldiers,
        midpoint.x,
        midpoint.y,
        fx_from_mm(crate::soldiers::BODY_RADIUS_MM),
        &mut neighbors,
    );
    !neighbors[..count].iter().any(|&id| {
        id != mover
            && soldiers
                .index_if_present(id)
                .is_some_and(|i| soldiers.is_alive(i))
    })
}

/// 追跡隊の誰かが対象を見えているか。指揮官だけでなく部隊の誰かが近ければよい。
///
/// 走査は「見つけた時点で打ち切り」なので、密集した部隊でも実際に見るのは
/// 先頭の数人で済むことが多い。
fn unit_can_see(node: &CommandNode, soldiers: &Soldiers, target: Vec2Fx, sight: Fx) -> bool {
    let Some(unit) = &node.unit else {
        return false;
    };
    let sight_sq = (sight as i64) * (sight as i64);
    unit.soldiers.iter().any(|&id| {
        soldiers.index_if_present(id).is_some_and(|i| {
            soldiers.is_alive(i) && sim_math::dist_sq(soldiers.pos(i), target) <= sight_sq
        })
    })
}

/// 側面攻撃の目標を、敵部隊の重心から横へずらす距離（m）。真正面へ向かわせると
/// 側面へ回り込まずに正面衝突するので、いったん敵の横を目指させる。
const FLANK_OFFSET_M: i32 = 60;

/// 隊列アンカーの基準速度（mm/s）。兵士自身の移動速度より少し遅くし、
/// 前の隊列目標へ追いつく余裕を残す。
const FORMATION_ANCHOR_SPEED_MM_PER_S: i32 = 1_200;
const ADVANCE_ANCHOR_SPEED_MM_PER_S: i32 = 1_800;
const FOOT_CHARGE_ANCHOR_SPEED_MM_PER_S: i32 = 2_400;
const MOUNTED_CHARGE_ANCHOR_SPEED_MM_PER_S: i32 = 6_500;
/// 重心がアンカーから離れすぎたとき、アンカーを減速・停止する距離。
const ANCHOR_SLOW_LAG_MM: i32 = 8_000;
const ANCHOR_STOP_LAG_MM: i32 = 15_000;
/// 同じ移動目標を細かく再発令されても経路を引き直さない距離。
const REPATH_DISTANCE_MM: i32 = 4_000;
/// 区域内の配置を一方向の格子にせず、各兵士を円盤へ分散させる角度。
/// 137.5 度に近い brad 値で、隣り合うスロットが同じ方向へ並ばない。
const AREA_SPIRAL_STEP_BRAD: u16 = 25_031;

/// 歩行中に保つ最低間隔。停止して整列したあとは Unit 固有の元の間隔へ戻す。
const MARCH_FILE_SPACING_MIN_MM: i32 = 900;
const MARCH_RANK_SPACING_MIN_MM: i32 = 1_100;
/// 歩行間隔と整列間隔の混ざり具合が 1 tick に動ける量（permille）。
/// 40 なら 0〜1000 まで 25 tick = 1.25 秒かけて移り変わる。停止した瞬間に
/// 全員の目標が跳ぶと、隊列が一斉に横滑りしてしまう（B4）。
const MARCH_BLEND_STEP_PERMILLE: u16 = 40;

/// 戦列の穴は一度に全段を詰めず、一定間隔で直後の一人だけが前へ出る。
const VACANCY_FILL_INTERVAL_TICKS: u32 = 10;
const MAX_VACANCY_FILLS_PER_INTERVAL: usize = 8;

/// 前後列の交代を検討する間隔（tick）。20 Hz なので 40 tick = 2 秒。
const ROTATION_INTERVAL_TICKS: u32 = 40;
/// 前列の兵士がこの疲労を超えたら、交代の対象になる。
const ROTATION_FATIGUE_THRESHOLD: u16 = 5_500;
/// 交代する意味があるとみなす疲労差。
const ROTATION_FATIGUE_MARGIN: u16 = 1_500;
/// 1 回の判定で入れ替える人数の上限。列がまるごと入れ替わって陣形が溶けない
/// ようにするためと、部隊が大きくても走査を O(人数) に抑えるため。
const MAX_ROTATIONS_PER_INTERVAL: usize = 4;

/// 追撃の紐（leash）。仕様 12 章 M5「追撃に出た騎兵が戻ってくるのに
/// 現実的な時間がかかる」を実装するための、部隊単位の上限距離。
/// 任務がいまどの段階にあるか（B5）。
///
/// 「命令を受けた」から「達成した／諦めた」までを段階として持つ。条件付き命令
/// （敵接近時、損耗率、地点喪失時）は、この段階の変化を引き金として後から
/// 接続できる。
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MissionState {
    /// 任務を受けていない。
    #[default]
    Idle,
    /// 目標へ移動している。
    Moving,
    /// 目標に着いて、隊形・区域へ展開している。
    Deploying,
    /// 交戦している。
    Engaged,
    /// 達成した（地点を確保した、対象を倒した）。
    Secured,
    /// 達成できないと判断した（見失った、追い出された）。
    Failed,
    /// 持ち場・元の任務へ戻っている。
    Returning,
}

impl MissionState {
    /// JSON と UI が使う安定した識別子。
    pub fn id(self) -> &'static str {
        match self {
            MissionState::Idle => "idle",
            MissionState::Moving => "moving",
            MissionState::Deploying => "deploying",
            MissionState::Engaged => "engaged",
            MissionState::Secured => "secured",
            MissionState::Failed => "failed",
            MissionState::Returning => "returning",
        }
    }
}

/// 任務の進み具合。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MissionProgress {
    pub state: MissionState,
    /// この段階に入った tick。滞在時間の判定に使う。
    pub since_tick: u32,
    /// 地点任務で、範囲の中にいる味方と敵の人数（直近の評価時点）。
    pub inside_friendly: u16,
    pub inside_enemy: u16,
}

/// 地点を「確保した」とみなすのに必要な、範囲内にいる味方の割合（permille）。
const OCCUPY_SECURED_PERMILLE: u32 = 600;
/// 確保の判定に必要な滞在時間（tick）。20 Hz なので 100 tick = 5 秒。
const OCCUPY_HOLD_TICKS: u32 = 100;
/// 任務の段階を見直す間隔（tick）。
const MISSION_REVIEW_TICKS: u32 = 10;
/// 地点任務で、区域の中に置く人数の目安（1 人あたりの面積 m²）。これを超える
/// 分は周縁の警戒に回す。
const AREA_SQUARE_METRES_PER_SOLDIER: u32 = 4;
/// 周縁警戒の兵士を置く輪の半径（区域半径に対する permille）。
const AREA_PERIMETER_PERMILLE: u32 = 1_150;

/// 人物追跡の記憶（B1）。
///
/// 追跡隊は対象の現在位置を常に知っているわけではない。見えている間だけ
/// 最後の目撃位置を更新し、見失ったら「最後に見た場所」を目指す。そこへ着いても
/// 見つからない、あるいは長く見失ったままなら追跡を打ち切る。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HuntMemory {
    pub target: SoldierId,
    /// 最後に対象を見た位置。
    pub last_seen: Vec2Fx,
    /// その tick。ここからの経過が打ち切りの目安になる。
    pub last_seen_tick: u32,
    pub state: HuntState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HuntState {
    /// 見えているので、現在位置を追える。
    Tracking,
    /// 見失った。最後に見た場所を探している。
    Searching,
    /// 探しても見つからず打ち切った。
    Lost,
    /// 対象が倒れた。任務は達成。
    TargetDown,
}

/// 追跡隊の誰かがこの距離まで近づいていれば、対象が見えているとみなす（m）。
const HUNT_SIGHT_RADIUS_M: i32 = 40;
/// 見失ってから追跡を打ち切るまで（tick）。20 Hz なので 400 tick = 20 秒。
const HUNT_GIVE_UP_TICKS: u32 = 400;
/// 最後の目撃地点へ着いた後、周囲を探す時間（tick）。5 秒。
const HUNT_SEARCH_TICKS: u32 = 100;
/// 追跡の見直し間隔（tick）。ノード ID で位相を散らす。
const HUNT_UPDATE_TICKS: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PursuitLeash {
    /// 追撃を開始した時点の部隊の位置。呼び戻し先にもなる。
    pub origin: Vec2Fx,
    pub max_distance_m: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormationChange {
    pub from: FormationId,
    pub to: FormationId,
    pub started_tick: u32,
    pub complete_tick: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandNode {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub echelon: u8,
    pub faction: FactionId,
    pub commander: SoldierId,
    pub deputies: Vec<SoldierId>,
    pub command_state: CommandState,
    pub objective: Option<Intent>,
    pub received_order: Option<Order>,
    pub pending_orders: Vec<InFlightOrder>,
    pub blackboard: Blackboard,
    pub stats: NodeStats,
    pub unit: Option<Unit>,
    succession_end_tick: Option<u32>,
    /// M7: この指揮官の性格（`objective`/`Order` とは別軸、戦略層の判断だけに効く）。
    pub commander_attrs: CommanderAttrs,
    /// M7: この指揮官が選んだ戦略目的（非葉ノード）。子への `Order` の元になる。
    pub strategic_objective: Option<Objective>,
    /// M7: 軍全体の会戦プラン。ルートノード（`parent.is_none()`）だけが持つ。
    pub battle_plan: Option<BattlePlan>,
    /// M7: 直近の判断（上位候補・スコア内訳）。仕様 05 章 7 節「なぜそうしたか」。
    pub decision_log: DecisionLog,
    /// M7: 次に戦略層の思考をする tick（位相分散、`crate::commander_ai` が使う）。
    pub(crate) next_assess_tick: u32,
    /// B5: 任務の進み具合。条件付き命令はここの変化を引き金にできる。
    pub mission: MissionProgress,
    /// B1: 人物追跡の記憶。`objective` が `HuntPerson` のときだけ入る。
    pub hunt: Option<HuntMemory>,
    /// 追跡を打ち切ったときに戻る任務。追跡を受ける直前の任務。
    pub hunt_fallback: Option<Intent>,
}

#[derive(Clone, Debug, Default)]
pub struct CommandTree {
    /// ノードごとの「歩行間隔へどれだけ寄っているか」（permille）。停止と行軍の
    /// あいだを滑らかに行き来させるための状態（B4）。
    pub(crate) march_blend: Vec<u16>,
    /// 兵種でスロットを並べ替え済みか（B4）。並べ替えは編成のたびに 1 回だけ。
    pub(crate) role_sorted: Vec<bool>,
    pub nodes: Vec<CommandNode>,
    pub messengers: Vec<Messenger>,
    pub events: Vec<CommandEvent>,
    /// 前後列の交代が起きた回数（UI とテスト用）。
    pub rotations: u32,
    next_order_id: OrderId,
}

impl CommandTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn node(&self, id: NodeId) -> Option<&CommandNode> {
        self.nodes.get(id as usize)
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut CommandNode> {
        self.nodes.get_mut(id as usize)
    }

    pub fn add_node(
        &mut self,
        parent: Option<NodeId>,
        echelon: u8,
        faction: FactionId,
        commander: SoldierId,
        deputies: Vec<SoldierId>,
        unit: Option<Unit>,
    ) -> NodeId {
        let id = self.nodes.len() as NodeId;
        self.nodes.push(CommandNode {
            id,
            parent,
            children: Vec::new(),
            echelon,
            faction,
            commander,
            deputies,
            command_state: CommandState::Commanded,
            objective: None,
            received_order: None,
            pending_orders: Vec::new(),
            blackboard: Blackboard::default(),
            stats: NodeStats::default(),
            unit,
            succession_end_tick: None,
            commander_attrs: CommanderAttrs::default(),
            strategic_objective: None,
            battle_plan: None,
            decision_log: DecisionLog::default(),
            // 位相を NodeId でずらし、全ノードが同じ tick に集中して思考しないようにする
            // （仕様 05 章 2 節の think_at と同じ考え方）。
            next_assess_tick: id % crate::commander_ai::ASSESS_INTERVAL_TICKS,
            mission: MissionProgress::default(),
            hunt: None,
            hunt_fallback: None,
        });
        if let Some(parent_id) = parent {
            if let Some(parent_node) = self.nodes.get_mut(parent_id as usize) {
                parent_node.children.push(id);
            }
        }
        id
    }

    /// 発令元から対象までのツリー上の各区間に伝令を作る。
    pub fn issue_order(
        &mut self,
        issuer: NodeId,
        target: NodeId,
        intent: Intent,
        priority: Priority,
        tick: u32,
        soldiers: &Soldiers,
    ) -> Option<OrderId> {
        self.issue_order_via(
            issuer,
            target,
            intent,
            priority,
            DeliveryMethod::Messenger,
            tick,
            soldiers,
        )
    }

    /// 伝令・旗・角笛のいずれかを選んで命令を送る。旗と角笛は限定語彙のみ、
    /// かつ仕様の距離制限を満たす場合に利用できる。
    #[allow(clippy::too_many_arguments)]
    pub fn issue_order_via(
        &mut self,
        issuer: NodeId,
        target: NodeId,
        intent: Intent,
        priority: Priority,
        method: DeliveryMethod,
        tick: u32,
        soldiers: &Soldiers,
    ) -> Option<OrderId> {
        if self.node(issuer).is_none()
            || self.node(target).is_none()
            || !self.is_descendant(target, issuer)
        {
            return None;
        }
        if method != DeliveryMethod::Messenger {
            let distance = dist(
                self.commander_pos(issuer, soldiers),
                self.commander_pos(target, soldiers),
            );
            let limit = match method {
                DeliveryMethod::Flag => fx(800),
                DeliveryMethod::Acoustic => fx(400),
                DeliveryMethod::Messenger => 0,
            };
            if distance > limit || !Self::signal_intent(intent) {
                return None;
            }
        }
        let id = self.next_order_id;
        self.next_order_id = self.next_order_id.wrapping_add(1);
        let order = Order {
            id,
            issued_tick: tick,
            issuer,
            target,
            intent,
            priority,
            expires_tick: None,
        };
        self.events.push(CommandEvent {
            tick,
            node: issuer,
            order: Some(id),
            kind: CommandEventKind::Issued,
        });
        self.queue_next_hop(order, issuer, method, tick, soldiers)
    }

    fn signal_intent(intent: Intent) -> bool {
        matches!(
            intent,
            Intent::MoveTo { .. }
                | Intent::Hold { .. }
                | Intent::Charge { .. }
                | Intent::Withdraw { .. }
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn issue_order_with_expiry(
        &mut self,
        issuer: NodeId,
        target: NodeId,
        intent: Intent,
        priority: Priority,
        expires_tick: Option<u32>,
        tick: u32,
        soldiers: &Soldiers,
    ) -> Option<OrderId> {
        let id = self.issue_order(issuer, target, intent, priority, tick, soldiers)?;
        // The order is already queued; update its immutable copy in the messenger.
        for messenger in &mut self.messengers {
            if messenger.order.id == id {
                messenger.order.expires_tick = expires_tick;
            }
        }
        for node in &mut self.nodes {
            for pending in &mut node.pending_orders {
                if pending.order.id == id {
                    pending.order.expires_tick = expires_tick;
                }
            }
        }
        Some(id)
    }

    fn queue_next_hop(
        &mut self,
        order: Order,
        from: NodeId,
        method: DeliveryMethod,
        tick: u32,
        soldiers: &Soldiers,
    ) -> Option<OrderId> {
        let to = if from == order.target {
            from
        } else {
            self.next_hop(from, order.target)?
        };
        let origin = self.commander_pos(from, soldiers);
        let destination = self.commander_pos(to, soldiers);
        let (courier, travel) = if method == DeliveryMethod::Messenger {
            let courier = self.choose_courier(from, soldiers)?;
            let distance = dist(origin, destination);
            let speed = per_sec_to_per_tick(fx_from_mm(9000)).max(1);
            let travel = ((distance as i64 + speed as i64 - 1) / speed as i64) as u32;
            (courier, travel)
        } else {
            (crate::soldiers::NO_ID, 0)
        };
        let signal_delay = match method {
            DeliveryMethod::Messenger => ms_to_ticks(10_000),
            DeliveryMethod::Flag => ms_to_ticks(5_000),
            DeliveryMethod::Acoustic => ms_to_ticks(3_000),
        };
        let messenger_id = self.messengers.len();
        let total_ticks = if method == DeliveryMethod::Messenger {
            signal_delay
                .saturating_add(travel)
                .saturating_add(signal_delay)
        } else {
            signal_delay
        };
        self.messengers.push(Messenger {
            soldier: courier,
            order,
            from,
            to,
            state: MessengerState::Riding,
            method,
            remaining_ticks: total_ticks,
            total_ticks,
            position: origin,
            destination,
        });
        if let Some(node) = self.node_mut(from) {
            node.pending_orders.push(InFlightOrder {
                order,
                messenger: messenger_id,
                next_hop: to,
            });
        }
        let _ = tick;
        Some(order.id)
    }

    fn commander_pos(&self, id: NodeId, soldiers: &Soldiers) -> Vec2Fx {
        self.node(id)
            .and_then(|n| soldiers.pos_checked(n.commander))
            .unwrap_or(Vec2Fx::ZERO)
    }

    fn choose_courier(&self, from: NodeId, soldiers: &Soldiers) -> Option<SoldierId> {
        let node = self.node(from)?;
        if soldiers.is_active_id(node.commander) {
            return Some(node.commander);
        }
        self.subtree_soldiers(from)
            .into_iter()
            .find(|&id| soldiers.is_active_id(id))
    }

    pub fn subtree_soldiers(&self, id: NodeId) -> Vec<SoldierId> {
        let Some(node) = self.node(id) else {
            return Vec::new();
        };
        if let Some(unit) = &node.unit {
            return unit.soldiers.clone();
        }
        let children = node.children.clone();
        let mut result = Vec::new();
        for child in children {
            result.extend(self.subtree_soldiers(child));
        }
        result
    }

    fn is_descendant(&self, node: NodeId, ancestor: NodeId) -> bool {
        if node == ancestor {
            return true;
        }
        let mut current = self.node(node).and_then(|n| n.parent);
        while let Some(id) = current {
            if id == ancestor {
                return true;
            }
            current = self.node(id).and_then(|n| n.parent);
        }
        false
    }

    fn next_hop(&self, from: NodeId, target: NodeId) -> Option<NodeId> {
        if from == target {
            return Some(from);
        }
        let mut current = target;
        let mut parent = self.node(current)?.parent?;
        while parent != from {
            current = parent;
            parent = self.node(current)?.parent?;
        }
        Some(current)
    }

    /// 伝令の移動、命令の受領、指揮継承を進める。
    pub fn tick(&mut self, soldiers: &mut Soldiers, terrain: &Terrain, tick: u32) {
        self.tick_succession(soldiers, tick);
        let mut arrivals = Vec::new();
        let mut lost = Vec::new();
        for (index, messenger) in self.messengers.iter_mut().enumerate() {
            if !matches!(
                messenger.state,
                MessengerState::Riding | MessengerState::Delivering
            ) {
                continue;
            }
            if messenger.method == DeliveryMethod::Messenger
                && !soldiers.is_active_id(messenger.soldier)
            {
                messenger.state = MessengerState::Lost;
                lost.push((messenger.to, messenger.order.id));
                continue;
            }
            if messenger.remaining_ticks > 0 {
                messenger.remaining_ticks -= 1;
            }
            let elapsed = messenger
                .total_ticks
                .saturating_sub(messenger.remaining_ticks);
            let progress = if messenger.total_ticks == 0 {
                FX_ONE
            } else {
                ((elapsed as i64 * FX_ONE as i64) / messenger.total_ticks as i64) as Fx
            };
            messenger.position = messenger.position.lerp(messenger.destination, progress);
            if messenger.remaining_ticks == 0 {
                messenger.state = MessengerState::Delivered;
                arrivals.push(index);
            }
        }
        for (node, order) in lost {
            self.events.push(CommandEvent {
                tick,
                node,
                order: Some(order),
                kind: CommandEventKind::MessengerLost,
            });
        }
        for index in arrivals {
            self.deliver(index, soldiers, terrain, tick);
        }
        self.update_stats(soldiers);
        self.tick_mission_state(soldiers, tick);
        self.tick_dynamic_objectives(soldiers, terrain, tick);
        self.tick_pursuit(soldiers, terrain);
    }

    /// 任務の段階（[`MissionState`]）を進める（B5）。
    ///
    /// 「移動中 → 展開中 → 交戦中 → 確保済み／失敗／帰還中」を、部隊の位置・
    /// 交戦・地点の中身から決める。段階が変わったら事象として残すので、条件付き
    /// 命令は後からここへ接続できる。
    fn tick_mission_state(&mut self, soldiers: &Soldiers, tick: u32) {
        let mut changes = Vec::new();
        for node in &self.nodes {
            if (tick + node.id) % MISSION_REVIEW_TICKS != 0 {
                continue;
            }
            let Some(unit) = &node.unit else {
                continue;
            };
            let Some(objective) = node.objective else {
                if node.mission.state != MissionState::Idle {
                    changes.push((node.id, MissionState::Idle, 0, 0));
                }
                continue;
            };
            let alive: Vec<usize> = unit
                .soldiers
                .iter()
                .filter_map(|&id| soldiers.index_if_present(id))
                .filter(|&i| soldiers.is_alive(i))
                .collect();
            if alive.is_empty() {
                if node.mission.state != MissionState::Failed {
                    changes.push((node.id, MissionState::Failed, 0, 0));
                }
                continue;
            }
            let fighting = alive
                .iter()
                .filter(|&&i| soldiers.hot.state[i].is_fighting())
                .count();
            let arrived = unit.path.is_empty()
                && dist(unit.formation_origin, unit.path_final) <= pathing::arrival_radius();

            let (state, inside_friendly, inside_enemy) = match objective {
                Intent::OccupyArea { center, radius_m }
                | Intent::GuardArea {
                    center, radius_m, ..
                } => {
                    let radius = fx(radius_m as i32) as i64;
                    let inside = alive
                        .iter()
                        .filter(|&&i| sim_math::dist_sq(soldiers.pos(i), center) <= radius * radius)
                        .count() as u16;
                    let enemies = count_enemies_inside(soldiers, node.faction, center, radius);
                    let held =
                        inside as u32 * 1_000 >= alive.len() as u32 * OCCUPY_SECURED_PERMILLE;
                    let state = if fighting > 0 || enemies > 0 {
                        MissionState::Engaged
                    } else if !arrived {
                        MissionState::Moving
                    } else if !held {
                        // 着いてはいるが、まだ範囲へ散りきっていない。
                        MissionState::Deploying
                    } else if node.mission.state == MissionState::Secured {
                        // 一度確保したら、追い出されるまで確保済みのまま。
                        MissionState::Secured
                    } else if node.mission.state == MissionState::Deploying
                        && tick.saturating_sub(node.mission.since_tick) >= OCCUPY_HOLD_TICKS
                    {
                        // 十分な人数が範囲に留まった時間で確保とみなす。
                        MissionState::Secured
                    } else {
                        MissionState::Deploying
                    };
                    (state, inside, enemies)
                }
                Intent::HuntPerson { .. } => {
                    let state = match node.hunt.map(|hunt| hunt.state) {
                        Some(HuntState::Tracking) => MissionState::Moving,
                        Some(HuntState::Searching) => MissionState::Deploying,
                        Some(HuntState::TargetDown) => MissionState::Secured,
                        Some(HuntState::Lost) => MissionState::Failed,
                        None => MissionState::Idle,
                    };
                    (state, 0, 0)
                }
                Intent::Withdraw { .. } | Intent::Reserve { .. } => {
                    let state = if arrived {
                        MissionState::Returning
                    } else {
                        MissionState::Moving
                    };
                    (state, 0, 0)
                }
                _ => {
                    let state = if fighting > 0 {
                        MissionState::Engaged
                    } else if !arrived {
                        MissionState::Moving
                    } else {
                        MissionState::Deploying
                    };
                    (state, 0, 0)
                }
            };
            if state != node.mission.state
                || inside_friendly != node.mission.inside_friendly
                || inside_enemy != node.mission.inside_enemy
            {
                changes.push((node.id, state, inside_friendly, inside_enemy));
            }
        }
        for (node_id, state, inside_friendly, inside_enemy) in changes {
            let changed = self
                .node(node_id)
                .is_some_and(|node| node.mission.state != state);
            if let Some(node) = self.node_mut(node_id) {
                if changed {
                    node.mission.state = state;
                    node.mission.since_tick = tick;
                }
                node.mission.inside_friendly = inside_friendly;
                node.mission.inside_enemy = inside_enemy;
            }
            if changed {
                self.events.push(CommandEvent {
                    tick,
                    node: node_id,
                    order: None,
                    kind: CommandEventKind::MissionStateChanged,
                });
            }
        }
    }

    /// 人物追跡を、記憶（[`HuntMemory`]）に沿って 1 段進める。
    ///
    /// 追跡隊は対象の現在位置を常に知っているわけではない。誰かが
    /// [`HUNT_SIGHT_RADIUS_M`] 以内にいる間だけ最後の目撃位置を更新し、見失えば
    /// そこへ向かって探す。対象が倒れる、長く見失う、探しても見つからない場合は
    /// 追跡を打ち切り、直前の任務——無ければその場の確保——へ戻す。止まったまま
    /// 放置しない。
    fn tick_dynamic_objectives(&mut self, soldiers: &mut Soldiers, terrain: &Terrain, tick: u32) {
        let sight = fx(HUNT_SIGHT_RADIUS_M);
        let mut updates = Vec::new();
        let mut finished = Vec::new();
        for node in &self.nodes {
            let Some(Intent::HuntPerson { target }) = node.objective else {
                continue;
            };
            // 見直しはノードごとに位相をずらした周期で行う。
            if (tick + node.id) % HUNT_UPDATE_TICKS != 0 {
                continue;
            }
            let Some(mut memory) = node.hunt else {
                continue;
            };
            if !soldiers.is_active_id(target) {
                memory.state = HuntState::TargetDown;
                finished.push((node.id, memory));
                continue;
            }
            let target_pos = soldiers.pos(target as usize);
            if unit_can_see(node, soldiers, target_pos, sight) {
                memory.last_seen = target_pos;
                memory.last_seen_tick = tick;
                memory.state = HuntState::Tracking;
                updates.push((node.id, memory, memory.last_seen));
                continue;
            }
            memory.state = HuntState::Searching;
            let elapsed = tick.saturating_sub(memory.last_seen_tick);
            let searched = node.unit.as_ref().is_some_and(|unit| {
                dist(unit.formation_origin, memory.last_seen) <= pathing::arrival_radius()
            }) && elapsed >= HUNT_SEARCH_TICKS;
            if elapsed >= HUNT_GIVE_UP_TICKS || searched {
                memory.state = HuntState::Lost;
                finished.push((node.id, memory));
            } else {
                updates.push((node.id, memory, memory.last_seen));
            }
        }
        for (node_id, memory, destination) in updates {
            if let Some(node) = self.node_mut(node_id) {
                node.hunt = Some(memory);
            }
            self.set_movement_target(node_id, destination, soldiers, terrain);
        }
        for (node_id, memory) in finished {
            self.finish_hunt(node_id, memory, soldiers, terrain, tick);
        }
    }

    /// 追跡を終える。直前の任務があればそこへ戻り、無ければ現在地の確保へ移る。
    fn finish_hunt(
        &mut self,
        node_id: NodeId,
        memory: HuntMemory,
        soldiers: &mut Soldiers,
        terrain: &Terrain,
        tick: u32,
    ) {
        let Some(node) = self.node_mut(node_id) else {
            return;
        };
        node.hunt = Some(memory);
        let fallback = node.hunt_fallback.take();
        let here = node
            .unit
            .as_ref()
            .map(|unit| unit.formation_origin)
            .unwrap_or(node.stats.centroid);
        let facing = node
            .unit
            .as_ref()
            .map(|unit| unit.formation_facing)
            .unwrap_or(0);
        if let Some(unit) = node.unit.as_mut() {
            unit.path.clear();
            unit.path_final = here;
        }
        let next = fallback.unwrap_or(Intent::Hold {
            pos: here,
            facing,
            allow_pursuit: false,
        });
        self.set_objective(node_id, next, soldiers, terrain, tick);
        // 打ち切った理由は残す（UI と回帰指標が「なぜ止めたか」を読めるように）。
        // 次に同じ人物の追跡を受け直したときは、終わった記憶ではなく新しい記憶で
        // 始める（`record_objective` が判定する）。
        if let Some(node) = self.node_mut(node_id) {
            node.hunt = Some(memory);
        }
        self.events.push(CommandEvent {
            tick,
            node: node_id,
            order: None,
            kind: match memory.state {
                HuntState::TargetDown => CommandEventKind::ObjectiveComplete,
                _ => CommandEventKind::ObjectiveAbandoned,
            },
        });
    }

    /// 追撃の紐を確認し、上限距離（規律が低いほど超過を許す）を超えた部隊を
    /// 起点へ呼び戻す。呼び戻しは通常の移動命令と同じ経路探索・速度で進むので、
    /// 「戻ってくるのに現実的な時間がかかる」が自然に満たされる。
    fn tick_pursuit(&mut self, soldiers: &Soldiers, terrain: &Terrain) {
        let node_ids: Vec<NodeId> = self.nodes.iter().map(|n| n.id).collect();
        for node_id in node_ids {
            let Some(node) = self.node(node_id) else {
                continue;
            };
            let Some(unit) = &node.unit else {
                continue;
            };
            let Some(leash) = unit.pursuit_leash else {
                continue;
            };
            let centroid = if node.stats.alive > 0 {
                node.stats.centroid
            } else {
                unit.formation_origin
            };
            let overrun_m = self.discipline_overrun_m(node_id, soldiers);
            let effective_max = fx((leash.max_distance_m as i32 + overrun_m).max(0));
            if dist(centroid, leash.origin) <= effective_max {
                continue;
            }
            self.set_movement_target(node_id, leash.origin, soldiers, terrain);
            if let Some(node) = self.node_mut(node_id) {
                if let Some(unit) = node.unit.as_mut() {
                    unit.pursuit_leash = None;
                }
            }
        }
    }

    /// 規律が低い部隊ほど、追撃の紐を大きく超過して深追いする
    /// （仕様「discipline が低い部隊は制限を超えて追う」）。最大 100 m。
    fn discipline_overrun_m(&self, node_id: NodeId, soldiers: &Soldiers) -> i32 {
        let Some(unit) = self.node(node_id).and_then(|n| n.unit.as_ref()) else {
            return 0;
        };
        let mut sum = 0i32;
        let mut count = 0i32;
        for &id in &unit.soldiers {
            if let Some(i) = soldiers.index_if_present(id) {
                sum += soldiers.attrs[i].discipline as i32;
                count += 1;
            }
        }
        if count == 0 {
            return 0;
        }
        let avg = sum / count;
        (255 - avg) * 100 / 255
    }

    fn deliver(
        &mut self,
        messenger_id: usize,
        soldiers: &mut Soldiers,
        terrain: &Terrain,
        tick: u32,
    ) {
        let messenger = self.messengers[messenger_id];
        let node_id = messenger.to;
        let Some(node) = self.node(node_id) else {
            return;
        };
        let state = node.command_state;
        let commander = node.commander;
        if let Some(node) = self.node_mut(node_id) {
            node.pending_orders.retain(|o| o.messenger != messenger_id);
        }
        self.events.push(CommandEvent {
            tick,
            node: node_id,
            order: Some(messenger.order.id),
            kind: CommandEventKind::Received,
        });
        if state != CommandState::Commanded || !soldiers.is_active_id(commander) {
            self.events.push(CommandEvent {
                tick,
                node: node_id,
                order: Some(messenger.order.id),
                kind: CommandEventKind::Ignored,
            });
            return;
        }
        let compliance = self.interpret(node_id, messenger.order, soldiers, tick);
        if let Some(node) = self.node_mut(node_id) {
            node.received_order = Some(messenger.order);
        }
        if compliance != Compliance::Ignored {
            self.record_objective(node_id, messenger.order.intent, soldiers, tick);
        }
        if compliance != Compliance::Ignored {
            self.apply_intent(node_id, messenger.order.intent, soldiers, terrain, tick);
        }
        let kind = match compliance {
            Compliance::Obeyed => CommandEventKind::Obeyed,
            Compliance::Partial => CommandEventKind::Partial,
            Compliance::Ignored => CommandEventKind::Ignored,
        };
        self.events.push(CommandEvent {
            tick,
            node: node_id,
            order: Some(messenger.order.id),
            kind,
        });
        if compliance != Compliance::Ignored && node_id != messenger.order.target {
            let _ = self.queue_next_hop(messenger.order, node_id, messenger.method, tick, soldiers);
        }
    }

    /// 指揮官の性格を設定する。生成後に呼ぶ想定（`add_node` の引数を増やさない
    /// ためのセッター）。
    pub fn set_commander_attrs(&mut self, node_id: NodeId, attrs: CommanderAttrs) {
        if let Some(node) = self.node_mut(node_id) {
            node.commander_attrs = attrs;
        }
    }

    /// 上位からの命令を経由せず、この指揮官自身の判断で `intent` を即座に
    /// 適用する（M7 の独断専行、仕様 05 章「ambition の高い騎士が命令を
    /// 無視して突撃する」）。通常の `issue_order` と違い、伝令の遅延も
    /// `interpret` による服従判定も経ない——これは他者からの命令ではなく、
    /// 指揮官自身がその場で下した決断だから。
    pub fn override_intent(
        &mut self,
        node_id: NodeId,
        intent: Intent,
        soldiers: &mut Soldiers,
        terrain: &Terrain,
        tick: u32,
    ) {
        self.set_objective(node_id, intent, soldiers, terrain, tick);
        self.events.push(CommandEvent {
            tick,
            node: node_id,
            order: None,
            kind: CommandEventKind::Insubordination,
        });
    }

    /// 伝令を介さずに、その場で任務を設定して適用する。
    ///
    /// ワールドの初期配置——シナリオや回帰テストが「この部隊はこの任務から
    /// 始まる」と決める場面——のための入口。会戦中の命令は伝令の遅延を通る
    /// [`CommandTree::issue_order`] を使うこと。独断専行は
    /// [`CommandTree::override_intent`] がこの上に事象記録を足す。
    pub fn set_objective(
        &mut self,
        node_id: NodeId,
        intent: Intent,
        soldiers: &mut Soldiers,
        terrain: &Terrain,
        tick: u32,
    ) {
        self.record_objective(node_id, intent, soldiers, tick);
        self.apply_intent(node_id, intent, soldiers, terrain, tick);
    }

    /// 任務を記録する。人物追跡なら記憶を作り、打ち切ったときに戻る任務も覚える。
    ///
    /// 発令時点では対象の位置を「報告された位置」として受け取る——命令を出した
    /// 指揮官はそこに居ると思っている——が、以降は追跡隊自身が見えている間だけ
    /// 更新する。
    fn record_objective(
        &mut self,
        node_id: NodeId,
        intent: Intent,
        soldiers: &Soldiers,
        tick: u32,
    ) {
        let reported = match intent {
            Intent::HuntPerson { target } => soldiers.pos_checked(target),
            _ => None,
        };
        let Some(node) = self.node_mut(node_id) else {
            return;
        };
        let previous = node.objective;
        node.objective = Some(intent);
        match intent {
            Intent::HuntPerson { target } => {
                // 同じ人物を続けて追っている間だけ記憶を引き継ぐ。打ち切り済み
                // （`Lost` / `TargetDown`）の記憶からは再開しない。
                let continuing = matches!(
                    node.hunt,
                    Some(HuntMemory { target: t, state: HuntState::Tracking | HuntState::Searching, .. }) if t == target
                );
                if !continuing {
                    node.hunt_fallback =
                        previous.filter(|p| !matches!(p, Intent::HuntPerson { .. }));
                    node.hunt = Some(HuntMemory {
                        target,
                        last_seen: reported.unwrap_or_else(|| {
                            node.unit
                                .as_ref()
                                .map(|u| u.formation_origin)
                                .unwrap_or(node.stats.centroid)
                        }),
                        last_seen_tick: tick,
                        state: HuntState::Tracking,
                    });
                }
            }
            _ => {
                node.hunt = None;
                node.hunt_fallback = None;
            }
        }
    }

    fn apply_intent(
        &mut self,
        node_id: NodeId,
        intent: Intent,
        soldiers: &mut Soldiers,
        terrain: &Terrain,
        tick: u32,
    ) {
        let mut requested_formation = None;
        let mut route: Option<Vec2Fx> = None;
        // ノードを可変借用する前に、他ノード（Charge/Pursue の対象）の重心、
        // 追跡対象の現在位置、この部隊の現在地を読んでおく。
        let target_centroid = match intent {
            Intent::Charge { target }
            | Intent::Pursue { target, .. }
            | Intent::Attack { target, .. } => self.node(target).map(|n| n.stats.centroid),
            Intent::HuntPerson { target } => soldiers
                .is_active_id(target)
                .then(|| soldiers.pos(target as usize)),
            // 側面攻撃は敵の重心そのものではなく、その横を目標にする。
            Intent::Flank { target, side } => self.node(target).map(|n| {
                let (sin, cos) = (
                    sim_math::sin_fx(n.stats.facing),
                    sim_math::cos_fx(n.stats.facing),
                );
                // 部隊の正面が (-sin, cos) なので、その右手は (cos, sin)。
                let right = Vec2Fx::new(cos, sin);
                let offset = fx(FLANK_OFFSET_M);
                match side {
                    Side::Right => n.stats.centroid.add(right.scale(offset)),
                    Side::Left => n.stats.centroid.sub(right.scale(offset)),
                }
            }),
            _ => None,
        };
        let pursuit_origin =
            matches!(intent, Intent::Pursue { .. }).then(|| self.unit_origin(node_id, soldiers));
        if let Some(node) = self.node_mut(node_id) {
            let Some(unit) = node.unit.as_mut() else {
                return;
            };
            if !matches!(intent, Intent::Pursue { .. }) {
                unit.pursuit_leash = None;
            }
            match intent {
                Intent::MoveTo {
                    pos,
                    facing,
                    formation,
                    ..
                } => {
                    unit.formation_facing = facing;
                    requested_formation = Some(formation);
                    route = Some(pos);
                }
                Intent::Hold { pos, facing, .. } => {
                    unit.formation_facing = facing;
                    route = Some(pos);
                }
                Intent::Reserve { rally_pos } | Intent::Withdraw { to: rally_pos, .. } => {
                    route = Some(rally_pos);
                }
                Intent::OccupyArea { center, .. } | Intent::GuardArea { center, .. } => {
                    route = Some(center);
                }
                Intent::Charge { .. } => {
                    unit.pursuit_leash = None;
                    if let Some(centroid) = target_centroid {
                        route = Some(centroid);
                    }
                    // 騎乗している者は突撃モメンタムの積み上げへ、徒歩の者は
                    // 通常より積極的な前進状態へ（仕様 12 章 M5、06 章 4.1 節）。
                    for &id in &unit.soldiers {
                        let Some(i) = soldiers.index_if_present(id) else {
                            continue;
                        };
                        if !soldiers.is_active_id(id) {
                            continue;
                        }
                        soldiers.hot.state[i] =
                            if soldiers.hot.flags[i] & crate::soldiers::flags::MOUNTED != 0 {
                                State::Charging
                            } else {
                                State::Advancing
                            };
                    }
                }
                // 攻撃・側面攻撃も部隊を動かす。ここで経路を与えないと、命令を
                // 受けた部隊がその場に立ち尽くす——指揮官 AI の `Envelop` は
                // 子へ Attack/Flank/Charge を配るので、騎兵以外は誰も動かない
                // ことになってしまう（憑依 UI の「攻撃」「側面」も同じ）。
                Intent::Attack { .. } | Intent::Flank { .. } => {
                    if let Some(destination) = target_centroid {
                        route = Some(destination);
                    }
                }
                Intent::HuntPerson { .. } => {
                    if let Some(destination) = target_centroid {
                        route = Some(destination);
                    }
                    for &id in &unit.soldiers {
                        let Some(i) = soldiers.index_if_present(id) else {
                            continue;
                        };
                        if soldiers.is_active_id(id)
                            && matches!(
                                soldiers.hot.state[i],
                                State::Idle | State::Marching | State::Repositioning
                            )
                        {
                            soldiers.hot.state[i] = State::Advancing;
                        }
                    }
                }
                Intent::Pursue { max_distance_m, .. } => {
                    if let Some(centroid) = target_centroid {
                        route = Some(centroid);
                    }
                    if let Some(origin) = pursuit_origin {
                        unit.pursuit_leash = Some(PursuitLeash {
                            origin,
                            max_distance_m,
                        });
                    }
                }
                _ => {}
            }
        }
        if let Some(destination) = route {
            self.set_movement_target(node_id, destination, soldiers, terrain);
        }
        if let Some(formation) = requested_formation {
            if self
                .node(node_id)
                .and_then(|n| n.unit.as_ref())
                .is_some_and(|u| u.formation != formation)
            {
                let _ = self.change_formation(node_id, formation, soldiers, tick);
            }
        }
    }

    /// 部隊の移動目標を設定する。
    ///
    /// `formation_origin` はその場から連続的に動く「現在の隊列アンカー」として残し、
    /// `path_final` に最終目的地、`path` にそこまでのウェイポイントを入れる。
    /// 目的地を直接 `formation_origin` へ代入すると、全兵士のスロットが同じ tick に
    /// 遠方へ飛び、一斉に同じ方向へ歩き出すため。
    fn set_movement_target(
        &mut self,
        node_id: NodeId,
        destination: Vec2Fx,
        soldiers: &Soldiers,
        terrain: &Terrain,
    ) {
        let start = self
            .node(node_id)
            .and_then(|node| node.unit.as_ref())
            .map(|unit| unit.formation_origin)
            .unwrap_or_else(|| self.unit_origin(node_id, soldiers));
        if let Some(unit) = self.node(node_id).and_then(|node| node.unit.as_ref()) {
            let threshold = fx_from_mm(REPATH_DISTANCE_MM);
            if dist(unit.path_final, destination) <= threshold
                && (!unit.path.is_empty()
                    || dist(unit.formation_origin, destination) <= pathing::arrival_radius())
            {
                return;
            }
        }
        let waypoints =
            pathing::find_path(terrain, start, destination).unwrap_or_else(|| vec![destination]);
        let Some(node) = self.node_mut(node_id) else {
            return;
        };
        let Some(unit) = node.unit.as_mut() else {
            return;
        };
        unit.path = waypoints;
        unit.path_final = destination;
    }

    /// 部隊の現在位置の代表点。生存者の重心があればそれを、なければ部隊の
    /// 現在の陣形起点を使う（配置直後などまだ統計が計算されていない場合）。
    fn unit_origin(&self, node_id: NodeId, soldiers: &Soldiers) -> Vec2Fx {
        let Some(node) = self.node(node_id) else {
            return Vec2Fx::ZERO;
        };
        if node.stats.alive > 0 {
            return node.stats.centroid;
        }
        let Some(unit) = &node.unit else {
            return Vec2Fx::ZERO;
        };
        unit.soldiers
            .iter()
            .find_map(|&id| soldiers.pos_checked(id))
            .unwrap_or(unit.formation_origin)
    }

    fn interpret(
        &self,
        node_id: NodeId,
        order: Order,
        soldiers: &Soldiers,
        tick: u32,
    ) -> Compliance {
        if order.expires_tick.is_some_and(|expires| tick > expires) {
            return Compliance::Ignored;
        }
        if order.priority == Priority::Absolute {
            return Compliance::Obeyed;
        }
        let Some(node) = self.node(node_id) else {
            return Compliance::Ignored;
        };
        let Some(i) = soldiers.index_if_present(node.commander) else {
            return Compliance::Ignored;
        };
        let attrs = soldiers.attrs[i];
        // M7: 指揮官の性格（`CommanderAttrs::obedience`/`initiative`）も服従判定に効く
        // （仕様「obedience: 命令遵守。initiative の反対に効く」）。将官には
        // 通常の兵士より重く効かせる（`obedience`/`initiative` の値域は
        // 兵士の `discipline`/`loyalty` と同じ 0..255 なので単純加算できる）。
        let obedience =
            attrs.discipline as u16 + attrs.loyalty as u16 + node.commander_attrs.obedience as u16;
        let conflict = match (node.objective, order.intent) {
            (Some(Intent::Charge { .. }), Intent::Withdraw { .. }) => attrs.aggression as u16,
            (Some(Intent::Hold { .. }), Intent::Charge { .. }) => attrs.self_preservation as u16,
            _ => 0,
        } + node.commander_attrs.initiative as u16 / 2;
        let score = obedience.saturating_sub(conflict / 2);
        let noise = sim_math::Rng::stream(
            0x4D33,
            node.commander,
            sim_math::Purpose::DecisionNoise,
            tick,
        )
        .range(0, 256) as u16;
        if score.saturating_add(noise / 4) >= 300 {
            Compliance::Obeyed
        } else if score.saturating_add(noise / 2) >= 170 {
            Compliance::Partial
        } else {
            Compliance::Ignored
        }
    }

    fn tick_succession(&mut self, soldiers: &Soldiers, tick: u32) {
        for index in 0..self.nodes.len() {
            let commander_dead = {
                let node = &self.nodes[index];
                !soldiers.is_active_id(node.commander)
            };
            if commander_dead && self.nodes[index].command_state == CommandState::Commanded {
                self.nodes[index].command_state = CommandState::Leaderless;
                self.nodes[index].succession_end_tick = None;
                self.events.push(CommandEvent {
                    tick,
                    node: index as NodeId,
                    order: None,
                    kind: CommandEventKind::LeaderLost,
                });
                let deputy = self.nodes[index]
                    .deputies
                    .iter()
                    .copied()
                    .find(|&id| soldiers.is_active_id(id));
                if deputy.is_some() {
                    self.nodes[index].command_state = CommandState::Succeeding;
                    self.nodes[index].succession_end_tick = Some(tick + 20 * sim_math::TICK_HZ);
                    self.events.push(CommandEvent {
                        tick,
                        node: index as NodeId,
                        order: None,
                        kind: CommandEventKind::SuccessionStarted,
                    });
                }
            }
            if self.nodes[index].command_state != CommandState::Succeeding {
                continue;
            }
            let Some(deputy) = self.nodes[index]
                .deputies
                .iter()
                .copied()
                .find(|&id| soldiers.is_active_id(id))
            else {
                self.nodes[index].command_state = CommandState::Leaderless;
                self.nodes[index].succession_end_tick = None;
                continue;
            };
            if self.nodes[index]
                .succession_end_tick
                .is_some_and(|end| tick >= end)
            {
                self.nodes[index].commander = deputy;
                self.nodes[index].command_state = CommandState::Commanded;
                self.nodes[index].succession_end_tick = None;
                self.events.push(CommandEvent {
                    tick,
                    node: index as NodeId,
                    order: None,
                    kind: CommandEventKind::SuccessionCompleted,
                });
            }
        }
    }

    /// 疲れた兵士を、戦っていない仲間と入れ替える（仕様 06 章 6 節
    /// 「指揮官は交代を命じられる」）。
    ///
    /// 陣形スロットは `unit.soldiers` の並び順から決まるので、**配列の要素を
    /// 入れ替えるだけで立ち位置が入れ替わる**。入れ替えた 2 人はそれぞれ相手の
    /// スロットへ歩き出す。
    ///
    /// 「前列」をスロット番号で決めないのは、スロットの幾何（`formation_facing`
    /// と rank の伸びる向き）から見て、どちら側が敵に近いかが部隊の向きと敵の
    /// 位置で変わるため。代わりに**実際に交戦している（`target` を持つ）者**を
    /// 戦列とみなす。誰も交戦していない部隊——行軍中の疲労——では、単に最も
    /// 疲れた者と最も元気な者を入れ替える。
    ///
    /// 交代は訓練の産物なので、入れ替わる 2 人の `discipline` が確率を決める。
    /// 規律の低い部隊では疲れた兵士が戦列に残り続け、そのまま崩れる。
    ///
    /// `world_seed` と `tick` から判定を導くので、実行順序に依存しない。
    pub fn rotate_tired_ranks(&mut self, soldiers: &Soldiers, world_seed: u64, tick: u32) {
        if tick % ROTATION_INTERVAL_TICKS != 0 {
            return;
        }
        for index in 0..self.nodes.len() {
            let Some(unit) = self.nodes[index].unit.as_mut() else {
                continue;
            };
            if unit.ranks < 2 || unit.formation_change.is_some() || unit.soldiers.len() < 2 {
                continue;
            }
            // 借用を跨がないよう、判定は ID を受け取る関数にしておく
            // （`unit.soldiers` は最後に swap で書き換える）
            let fighting = |id: SoldierId| soldiers.target[id as usize] != crate::soldiers::NO_ID;
            let alive = |id: SoldierId| soldiers.is_active_id(id);
            let fatigue = |id: SoldierId| soldiers.fatigue[id as usize];

            let mut anyone_fighting = false;
            // 交代で下がる側の候補（疲れて戦っている者）と、上がる側の候補
            // （元気で戦っていない者）を 1 パスで集める。上がる側は
            // 「最も元気な数人」だけ持てばよいので、固定長の配列に収める
            // ——部隊が 500 人でも走査は 1 回、確保はゼロ。
            let mut fresh: [(u16, usize); MAX_ROTATIONS_PER_INTERVAL] =
                [(u16::MAX, usize::MAX); MAX_ROTATIONS_PER_INTERVAL];
            for k in 0..unit.soldiers.len() {
                let id = unit.soldiers[k];
                if !alive(id) {
                    continue;
                }
                if fighting(id) {
                    anyone_fighting = true;
                    continue;
                }
                let f = fatigue(id);
                // 挿入ソート（要素は MAX_ROTATIONS_PER_INTERVAL 個だけ）
                let mut slot = MAX_ROTATIONS_PER_INTERVAL;
                for (n, entry) in fresh.iter().enumerate() {
                    if f < entry.0 {
                        slot = n;
                        break;
                    }
                }
                if slot < MAX_ROTATIONS_PER_INTERVAL {
                    for n in (slot + 1..MAX_ROTATIONS_PER_INTERVAL).rev() {
                        fresh[n] = fresh[n - 1];
                    }
                    fresh[slot] = (f, k);
                }
            }

            let mut rotated = 0usize;
            for k in 0..unit.soldiers.len() {
                if rotated >= MAX_ROTATIONS_PER_INTERVAL {
                    break;
                }
                let tired_id = unit.soldiers[k];
                if !alive(tired_id) || fatigue(tired_id) < ROTATION_FATIGUE_THRESHOLD {
                    continue;
                }
                // 交戦している部隊では、下がるのは戦列に立っている者だけ
                if anyone_fighting && !fighting(tired_id) {
                    continue;
                }
                let tired_fatigue = fatigue(tired_id);
                let Some(&(_, rested)) = fresh.iter().find(|&&(f, idx)| {
                    idx != usize::MAX && f + ROTATION_FATIGUE_MARGIN <= tired_fatigue
                }) else {
                    continue;
                };
                if rested == k {
                    continue;
                }
                let rested_id = unit.soldiers[rested];
                let discipline = (soldiers.attrs[tired_id as usize].discipline as i32
                    + soldiers.attrs[rested_id as usize].discipline as i32)
                    / 2;
                let chance = (discipline * 3).clamp(0, 900) as u32;
                let mut rng =
                    Rng::stream(world_seed, tired_id, sim_math::Purpose::DecisionNoise, tick);
                if !rng.chance_permille(chance) {
                    continue;
                }
                unit.soldiers.swap(k, rested);
                // 使った受け手は空にする（同じ兵士を二重に上げない）
                for entry in fresh.iter_mut() {
                    if entry.1 == rested || entry.1 == k {
                        *entry = (u16::MAX, usize::MAX);
                    }
                }
                rotated += 1;
                self.rotations = self.rotations.saturating_add(1);
            }
            if rotated > 0 {
                self.events.push(CommandEvent {
                    tick,
                    node: index as NodeId,
                    order: None,
                    kind: CommandEventKind::RanksRotated,
                });
            }
        }
    }

    /// 葉ノードの兵士に、現在の陣形スロットを目標として設定する。
    pub fn formation_goals(&mut self, soldiers: &mut Soldiers, goals: &mut [Vec2Fx], tick: u32) {
        self.march_blend.resize(self.nodes.len(), 0);
        self.role_sorted.resize(self.nodes.len(), false);
        // 兵種によるスロットの並べ替えは、その部隊で 1 回だけ。毎 tick 並べ替えると
        // 損耗のたびに全員の持ち場が動いてしまう。
        for index in 0..self.nodes.len() {
            if self.role_sorted[index] {
                continue;
            }
            self.role_sorted[index] = true;
            if let Some(unit) = self.nodes[index].unit.as_mut() {
                sort_slots_by_role(unit, soldiers);
            }
        }
        // ノードの反復中に書き換えるので、混ざり具合だけ先に取り出しておく。
        let march_blend = &mut self.march_blend;
        for (index, blend) in march_blend.iter_mut().enumerate().take(self.nodes.len()) {
            let mut formation_changed = false;
            let centroid = self.nodes[index].stats.centroid;
            let area_mission = match self.nodes[index].objective {
                Some(Intent::OccupyArea { center, radius_m })
                | Some(Intent::GuardArea {
                    center, radius_m, ..
                }) => Some((center, radius_m)),
                _ => None,
            };
            // 守備任務は、もっとも近い敵部隊の方へ正面を向ける。
            let guard_facing = match self.nodes[index].objective {
                Some(Intent::GuardArea { center, .. }) => {
                    nearest_enemy_bearing(&self.nodes, self.nodes[index].faction, center)
                }
                _ => None,
            };
            {
                let Some(unit) = self.nodes[index].unit.as_mut() else {
                    continue;
                };
                if let Some(change) = unit.formation_change {
                    if tick >= change.complete_tick {
                        unit.formation = change.to;
                        unit.formation_change = None;
                        formation_changed = true;
                    }
                }
                if let Some(facing) = guard_facing {
                    // 守備は敵の来る方向を向く（B5 の「優先方向」）。
                    unit.formation_facing = facing;
                }
                let moving = advance_formation_anchor(unit, centroid, soldiers);
                let transitioning = unit.formation_change.is_some();
                // 歩行間隔と整列間隔は、止まった瞬間に切り替えず滑らかに移す。
                let target = if moving { 1_000u16 } else { 0 };
                *blend = if *blend < target {
                    (*blend + MARCH_BLEND_STEP_PERMILLE).min(target)
                } else {
                    blend.saturating_sub(MARCH_BLEND_STEP_PERMILLE).max(target)
                };
                let blend = *blend;
                if !unit.soldiers.iter().any(|&id| soldiers.is_active_id(id)) {
                    continue;
                }
                let ranks = unit.ranks.max(1) as u32;
                // 死者を除いて詰め直すと、その後ろの全員の目標が同時に変わる。
                // 元のスロット数を維持し、穴は `fill_front_vacancies` が局所的に埋める。
                let slot_count = unit.soldiers.len() as u32;
                let files = slot_count.div_ceil(ranks).max(1);
                let file_spacing = blend_spacing(
                    unit.file_spacing,
                    unit.file_spacing.max(fx_from_mm(MARCH_FILE_SPACING_MIN_MM)),
                    blend,
                );
                let rank_spacing = blend_spacing(
                    unit.rank_spacing,
                    unit.rank_spacing.max(fx_from_mm(MARCH_RANK_SPACING_MIN_MM)),
                    blend,
                );
                let (sin, cos) = (
                    sim_math::sin_fx(unit.formation_facing),
                    sim_math::cos_fx(unit.formation_facing),
                );
                // 区域へ到着した後だけ円盤配置へ展開する。行軍中まで区域の全幅へ
                // 広がると、狭路を通れず、隊列アンカーから大きく遅れてしまう。
                let deployed_area = area_mission.filter(|&(center, _)| {
                    !moving
                        && unit.path.is_empty()
                        && dist(unit.formation_origin, center) <= pathing::arrival_radius()
                });
                for (stable_slot, &id) in unit.soldiers.iter().enumerate() {
                    let i = id as usize;
                    if i >= goals.len() || !soldiers.is_active_id(id) {
                        continue;
                    }
                    let stable_slot = stable_slot as u32;
                    soldiers.slot[i] = stable_slot.min(u16::MAX as u32) as u16;
                    let file = stable_slot % files;
                    let rank = stable_slot / files;
                    let local_x = (file as Fx * file_spacing)
                        - ((files.saturating_sub(1) as Fx * file_spacing) / 2);
                    let local_y = rank as Fx * rank_spacing;
                    let offset = if let Some((_, radius_m)) = deployed_area {
                        area_slot_offset(stable_slot, slot_count, radius_m, unit.formation_facing)
                    } else {
                        Vec2Fx::new(
                            fx_mul(local_x, cos) - fx_mul(local_y, sin),
                            fx_mul(local_x, sin) + fx_mul(local_y, cos),
                        )
                    };
                    goals[i] = unit.formation_origin.add(offset);
                    if transitioning {
                        if soldiers.hot.state[i] == State::Idle {
                            soldiers.hot.state[i] = State::Repositioning;
                        }
                    } else {
                        if soldiers.hot.state[i] == State::Repositioning {
                            soldiers.hot.state[i] = State::Marching;
                        }
                        if soldiers.hot.state[i] == State::Idle
                            && dist(soldiers.pos(i), goals[i]) > fx_from_mm(300)
                        {
                            soldiers.hot.state[i] = State::Marching;
                        }
                    }
                }
                unit.ranks = ranks as u16;
            }
            if formation_changed {
                self.events.push(CommandEvent {
                    tick,
                    node: index as NodeId,
                    order: None,
                    kind: CommandEventKind::FormationChanged,
                });
            }
        }
    }

    /// 死傷者が空けたスロットへ、後ろの兵士を一段だけ前進させる。
    ///
    /// スロット番号が大きいほど進行方向側（前列）なので、穴を埋めるのは
    /// 「1 段小さいスロット」にいる兵士——つまり真後ろの者——になる。前から
    /// 順に見るので、前列の穴が後方へ波のように伝わる。一人の死で部隊全員が
    /// 一斉移動することはない。
    ///
    /// 埋める順は、同じ file の後ろ → 隣接 file の後ろ → 2 段後ろ（B4）。
    /// 前列には近接兵を優先して置き、射撃兵は後ろに残す。踏み出しは規律・疲労・
    /// 混雑で遅れ、間に人がいて通れないときは見送る。
    pub fn fill_front_vacancies(&mut self, soldiers: &Soldiers, hash: &SpatialHash, tick: u32) {
        if tick % VACANCY_FILL_INTERVAL_TICKS != 0 {
            return;
        }
        let round = tick / VACANCY_FILL_INTERVAL_TICKS;
        for node in &mut self.nodes {
            let Some(unit) = node.unit.as_mut() else {
                continue;
            };
            if unit.formation_change.is_some() || unit.soldiers.len() < 2 {
                continue;
            }
            let ranks = unit.ranks.max(1) as usize;
            let files = unit.soldiers.len().div_ceil(ranks).max(1);
            let mut moved_from = vec![false; unit.soldiers.len()];
            let mut filled = 0usize;
            // 前列（大きいスロット）から見る。前の穴ほど先に埋まる。
            for gap in (0..unit.soldiers.len()).rev() {
                if filled >= MAX_VACANCY_FILLS_PER_INTERVAL {
                    break;
                }
                if moved_from[gap] || soldiers.is_active_id(unit.soldiers[gap]) {
                    continue;
                }
                let file = gap % files;
                // 同じ file の 1 段後ろ → 隣接 file の 1 段後ろ → 同じ file の 2 段後ろ。
                let mut candidates = [usize::MAX; 4];
                let mut count = 0;
                let push = |slot: Option<usize>, candidates: &mut [usize; 4], count: &mut usize| {
                    if let Some(slot) = slot {
                        if *count < candidates.len() {
                            candidates[*count] = slot;
                            *count += 1;
                        }
                    }
                };
                push(gap.checked_sub(files), &mut candidates, &mut count);
                if file > 0 {
                    push(gap.checked_sub(files + 1), &mut candidates, &mut count);
                }
                if file + 1 < files {
                    push(
                        gap.checked_sub(files).and_then(|s| s.checked_add(1)),
                        &mut candidates,
                        &mut count,
                    );
                }
                push(gap.checked_sub(files * 2), &mut candidates, &mut count);

                let gap_pos = soldiers
                    .pos_checked(unit.soldiers[gap])
                    .unwrap_or(unit.formation_origin);
                let mut chosen = None;
                for &slot in &candidates[..count] {
                    if slot >= unit.soldiers.len() || moved_from[slot] {
                        continue;
                    }
                    let id = unit.soldiers[slot];
                    let Some(i) = soldiers
                        .index_if_present(id)
                        .filter(|&i| soldiers.is_alive(i))
                    else {
                        continue;
                    };
                    // 組み合っている兵士は列を詰め直す余裕がない。
                    if soldiers.hot.state[i].is_fighting() {
                        continue;
                    }
                    // 前列は近接兵に任せ、射撃兵は後ろに残す。ほかに候補が
                    // 無ければ射撃兵でも埋める。
                    let missile = soldiers.hot.flags[i] & flags::MISSILE_TROOP != 0;
                    if missile && chosen.is_some() {
                        continue;
                    }
                    // 踏み出しの遅れ。規律が高く、疲れておらず、周りが空いて
                    // いるほど早く動く。乱数を使わず、判定の回（round）で散らす。
                    if round % step_out_delay(soldiers, i, hash) != 0 {
                        continue;
                    }
                    // 間に人がいて通れないなら見送る（次の回に別の候補が入る）。
                    if !lane_is_clear(soldiers, hash, soldiers.pos(i), gap_pos, id) {
                        continue;
                    }
                    chosen = Some(slot);
                    if !missile {
                        break;
                    }
                }
                let Some(slot) = chosen else {
                    continue;
                };
                unit.soldiers.swap(gap, slot);
                moved_from[slot] = true;
                filled += 1;
            }
        }
    }

    pub fn change_formation(
        &mut self,
        node_id: NodeId,
        to: FormationId,
        soldiers: &Soldiers,
        tick: u32,
    ) -> bool {
        let seconds;
        {
            let Some(node) = self.node_mut(node_id) else {
                return false;
            };
            let Some(unit) = node.unit.as_mut() else {
                return false;
            };
            if unit.formation == to || unit.formation_change.is_some() {
                return false;
            }
            let alive = unit
                .soldiers
                .iter()
                .filter(|&&id| soldiers.is_active_id(id))
                .count() as u32;
            let discipline = unit
                .soldiers
                .iter()
                .filter_map(|&id| soldiers.index_if_present(id))
                .map(|i| soldiers.attrs[i].discipline as u32)
                .sum::<u32>()
                / alive.max(1);
            let base = formation_def(to).change_time_base_s as u32;
            seconds = (base * alive.max(1).div_ceil(100) * (2000u32.saturating_sub(discipline))
                / 1000)
                .max(1);
            let from = unit.formation;
            unit.formation_change = Some(FormationChange {
                from,
                to,
                started_tick: tick,
                complete_tick: tick + seconds * sim_math::TICK_HZ,
            });
        }
        self.events.push(CommandEvent {
            tick,
            node: node_id,
            order: None,
            kind: CommandEventKind::FormationChangeStarted,
        });
        true
    }

    fn update_stats(&mut self, soldiers: &Soldiers) {
        for node in &mut self.nodes {
            let ids = if let Some(unit) = &node.unit {
                unit.soldiers.clone()
            } else {
                Vec::new()
            };
            if ids.is_empty() {
                continue;
            }
            node.stats.downed = 0;
            node.stats.dead = 0;
            node.stats.broken = 0;
            let mut sum = Vec2Fx::ZERO;
            let mut alive = 0u32;
            let mut morale = 0u32;
            let mut fatigue = 0u32;
            for id in ids {
                let Some(i) = soldiers.index_if_present(id) else {
                    continue;
                };
                let state = soldiers.hot.state[i];
                if state.is_active() {
                    sum = sum.add(soldiers.pos(i));
                    alive += 1;
                }
                if state == State::Downed {
                    node.stats.downed += 1;
                }
                if state == State::Dead {
                    node.stats.dead += 1;
                }
                if state == State::Broken {
                    node.stats.broken += 1;
                }
                morale += soldiers.morale[i] as u32;
                fatigue += soldiers.fatigue[i] as u32;
            }
            node.stats.alive = alive;
            if alive > 0 {
                node.stats.centroid = Vec2Fx::new(sum.x / alive as Fx, sum.y / alive as Fx);
            }
            let count = node.unit.as_ref().map_or(0, |u| u.soldiers.len()) as u32;
            node.stats.avg_morale = (morale / count.max(1)) as u16;
            node.stats.avg_fatigue = (fatigue / count.max(1)) as u16;
        }
    }

    pub fn state_hash(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut mix = |v: u64| {
            h ^= v;
            h = h.wrapping_mul(0x0100_0000_01b3);
        };
        for &blend in &self.march_blend {
            mix(blend as u64);
        }
        for node in &self.nodes {
            mix(node.id as u64);
            mix(node.commander as u64);
            mix(node.command_state as u64);
            mix(node.received_order.map_or(u64::MAX, |o| o.id as u64));
            mix(node.objective.map_or(u64::MAX, intent_hash));
            if let Some(unit) = &node.unit {
                mix(unit.formation as u64);
                mix(unit.formation_origin.x as u32 as u64);
                mix(unit.formation_origin.y as u32 as u64);
                mix(unit.path.len() as u64);
                mix(unit.path_final.x as u32 as u64);
                mix(unit.path_final.y as u32 as u64);
                for waypoint in &unit.path {
                    mix(waypoint.x as u32 as u64);
                    mix(waypoint.y as u32 as u64);
                }
                // スロットの並び（前後列の交代で変わる）も決定論の検証対象
                for &id in &unit.soldiers {
                    mix(id as u64);
                }
            }
            // M7: 指揮官 AI の状態も決定論検証の対象にする。
            mix(node.blackboard.enemy_forces.len() as u64);
            for f in &node.blackboard.enemy_forces {
                mix(f.node as u64);
                mix(f.confidence as u64);
                mix(f.est_strength as u64);
            }
            mix(node.strategic_objective.map_or(u64::MAX, objective_hash));
            mix(node.battle_plan.map_or(u64::MAX, |p| p as u64));
        }
        for messenger in &self.messengers {
            mix(messenger.order.id as u64);
            mix(messenger.state as u64);
            mix(messenger.remaining_ticks as u64);
        }
        h
    }
}

/// Unit の現在アンカーを経路上で 1 tick 分だけ進める。
///
/// 兵士の重心が遅れている場合はアンカーを減速し、目標スロットだけが部隊から
/// 遠くへ逃げ続けるのを防ぐ。返り値は、この tick の開始時点で移動中だったか。
fn advance_formation_anchor(unit: &mut Unit, centroid: Vec2Fx, soldiers: &Soldiers) -> bool {
    if unit.path.is_empty() {
        return false;
    }
    let moving = true;
    let formation = formation_def(unit.formation);
    let mut anchor_speed = FORMATION_ANCHOR_SPEED_MM_PER_S;
    for &id in &unit.soldiers {
        let Some(i) = soldiers.index_if_present(id) else {
            continue;
        };
        match soldiers.hot.state[i] {
            State::Charging if soldiers.hot.flags[i] & crate::soldiers::flags::MOUNTED != 0 => {
                anchor_speed = anchor_speed.max(MOUNTED_CHARGE_ANCHOR_SPEED_MM_PER_S);
            }
            State::Charging => {
                anchor_speed = anchor_speed.max(FOOT_CHARGE_ANCHOR_SPEED_MM_PER_S);
            }
            State::Advancing => {
                anchor_speed = anchor_speed.max(ADVANCE_ANCHOR_SPEED_MM_PER_S);
            }
            _ => {}
        }
    }
    let base_step = per_sec_to_per_tick(fx_from_mm(anchor_speed));
    let mut remaining = sim_math::fx_scale_permille(base_step, formation.move_mult as i32);
    let lag = dist(centroid, unit.formation_origin);
    if lag >= fx_from_mm(ANCHOR_STOP_LAG_MM) {
        remaining = 0;
    } else if lag >= fx_from_mm(ANCHOR_SLOW_LAG_MM) {
        remaining /= 2;
    }

    while remaining > 0 && !unit.path.is_empty() {
        let waypoint = unit.path[0];
        let delta = waypoint.sub(unit.formation_origin);
        let distance = delta.len();
        if distance <= remaining {
            unit.formation_origin = waypoint;
            unit.path.remove(0);
            remaining -= distance;
        } else {
            unit.formation_origin = unit
                .formation_origin
                .add(delta.normalized().scale(remaining));
            remaining = 0;
        }
    }
    moving
}

/// 安定スロットを、決定論的な螺旋で円形区域へ割り当てる。
///
/// 半径は `sqrt(slot / (count - 1))` で増やすため、中心付近に過密にならず
/// 円盤全体をほぼ均等に使う。角度は黄金角に近い値でずらし、全員が同じ列・
/// 同じ方向へ動き出す見た目も避ける。
fn area_slot_offset(stable_slot: u32, slot_count: u32, radius_m: u16, facing: Brad) -> Vec2Fx {
    if stable_slot == 0 || slot_count <= 1 || radius_m == 0 {
        return Vec2Fx::ZERO;
    }
    // 区域に無理なく入れる人数。これを超えた分は中へ押し込まず、周縁で警戒に
    // 回す（B5）。半径 10 m なら 314 m² ÷ 4 m² ≒ 78 人。
    let capacity = ((radius_m as u32).pow(2) * 314 / 100 / AREA_SQUARE_METRES_PER_SOLDIER).max(1);
    let inside_count = slot_count.min(capacity);
    let angle = facing.wrapping_add((stable_slot as u16).wrapping_mul(AREA_SPIRAL_STEP_BRAD));
    let radius = if stable_slot < inside_count {
        // 中は等面積になるよう、番号の平方根で外へ広げる。
        let ratio_million = (stable_slot as u64).saturating_mul(1_000_000)
            / (inside_count.saturating_sub(1).max(1) as u64);
        let radial_permille = sim_math::isqrt64(ratio_million.min(1_000_000)) as i32;
        sim_math::fx_scale_permille(fx(radius_m as i32), radial_permille)
    } else {
        // 周縁の輪。区域の少し外に立って外を見張る。
        sim_math::fx_scale_permille(fx(radius_m as i32), AREA_PERIMETER_PERMILLE as i32)
    };
    Vec2Fx::new(
        fx_mul(radius, sim_math::sin_fx(angle)),
        fx_mul(radius, sim_math::cos_fx(angle)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soldiers::Attrs;

    fn flat_terrain(size_m: u32) -> Terrain {
        sim_terrain::Terrain::flat(size_m / 4, 4, 1)
    }

    fn soldiers_at(positions: &[(i32, i32)]) -> Soldiers {
        let mut soldiers = Soldiers::default();
        for &(x, y) in positions {
            soldiers.push(
                fx(x),
                fx(y),
                0,
                0,
                0,
                Attrs::new(120, 120, 120, 120, 120, 120, 120, 220, 80, 120, 220, 180),
                0,
            );
        }
        soldiers
    }

    #[test]
    fn order_delay_grows_with_distance_and_reaches_leaf() {
        let mut soldiers = soldiers_at(&[(10, 10), (20, 10), (200, 10)]);
        let mut tree = CommandTree::new();
        let army = tree.add_node(None, 0, 0, 0, vec![1], None);
        let unit = Unit {
            soldiers: vec![2],
            troop_type: 0,
            formation: FORMATION_LINE,
            formation_origin: Vec2Fx::new(fx(200), fx(10)),
            formation_facing: 0,
            ranks: 1,
            file_spacing: fx_from_mm(800),
            rank_spacing: fx_from_mm(800),
            banner: None,
            formation_change: None,
            path: Vec::new(),
            path_final: Vec2Fx::ZERO,
            pursuit_leash: None,
        };
        let terrain = flat_terrain(400);
        let leaf = tree.add_node(Some(army), 1, 0, 2, vec![], Some(unit));
        tree.issue_order(
            army,
            leaf,
            Intent::Hold {
                pos: Vec2Fx::new(fx(200), fx(10)),
                facing: 0,
                allow_pursuit: false,
            },
            Priority::Routine,
            0,
            &soldiers,
        );
        let first = tree.messengers[0].remaining_ticks;
        assert!(first > 0);
        for tick in 0..first {
            tree.tick(&mut soldiers, &terrain, tick);
        }
        assert_eq!(tree.node(leaf).unwrap().received_order.unwrap().id, 0);
        assert!(tree
            .events
            .iter()
            .any(|e| e.kind == CommandEventKind::Obeyed));
    }

    #[test]
    fn dead_messenger_prevents_delivery() {
        let mut soldiers = soldiers_at(&[(10, 10), (20, 10)]);
        let mut tree = CommandTree::new();
        let root = tree.add_node(None, 0, 0, 0, vec![], None);
        let leaf = tree.add_node(
            Some(root),
            1,
            0,
            1,
            vec![],
            Some(Unit {
                soldiers: vec![1],
                troop_type: 0,
                formation: FORMATION_LINE,
                formation_origin: Vec2Fx::new(fx(20), fx(10)),
                formation_facing: 0,
                ranks: 1,
                file_spacing: fx_from_mm(800),
                rank_spacing: fx_from_mm(800),
                banner: None,
                formation_change: None,
                path: Vec::new(),
                path_final: Vec2Fx::ZERO,
                pursuit_leash: None,
            }),
        );
        let terrain = flat_terrain(400);
        tree.issue_order(
            root,
            leaf,
            Intent::Reserve {
                rally_pos: Vec2Fx::new(fx(20), fx(10)),
            },
            Priority::Routine,
            0,
            &soldiers,
        );
        soldiers.hot.state[0] = State::Dead;
        tree.tick(&mut soldiers, &terrain, 1);
        assert!(tree.node(leaf).unwrap().received_order.is_none());
        assert!(tree
            .events
            .iter()
            .any(|e| e.kind == CommandEventKind::MessengerLost));
    }

    #[test]
    fn deputy_succeeds_after_commander_loss() {
        let mut soldiers = soldiers_at(&[(10, 10), (11, 10)]);
        let mut tree = CommandTree::new();
        let root = tree.add_node(None, 0, 0, 0, vec![1], None);
        let terrain = flat_terrain(400);
        soldiers.hot.state[0] = State::Dead;
        tree.tick(&mut soldiers, &terrain, 0);
        assert_eq!(
            tree.node(root).unwrap().command_state,
            CommandState::Succeeding
        );
        for tick in 1..=20 * sim_math::TICK_HZ {
            tree.tick(&mut soldiers, &terrain, tick);
        }
        assert_eq!(tree.node(root).unwrap().commander, 1);
        assert_eq!(
            tree.node(root).unwrap().command_state,
            CommandState::Commanded
        );
    }

    #[test]
    fn move_to_routes_around_an_impassable_wall() {
        // 目的地までの直線上に通行不能な壁を置き、経路探索がそれを迂回することを確認する。
        let mut terrain = flat_terrain(400);
        // 粗いセル（16 m）を丸ごと塞ぐ厚みの壁を、隙間を 1 か所だけ残して置く
        let gap_start = terrain.dim.saturating_sub(12);
        for cy in 0..terrain.dim {
            if cy >= gap_start {
                continue;
            }
            for wx in (terrain.dim / 2)..(terrain.dim / 2 + 8) {
                let idx = terrain.idx(wx, cy);
                terrain.passability[idx] = 0;
            }
        }
        let soldiers = soldiers_at(&[(50, 200)]);
        let mut tree = CommandTree::new();
        let root = tree.add_node(None, 0, 0, 0, vec![], None);
        let unit = Unit {
            soldiers: vec![0],
            troop_type: 0,
            formation: FORMATION_LINE,
            formation_origin: Vec2Fx::new(fx(50), fx(200)),
            formation_facing: 0,
            ranks: 1,
            file_spacing: fx_from_mm(800),
            rank_spacing: fx_from_mm(800),
            banner: None,
            formation_change: None,
            path: Vec::new(),
            path_final: Vec2Fx::ZERO,
            pursuit_leash: None,
        };
        let leaf = tree.add_node(Some(root), 1, 0, 0, vec![], Some(unit));
        tree.set_movement_target(leaf, Vec2Fx::new(fx(350), fx(200)), &soldiers, &terrain);
        let unit = tree.node(leaf).unwrap().unit.as_ref().unwrap();
        // 直進なら壁の範囲を通るはずだが、経路上のどのウェイポイントも
        // 壁のセルには入らない
        let wall_range = (terrain.dim / 2)..(terrain.dim / 2 + 8);
        let mut all_points = vec![unit.formation_origin];
        all_points.extend(unit.path.iter().copied());
        assert!(
            all_points.len() > 1,
            "壁があるのに経路が直線 1 本になっている"
        );
        for p in &all_points[..all_points.len() - 1] {
            let (cx, _) = terrain.world_to_cell(p.x, p.y);
            assert!(
                !wall_range.contains(&cx),
                "ウェイポイントが壁の上に乗っている"
            );
        }
    }

    #[test]
    fn formation_slots_stay_stable_after_death() {
        let mut soldiers = soldiers_at(&[(100, 100), (100, 101), (100, 102), (100, 103)]);
        let mut tree = CommandTree::new();
        let root = tree.add_node(None, 0, 0, 0, vec![], None);
        let unit = Unit {
            soldiers: vec![0, 1, 2, 3],
            troop_type: 0,
            formation: FORMATION_LINE,
            formation_origin: Vec2Fx::new(fx(100), fx(100)),
            formation_facing: 0,
            ranks: 2,
            file_spacing: fx_from_mm(800),
            rank_spacing: fx_from_mm(800),
            banner: None,
            formation_change: None,
            path: Vec::new(),
            path_final: Vec2Fx::ZERO,
            pursuit_leash: None,
        };
        tree.add_node(Some(root), 1, 0, 0, vec![], Some(unit));
        let mut goals = vec![Vec2Fx::ZERO; 4];
        tree.formation_goals(&mut soldiers, &mut goals, 0);
        let before = goals[3];
        soldiers.hot.state[1] = State::Dead;
        tree.formation_goals(&mut soldiers, &mut goals, 1);
        assert_eq!(before, goals[3]);
        assert_eq!(soldiers.slot[3], 3);
    }

    /// 前列（スロット番号が大きい側）の穴を、真後ろの兵士が一段ずつ埋める。
    /// 穴は後方へ波のように伝わる（B4）。
    #[test]
    fn front_vacancy_is_filled_from_behind_one_rank_per_interval() {
        // 3 列 × 2 file。スロット 0,1 が最後尾、4,5 が前列。
        let mut soldiers = soldiers_at(&[
            (100, 100),
            (101, 100),
            (100, 101),
            (101, 101),
            (100, 102),
            (101, 102),
        ]);
        // 規律が高いほど踏み出しが早い。ここでは遅れを見ないので最大にする。
        for i in 0..soldiers.len() {
            soldiers.attrs[i].discipline = 250;
        }
        let mut hash = SpatialHash::default();
        hash.rebuild(&soldiers);
        let mut tree = CommandTree::new();
        let root = tree.add_node(None, 0, 0, 0, vec![], None);
        let mut unit = leaf_unit((0..6).collect(), Vec2Fx::new(fx(100), fx(100)));
        unit.ranks = 3;
        let leaf = tree.add_node(Some(root), 1, 0, 0, vec![], Some(unit));
        // 前列（スロット 4）の兵士が倒れる。
        soldiers.hot.state[4] = State::Dead;
        hash.rebuild(&soldiers);

        tree.fill_front_vacancies(&soldiers, &hash, VACANCY_FILL_INTERVAL_TICKS);
        let order = &tree.node(leaf).unwrap().unit.as_ref().unwrap().soldiers;
        assert_eq!(order, &[0, 1, 4, 3, 2, 5], "真後ろの兵が前列へ出ていない");

        // 前へ出た兵士は実際に歩いて空いた場所へ移る。次の穴（元いた場所）は
        // これで通れるようになる。
        soldiers.set_pos(2, Vec2Fx::new(fx(100), fx(102)));
        hash.rebuild(&soldiers);
        tree.fill_front_vacancies(&soldiers, &hash, VACANCY_FILL_INTERVAL_TICKS * 2);
        let order = &tree.node(leaf).unwrap().unit.as_ref().unwrap().soldiers;
        assert_eq!(order, &[4, 1, 0, 3, 2, 5], "穴が後方へ伝わっていない");
    }

    /// 射撃兵は前列の穴埋めに使わない。ほかに候補がいれば近接兵が前へ出る（B4）。
    #[test]
    fn missile_troops_stay_out_of_the_front_rank() {
        let mut soldiers = soldiers_at(&[
            (100, 100),
            (101, 100),
            (100, 101),
            (101, 101),
            (100, 102),
            (101, 102),
        ]);
        for i in 0..soldiers.len() {
            soldiers.attrs[i].discipline = 250;
        }
        // 同じ file の真後ろ（スロット 2）は射撃兵。隣接 file の後ろ（スロット 3）
        // は近接兵。
        soldiers.hot.flags[2] |= flags::MISSILE_TROOP;
        let mut hash = SpatialHash::default();
        hash.rebuild(&soldiers);
        let mut tree = CommandTree::new();
        let root = tree.add_node(None, 0, 0, 0, vec![], None);
        let mut unit = leaf_unit((0..6).collect(), Vec2Fx::new(fx(100), fx(100)));
        unit.ranks = 3;
        let leaf = tree.add_node(Some(root), 1, 0, 0, vec![], Some(unit));
        soldiers.hot.state[4] = State::Dead;
        hash.rebuild(&soldiers);

        tree.fill_front_vacancies(&soldiers, &hash, VACANCY_FILL_INTERVAL_TICKS);
        let order = &tree.node(leaf).unwrap().unit.as_ref().unwrap().soldiers;
        assert_eq!(order[4], 3, "射撃兵が前列へ出ている: {order:?}");
    }

    #[test]
    fn movement_target_advances_anchor_gradually() {
        let mut soldiers = soldiers_at(&[(50, 200)]);
        let terrain = flat_terrain(400);
        let mut tree = CommandTree::new();
        let root = tree.add_node(None, 0, 0, 0, vec![], None);
        let origin = Vec2Fx::new(fx(50), fx(200));
        let leaf = tree.add_node(
            Some(root),
            1,
            0,
            0,
            vec![],
            Some(leaf_unit(vec![0], origin)),
        );
        let destination = Vec2Fx::new(fx(150), fx(200));
        tree.set_movement_target(leaf, destination, &soldiers, &terrain);
        tree.node_mut(leaf).unwrap().stats.centroid = origin;

        let mut goals = vec![Vec2Fx::ZERO; 1];
        tree.formation_goals(&mut soldiers, &mut goals, 0);
        let unit = tree.node(leaf).unwrap().unit.as_ref().unwrap();
        assert!(unit.formation_origin.x > origin.x);
        assert!(unit.formation_origin.x < destination.x);
        assert_eq!(unit.path_final, destination);
    }

    #[test]
    fn marching_spacing_is_wider_than_formed_spacing() {
        let mut soldiers = soldiers_at(&[(100, 100), (101, 100), (100, 101), (101, 101)]);
        let mut tree = CommandTree::new();
        let root = tree.add_node(None, 0, 0, 0, vec![], None);
        let origin = Vec2Fx::new(fx(100), fx(100));
        let mut unit = leaf_unit(vec![0, 1, 2, 3], origin);
        unit.ranks = 2;
        unit.file_spacing = fx_from_mm(800);
        unit.rank_spacing = fx_from_mm(800);
        unit.path = vec![Vec2Fx::new(fx(120), fx(100))];
        unit.path_final = unit.path[0];
        let leaf = tree.add_node(Some(root), 1, 0, 0, vec![], Some(unit));
        tree.node_mut(leaf).unwrap().stats.centroid = origin;
        let mut goals = vec![Vec2Fx::ZERO; 4];

        // 歩き出してしばらくすると、歩行の間隔まで開く（滑らかに移る、B4）。
        for tick in 0..40 {
            tree.node_mut(leaf).unwrap().stats.centroid = tree
                .node(leaf)
                .unwrap()
                .unit
                .as_ref()
                .unwrap()
                .formation_origin;
            tree.formation_goals(&mut soldiers, &mut goals, tick);
        }
        assert_eq!(dist(goals[0], goals[1]), fx_from_mm(900));
        assert_eq!(dist(goals[0], goals[2]), fx_from_mm(1_100));

        // 止まった直後は跳ねず、少しずつ元の密な間隔へ戻る。
        let unit = tree.node_mut(leaf).unwrap().unit.as_mut().unwrap();
        unit.path.clear();
        tree.formation_goals(&mut soldiers, &mut goals, 40);
        let just_after_stopping = dist(goals[0], goals[1]);
        assert!(
            just_after_stopping < fx_from_mm(900) && just_after_stopping > fx_from_mm(800),
            "止まった瞬間に間隔が跳んでいる: {} mm",
            sim_math::fx_to_mm(just_after_stopping)
        );

        for tick in 41..80 {
            tree.formation_goals(&mut soldiers, &mut goals, tick);
        }
        assert_eq!(dist(goals[0], goals[1]), fx_from_mm(800));
        assert_eq!(dist(goals[0], goals[2]), fx_from_mm(800));
    }

    /// 地点占拠は「移動中 → 展開中 → 確保済み」と段階が進む（B5）。
    #[test]
    fn occupying_an_area_moves_through_its_mission_states() {
        let center = Vec2Fx::new(fx(100), fx(100));
        let mut soldiers = soldiers_at(&[(100, 100); 9]);
        let terrain = flat_terrain(400);
        let mut tree = CommandTree::new();
        let node = tree.add_node(
            None,
            0,
            0,
            0,
            vec![],
            Some(leaf_unit((0..9).collect(), center)),
        );
        tree.override_intent(
            node,
            Intent::OccupyArea {
                center,
                radius_m: 10,
            },
            &mut soldiers,
            &terrain,
            0,
        );
        // 全員が中心にいる（＝範囲内）。到着済みなので展開中から始まる。
        for tick in 0..MISSION_REVIEW_TICKS + 1 {
            tree.tick(&mut soldiers, &terrain, tick);
        }
        assert_eq!(
            tree.node(node).unwrap().mission.state,
            MissionState::Deploying,
            "着いた直後に確保済みになっている"
        );
        assert_eq!(tree.node(node).unwrap().mission.inside_friendly, 9);

        // 十分な時間そこに留まれば確保済みへ。
        for tick in MISSION_REVIEW_TICKS + 1..OCCUPY_HOLD_TICKS * 2 {
            tree.tick(&mut soldiers, &terrain, tick);
        }
        assert_eq!(
            tree.node(node).unwrap().mission.state,
            MissionState::Secured,
            "留まり続けても確保済みにならない"
        );

        // 敵が入り込んだら交戦中へ戻る。
        let intruder = soldiers.push(fx(101), fx(100), 0, 0, 1, Attrs::default(), 0);
        let _ = intruder;
        for tick in OCCUPY_HOLD_TICKS * 2..OCCUPY_HOLD_TICKS * 2 + MISSION_REVIEW_TICKS + 1 {
            tree.tick(&mut soldiers, &terrain, tick);
        }
        assert_eq!(
            tree.node(node).unwrap().mission.state,
            MissionState::Engaged
        );
        assert_eq!(tree.node(node).unwrap().mission.inside_enemy, 1);
    }

    /// 区域に入りきらない人数は、周縁の輪へ回る（B5）。
    #[test]
    fn surplus_soldiers_watch_the_perimeter() {
        // 半径 3 m の区域（面積 28 m² ≒ 7 人ぶん）に 20 人。
        let radius_m = 3u16;
        let inside = area_slot_offset(3, 20, radius_m, 0);
        let surplus = area_slot_offset(19, 20, radius_m, 0);
        assert!(
            inside.len() <= fx(radius_m as i32),
            "区域の中の兵士が外へ出ている"
        );
        assert!(
            surplus.len() > fx(radius_m as i32),
            "余った兵士が周縁へ回っていない"
        );
    }

    #[test]
    fn occupy_area_spreads_stable_slots_across_the_zone() {
        let center = Vec2Fx::new(fx(100), fx(100));
        let mut soldiers = soldiers_at(&[(100, 100); 9]);
        let terrain = flat_terrain(400);
        let mut tree = CommandTree::new();
        let node = tree.add_node(
            None,
            0,
            0,
            0,
            vec![],
            Some(leaf_unit((0..9).collect(), center)),
        );
        tree.override_intent(
            node,
            Intent::OccupyArea {
                center,
                radius_m: 20,
            },
            &mut soldiers,
            &terrain,
            0,
        );
        let mut goals = vec![Vec2Fx::ZERO; soldiers.len()];
        tree.formation_goals(&mut soldiers, &mut goals, 0);

        assert_eq!(goals[0], center);
        assert_ne!(goals[1], goals[2]);
        assert!(goals
            .iter()
            .skip(1)
            .all(|&goal| dist(center, goal) <= fx(20)));
        assert!(goals.iter().skip(1).any(|&goal| goal.x < center.x));
        assert!(goals.iter().skip(1).any(|&goal| goal.x > center.x));
    }

    /// 追跡は「見えている間だけ」現在位置を追い、見失えば最後に見た場所へ向かう
    /// （B1: 全知をやめる）。
    #[test]
    fn hunt_person_follows_only_what_the_unit_can_see() {
        let mut soldiers = soldiers_at(&[(50, 100), (60, 100)]);
        soldiers.faction[1] = 1;
        let terrain = flat_terrain(400);
        let mut tree = CommandTree::new();
        let node = tree.add_node(
            None,
            0,
            0,
            0,
            vec![],
            Some(leaf_unit(vec![0], soldiers.pos(0))),
        );
        tree.override_intent(
            node,
            Intent::HuntPerson { target: 1 },
            &mut soldiers,
            &terrain,
            0,
        );

        // 視界（40 m）の中で動く間は追える。
        soldiers.set_pos(1, Vec2Fx::new(fx(75), fx(100)));
        tree.tick(&mut soldiers, &terrain, 4);
        assert_eq!(
            tree.node(node).unwrap().unit.as_ref().unwrap().path_final,
            Vec2Fx::new(fx(75), fx(100))
        );
        assert_eq!(
            tree.node(node).unwrap().hunt.unwrap().state,
            HuntState::Tracking
        );

        // 視界の外へ逃げられたら、最後に見た場所のまま。現在位置は知らない。
        soldiers.set_pos(1, Vec2Fx::new(fx(200), fx(100)));
        tree.tick(&mut soldiers, &terrain, 8);
        let hunted = tree.node(node).unwrap();
        assert_eq!(
            hunted.unit.as_ref().unwrap().path_final,
            Vec2Fx::new(fx(75), fx(100)),
            "見えていない対象の現在位置を知っている"
        );
        assert_eq!(hunted.hunt.unwrap().state, HuntState::Searching);
        assert_eq!(hunted.hunt.unwrap().last_seen, Vec2Fx::new(fx(75), fx(100)));
    }

    /// 対象が倒れたら任務は達成。部隊は止まったままにならず、直前の任務へ戻る。
    #[test]
    fn hunt_person_returns_to_the_previous_mission_when_the_target_falls() {
        let mut soldiers = soldiers_at(&[(50, 100), (60, 100)]);
        soldiers.faction[1] = 1;
        let terrain = flat_terrain(400);
        let mut tree = CommandTree::new();
        let node = tree.add_node(
            None,
            0,
            0,
            0,
            vec![],
            Some(leaf_unit(vec![0], soldiers.pos(0))),
        );
        let guard = Intent::GuardArea {
            center: Vec2Fx::new(fx(50), fx(100)),
            radius_m: 10,
            intercept_radius_m: 20,
        };
        tree.override_intent(node, guard, &mut soldiers, &terrain, 0);
        tree.override_intent(
            node,
            Intent::HuntPerson { target: 1 },
            &mut soldiers,
            &terrain,
            0,
        );

        soldiers.hot.state[1] = State::Downed;
        tree.tick(&mut soldiers, &terrain, 4);
        let finished = tree.node(node).unwrap();
        assert_eq!(finished.objective, Some(guard), "元の任務へ戻っていない");
        assert_eq!(finished.hunt.unwrap().state, HuntState::TargetDown);
        assert!(finished.unit.as_ref().unwrap().path.is_empty());
    }

    /// 長く見失えば追跡を打ち切る。元の任務が無ければその場の確保へ移り、
    /// 目的の無いまま立ち尽くさない。
    #[test]
    fn hunt_person_gives_up_after_searching_the_last_known_position() {
        let mut soldiers = soldiers_at(&[(50, 100), (60, 100)]);
        soldiers.faction[1] = 1;
        let terrain = flat_terrain(400);
        let mut tree = CommandTree::new();
        let node = tree.add_node(
            None,
            0,
            0,
            0,
            vec![],
            Some(leaf_unit(vec![0], soldiers.pos(0))),
        );
        tree.override_intent(
            node,
            Intent::HuntPerson { target: 1 },
            &mut soldiers,
            &terrain,
            0,
        );
        soldiers.set_pos(1, Vec2Fx::new(fx(390), fx(390)));

        let mut tick = 0;
        while tick < HUNT_GIVE_UP_TICKS + 8 {
            tick += 4;
            tree.tick(&mut soldiers, &terrain, tick);
            if !matches!(
                tree.node(node).unwrap().objective,
                Some(Intent::HuntPerson { .. })
            ) {
                break;
            }
        }
        let given_up = tree.node(node).unwrap();
        assert!(
            matches!(given_up.objective, Some(Intent::Hold { .. })),
            "見失ったまま追跡を続けている: {:?}",
            given_up.objective
        );
        assert_eq!(given_up.hunt.unwrap().state, HuntState::Lost);
    }

    fn leaf_unit(soldiers: Vec<SoldierId>, origin: Vec2Fx) -> Unit {
        Unit {
            soldiers,
            troop_type: 0,
            formation: FORMATION_LINE,
            formation_origin: origin,
            formation_facing: 0,
            ranks: 1,
            file_spacing: fx_from_mm(800),
            rank_spacing: fx_from_mm(800),
            banner: None,
            formation_change: None,
            path: Vec::new(),
            path_final: origin,
            pursuit_leash: None,
        }
    }

    /// 仕様 12 章 M5 の受け入れ条件のうち「突撃が会戦の転換点になる」動線：
    /// Charge 命令が届くと、騎乗している者は Charging へ、徒歩の者は
    /// Advancing へ切り替わる。
    #[test]
    fn charge_order_sets_mounted_soldiers_charging() {
        let mut soldiers = soldiers_at(&[(10, 10), (11, 10), (300, 10)]);
        soldiers.hot.flags[1] |= crate::soldiers::flags::MOUNTED;
        let terrain = flat_terrain(400);
        let mut tree = CommandTree::new();
        let army = tree.add_node(None, 0, 0, 0, vec![], None);
        let friendly = tree.add_node(
            Some(army),
            1,
            0,
            0,
            vec![],
            Some(leaf_unit(vec![0, 1], Vec2Fx::new(fx(10), fx(10)))),
        );
        let enemy = tree.add_node(
            Some(army),
            1,
            1,
            2,
            vec![],
            Some(leaf_unit(vec![2], Vec2Fx::new(fx(300), fx(10)))),
        );
        tree.issue_order(
            army,
            friendly,
            Intent::Charge { target: enemy },
            Priority::Absolute,
            0,
            &soldiers,
        );
        let total = tree.messengers[0].total_ticks;
        for tick in 0..=total {
            tree.tick(&mut soldiers, &terrain, tick);
        }
        assert_eq!(
            soldiers.hot.state[1],
            State::Charging,
            "騎乗兵が Charging になっていない"
        );
        assert_eq!(
            soldiers.hot.state[0],
            State::Advancing,
            "徒歩兵が Advancing になっていない"
        );
    }

    /// 仕様 12 章 M5「追撃に出た騎兵が戻ってくるのに現実的な時間がかかる」。
    /// 紐の上限を超えると、部隊は起点へ呼び戻される（＝経路探索付きの
    /// 通常移動で戻る＝瞬間移動ではなく実時間がかかる）。
    #[test]
    fn pursuit_beyond_leash_recalls_the_unit_to_its_origin() {
        let mut soldiers = soldiers_at(&[(10, 10), (500, 500)]);
        let terrain = flat_terrain(1200);
        let mut tree = CommandTree::new();
        let army = tree.add_node(None, 0, 0, 0, vec![], None);
        let origin = Vec2Fx::new(fx(10), fx(10));
        let chaser = tree.add_node(
            Some(army),
            1,
            0,
            0,
            vec![],
            Some(leaf_unit(vec![0], origin)),
        );
        let quarry = tree.add_node(
            Some(army),
            1,
            1,
            1,
            vec![],
            Some(leaf_unit(vec![1], Vec2Fx::new(fx(500), fx(500)))),
        );
        tree.issue_order(
            army,
            chaser,
            Intent::Pursue {
                target: quarry,
                max_distance_m: 50,
            },
            Priority::Absolute,
            0,
            &soldiers,
        );
        let total = tree.messengers[0].total_ticks;
        for tick in 0..=total {
            tree.tick(&mut soldiers, &terrain, tick);
        }
        assert!(
            tree.node(chaser)
                .unwrap()
                .unit
                .as_ref()
                .unwrap()
                .pursuit_leash
                .is_some(),
            "追撃紐が設定されていない"
        );

        // 紐の上限を大きく超える位置まで追いついたことにする。
        soldiers.hot.pos_x[0] = fx(500);
        soldiers.hot.pos_y[0] = fx(500);
        tree.tick(&mut soldiers, &terrain, total + 1);

        let unit = tree.node(chaser).unwrap().unit.as_ref().unwrap();
        assert!(
            unit.pursuit_leash.is_none(),
            "紐の上限を超えても呼び戻されていない"
        );
        // 瞬間移動ではなく、経路探索付きの通常移動として起点へ向かう命令が
        // 積まれているはず（＝実時間をかけて戻ってくる）。
        assert_eq!(unit.path_final, origin, "起点への経路が設定されていない");
    }
}
