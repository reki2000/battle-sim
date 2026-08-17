//! 編成・指揮・陣形（M3）。
//!
//! 指揮ツリーは階層の名前を知らない汎用エンジンとして実装する。シナリオ側は
//! `CommandNode` を任意の深さで構成し、葉ノードに `Unit` を接続する。命令は
//! 伝令ごとに距離から遅延を計算するため、発令順や走査順に結果が依存しない。

use sim_math::{
    dist, fx, fx_from_mm, fx_mul, ms_to_ticks, per_sec_to_per_tick, Brad, Fx, Vec2Fx, FX_ONE,
};
use sim_terrain::Terrain;

use crate::pathing;
use crate::soldiers::{SoldierId, Soldiers, State};

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
/// `own_forces` と `terrain_notes` はこの PR では埋めない（本文冒頭のスコープ
/// 注記を参照）。自軍の戦力は `NodeStats` から直接読める前提で扱う。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blackboard {
    pub own_forces: Vec<KnownForce>,
    pub enemy_forces: Vec<KnownForce>,
    pub terrain_notes: Vec<TerrainNote>,
    pub last_updated: u32,
}

/// 戦況評価。仕様 05 章 5.2 節。`critical_points` は本 PR のスコープ外（本文参照）。
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
    pub formation_origin: Vec2Fx,
    pub formation_facing: Brad,
    pub ranks: u16,
    pub file_spacing: Fx,
    pub rank_spacing: Fx,
    pub banner: Option<u16>,
    pub formation_change: Option<FormationChange>,
    /// 残りの経路ウェイポイント（次に目指す点が先頭）。`formation_origin` が
    /// 現在の目標であり、ここには「その先」の点だけが入る（仕様 12 章 M2）。
    pub path: Vec<Vec2Fx>,
    /// 経路探索の最終目的地。到達判定や再計算の要否に使う。
    pub path_final: Vec2Fx,
    /// 追撃中なら、その上限距離と起点（仕様 12 章 M5）。
    pub pursuit_leash: Option<PursuitLeash>,
}

/// 側面攻撃の目標を、敵部隊の重心から横へずらす距離（m）。真正面へ向かわせると
/// 側面へ回り込まずに正面衝突するので、いったん敵の横を目指させる。
const FLANK_OFFSET_M: i32 = 60;

/// 追撃の紐（leash）。仕様 12 章 M5「追撃に出た騎兵が戻ってくるのに
/// 現実的な時間がかかる」を実装するための、部隊単位の上限距離。
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
}

#[derive(Clone, Debug, Default)]
pub struct CommandTree {
    pub nodes: Vec<CommandNode>,
    pub messengers: Vec<Messenger>,
    pub events: Vec<CommandEvent>,
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
        self.tick_pursuit(soldiers, terrain);
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
            if compliance != Compliance::Ignored {
                node.objective = Some(messenger.order.intent);
            }
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
        if let Some(node) = self.node_mut(node_id) {
            node.objective = Some(intent);
        }
        self.apply_intent(node_id, intent, soldiers, terrain, tick);
        self.events.push(CommandEvent {
            tick,
            node: node_id,
            order: None,
            kind: CommandEventKind::Insubordination,
        });
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
        // ノードを可変借用する前に、他ノード（Charge/Pursue の対象）の重心と
        // この部隊の現在地を読んでおく。
        let target_centroid = match intent {
            Intent::Charge { target }
            | Intent::Pursue { target, .. }
            | Intent::Attack { target, .. } => self.node(target).map(|n| n.stats.centroid),
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

    /// 部隊の移動目標を設定する。粗い A* で経路を求め、経路の最初のウェイポイントを
    /// `formation_origin`（陣形が追従する即時目標）に、残りを `unit.path` に積む。
    /// 経路が見つからない場合は直線移動にフォールバックする。
    fn set_movement_target(
        &mut self,
        node_id: NodeId,
        destination: Vec2Fx,
        soldiers: &Soldiers,
        terrain: &Terrain,
    ) {
        let start = self.unit_origin(node_id, soldiers);
        let mut waypoints =
            pathing::find_path(terrain, start, destination).unwrap_or_else(|| vec![destination]);
        let Some(node) = self.node_mut(node_id) else {
            return;
        };
        let Some(unit) = node.unit.as_mut() else {
            return;
        };
        let first = if waypoints.is_empty() {
            destination
        } else {
            waypoints.remove(0)
        };
        unit.formation_origin = first;
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

    /// 葉ノードの兵士に、現在の陣形スロットを目標として設定する。
    pub fn formation_goals(&mut self, soldiers: &mut Soldiers, goals: &mut [Vec2Fx], tick: u32) {
        for index in 0..self.nodes.len() {
            let mut formation_changed = false;
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
                // 経路上の現在のウェイポイントに近づいたら、次のウェイポイントへ進む。
                // 重心は前ティックの `update_stats` の結果なので 1 tick 遅れるが、
                // 隊列が長いユニットでは十分な精度。
                if !unit.path.is_empty() {
                    let centroid = self
                        .nodes
                        .get(index)
                        .map(|n| n.stats.centroid)
                        .unwrap_or(Vec2Fx::ZERO);
                    let unit = self.nodes[index].unit.as_mut().unwrap();
                    if dist(centroid, unit.formation_origin) <= pathing::arrival_radius() {
                        unit.formation_origin = unit.path.remove(0);
                    }
                }
                let unit = self.nodes[index].unit.as_mut().unwrap();
                let transitioning = unit.formation_change.is_some();
                let alive = unit
                    .soldiers
                    .iter()
                    .filter(|&&id| soldiers.is_active_id(id))
                    .count() as u32;
                if alive == 0 {
                    continue;
                }
                let ranks = unit.ranks.max(1) as u32;
                let files = alive.div_ceil(ranks).max(1);
                let (sin, cos) = (
                    sim_math::sin_fx(unit.formation_facing),
                    sim_math::cos_fx(unit.formation_facing),
                );
                let mut alive_slot = 0u32;
                for &id in &unit.soldiers {
                    let i = id as usize;
                    if i >= goals.len() || !soldiers.is_active_id(id) {
                        continue;
                    }
                    let file = alive_slot % files;
                    let rank = alive_slot / files;
                    let local_x = (file as Fx * unit.file_spacing)
                        - ((files.saturating_sub(1) as Fx * unit.file_spacing) / 2);
                    let local_y = rank as Fx * unit.rank_spacing;
                    let rotated = Vec2Fx::new(
                        fx_mul(local_x, cos) - fx_mul(local_y, sin),
                        fx_mul(local_x, sin) + fx_mul(local_y, cos),
                    );
                    goals[i] = unit.formation_origin.add(rotated);
                    if transitioning {
                        if soldiers.hot.state[i] == State::Idle {
                            soldiers.hot.state[i] = State::Repositioning;
                        }
                    } else {
                        if soldiers.hot.state[i] == State::Repositioning {
                            soldiers.hot.state[i] = State::Marching;
                        }
                        if soldiers.hot.state[i] == State::Idle && soldiers.pos(i) != goals[i] {
                            soldiers.hot.state[i] = State::Marching;
                        }
                    }
                    alive_slot += 1;
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
        for node in &self.nodes {
            mix(node.id as u64);
            mix(node.commander as u64);
            mix(node.command_state as u64);
            mix(node.received_order.map_or(u64::MAX, |o| o.id as u64));
            if let Some(unit) = &node.unit {
                mix(unit.formation as u64);
                mix(unit.formation_origin.x as u32 as u64);
                mix(unit.formation_origin.y as u32 as u64);
                mix(unit.path.len() as u64);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soldiers::Attrs;

    fn flat_terrain(size_m: u32) -> Terrain {
        sim_terrain::generate(&sim_terrain::TerrainParams {
            seed: 1,
            size_m,
            cell_m: 4,
            relief: 0,
            thermal_iterations: 0,
            ..Default::default()
        })
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
    fn formation_slots_are_deterministic_and_repack_after_death() {
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
        assert_ne!(before, goals[3]);
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
