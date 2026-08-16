//! M7: 指揮官 AI（戦略層）。
//!
//! 仕様は `docs/spec/05-ai.md` 5〜7 節、`docs/spec/12-roadmap.md` の M7。
//! データ型（`CommanderAttrs`/`Blackboard`/`SituationAssessment`/`Objective`/
//! `BattlePlan`/`DecisionLog` など）は `organization::CommandNode` の永続状態の
//! 一部として `organization.rs` に置き、このモジュールは `cavalry`/`engineering`
//! が `soldiers`/`combat`/`structures` を外から動かすのと同じやり方で、
//! それらを読み書きする判断ロジックだけを持つ。
//!
//! ## この実装でのスコープ
//!
//! - 仕様 05 章の**個体層（3 節）・戦術層の経路探索や陣形変更タイミング
//!   （4 節）は対象外**。M2〜M6 が既に持っている手続き的なルール
//!   （`steer`/`organization::formation_goals`/`cavalry` の突撃判断など）が
//!   その役割を担っており、汎用ユーティリティ AI へ置き換えるのは
//!   このモジュールとは別の大きな作業になる。
//! - `Blackboard::own_forces`/`terrain_notes` は仕様の型だけ用意してあり、
//!   ここでは埋めない。自軍の戦力は `NodeStats`（毎 tick 更新済み）から
//!   直接読める前提で `force_ratio` を計算するため、これらが空でも
//!   受け入れ条件には影響しない。
//! - `SituationAssessment::critical_points`（橋・高地・崩れかけの味方の検出）
//!   は実装しない。
//! - 視認判定は「地形の cover と高度」を簡略化し、観測者と対象を結ぶ線分上
//!   3 点の地表 cover を平均するだけにしてある（厳密なレイキャスト・
//!   高度差による遮蔽はしない）。
//! - `Objective` は仕様表の 8 種のうち、スコア式が明示されている 3 種
//!   （`Envelop`/`HoldHighGround`/`CommitReserve`）だけを実装する。
//! - `BattlePlan`（8 種）は、それぞれに固有の命令分解ロジックを持たせる
//!   代わりに、ルートノードの `Objective` スコアリングへ加点するバイアスとして
//!   実装する。これにより、アーキタイプ → 性格 → プラン選択 → Objective の
//!   傾向 → 実際に出る命令、という一本の因果の鎖を保ちながら、8 種類分の
//!   個別の分解ロジックを書かずに済む。

use sim_math::{dist, dist_sq, unit_vec, Fx, Purpose, Rng, Vec2Fx};
use sim_terrain::{Terrain, SURFACE_EFFECTS};

use crate::combat::CombatSystem;
use crate::organization::{
    BattlePlan, CommandNode, CommandState, CommandTree, CommanderAttrs, DecisionRecord, FactionId,
    Intent, KnownForce, MoveSpeed, NodeId, Objective, Priority, Side, SituationAssessment,
};
use crate::soldiers::{flags, Soldiers};

/// 戦略層の思考間隔（tick）。仕様 05 章 1 節の帯域（0.2〜1 Hz）のうち 0.5 Hz を採用。
pub(crate) const ASSESS_INTERVAL_TICKS: u32 = 40;
/// 会戦プラン（軍全体の方針）を再検討する間隔。Objective より大きな時間スケールで
/// 動く（仕様 06 章「戦況が大きく変われば再選択する」）。
const PLAN_REASSESS_STRIDE: u32 = 30; // ASSESS_INTERVAL_TICKS の何回に 1 回か
/// 平地での基準視認距離（mm）。地表の cover に応じて短くなる。
const BASE_DETECT_RANGE_MM: i32 = 400_000;
/// 一度も観測されなかったときに 1 回の評価あたり失う confidence。
const CONFIDENCE_DECAY_PER_ASSESS: u16 = 120;
/// 独断専行（`ambition` 駆動）が対象を探す範囲。
const INSUBORDINATION_RANGE_MM: i32 = 250_000;

/// スコアの内訳（ラベルと寄与量）。上位候補一覧にも、選ばれた候補の内訳にも使う。
type ScoreBreakdown = Vec<(&'static str, i32)>;
/// 決定ログに積める形の判断結果: (選ばれたもの, スコア, 上位候補, 内訳)。
type Decision<T> = (T, i32, ScoreBreakdown, ScoreBreakdown);

/// アーキタイプ。仕様 05 章 5.5 節の表。生成時に標準偏差 25 のノイズを乗せる。
pub struct Archetype {
    pub name: &'static str,
    pub means: CommanderAttrs,
}

#[allow(clippy::too_many_arguments)]
const fn attrs(
    boldness: u8,
    caution: u8,
    initiative: u8,
    obedience: u8,
    tactical_skill: u8,
    ambition: u8,
    charisma: u8,
    flexibility: u8,
    patience: u8,
    ruthlessness: u8,
) -> CommanderAttrs {
    CommanderAttrs {
        boldness,
        caution,
        initiative,
        obedience,
        tactical_skill,
        ambition,
        charisma,
        flexibility,
        patience,
        ruthlessness,
    }
}

pub const ARCHETYPES: [Archetype; 8] = [
    Archetype {
        name: "honor_hungry_knight",
        means: attrs(230, 40, 210, 60, 90, 245, 140, 70, 30, 120),
    },
    Archetype {
        name: "veteran_mercenary",
        means: attrs(130, 160, 150, 150, 200, 80, 160, 190, 180, 190),
    },
    Archetype {
        name: "cautious_commander",
        means: attrs(70, 220, 90, 180, 195, 90, 170, 150, 220, 110),
    },
    Archetype {
        name: "stubborn_baron",
        means: attrs(170, 90, 180, 70, 110, 190, 130, 40, 60, 150),
    },
    Archetype {
        name: "disciplinarian",
        means: attrs(110, 150, 60, 230, 170, 70, 150, 90, 170, 160),
    },
    Archetype {
        name: "reckless_youth",
        means: attrs(245, 25, 230, 50, 60, 200, 100, 60, 20, 80),
    },
    Archetype {
        name: "professional_marshal",
        means: attrs(140, 140, 130, 170, 220, 100, 180, 180, 160, 150),
    },
    Archetype {
        name: "cunning_captain",
        means: attrs(150, 130, 190, 110, 210, 110, 140, 220, 150, 200),
    },
];

/// アーキタイプ名から index を引く。
pub fn archetype_index(name: &str) -> Option<usize> {
    ARCHETYPES.iter().position(|a| a.name == name)
}

/// アーキタイプの平均値に標準偏差 25 のノイズを乗せて個体差を出す
/// （仕様 05 章 5.5 節）。
pub fn generate_commander_attrs(rng: &mut Rng, archetype: usize) -> CommanderAttrs {
    const SD: u8 = 25;
    let m = ARCHETYPES[archetype.min(ARCHETYPES.len() - 1)].means;
    CommanderAttrs {
        boldness: rng.attr(m.boldness, SD),
        caution: rng.attr(m.caution, SD),
        initiative: rng.attr(m.initiative, SD),
        obedience: rng.attr(m.obedience, SD),
        tactical_skill: rng.attr(m.tactical_skill, SD),
        ambition: rng.attr(m.ambition, SD),
        charisma: rng.attr(m.charisma, SD),
        flexibility: rng.attr(m.flexibility, SD),
        patience: rng.attr(m.patience, SD),
        ruthlessness: rng.attr(m.ruthlessness, SD),
    }
}

/// 指揮官の性格をアーキタイプから生成し、ノードへ設定する。
pub fn set_archetype(
    command: &mut CommandTree,
    node_id: NodeId,
    archetype: usize,
    world_seed: u64,
    salt: u32,
) {
    let mut rng = Rng::stream(world_seed, salt, Purpose::Spawn, node_id);
    let generated = generate_commander_attrs(&mut rng, archetype);
    command.set_commander_attrs(node_id, generated);
}

// ----------------------------------------------------------------------
// 視認・Blackboard
// ----------------------------------------------------------------------

/// 観測者から対象への視認しやすさ（0..1000）。3 点サンプリングの簡略版
/// （本ファイル冒頭のスコープ注記を参照）。
fn visibility_permille(terrain: &Terrain, from: Vec2Fx, to: Vec2Fx) -> u32 {
    let mut cover_sum = 0u32;
    for permille in [250i32, 500, 750] {
        let t = ((permille as i64 * sim_math::FX_ONE as i64) / 1000) as Fx;
        let p = from.lerp(to, t);
        let s = terrain.surface_at(p.x, p.y);
        cover_sum += SURFACE_EFFECTS[s as usize].cover as u32;
    }
    (1000u32).saturating_sub(cover_sum / 3)
}

/// この指揮官の Blackboard を更新する。異なる陣営の葉ノード（実体を持つ部隊）
/// のうち視認できたものは confidence をほぼ満額にし、見えなかった既知の敵は
/// confidence を減衰させる（仕様 05 章 5.1 節「直接視認」パス、`own_forces`/
/// `terrain_notes` はスコープ外）。
fn update_blackboard(
    node_id: NodeId,
    command: &mut CommandTree,
    soldiers: &Soldiers,
    terrain: &Terrain,
    tick: u32,
) {
    let Some(node) = command.node(node_id) else {
        return;
    };
    let Some(observer_pos) = soldiers.pos_checked(node.commander) else {
        return;
    };
    let faction = node.faction;

    let mut seen: Vec<KnownForce> = Vec::new();
    let mut unseen_ids: Vec<NodeId> = Vec::new();
    for other in &command.nodes {
        if other.faction == faction || other.unit.is_none() || other.stats.alive == 0 {
            continue;
        }
        let dist_mm = sim_math::fx_to_mm(dist(observer_pos, other.stats.centroid));
        let vis = visibility_permille(terrain, observer_pos, other.stats.centroid);
        let effective_range = ((BASE_DETECT_RANGE_MM as i64 * vis as i64) / 1000) as i32;
        if effective_range > 0 && dist_mm <= effective_range {
            let confidence = (1000 - ((dist_mm * 400) / effective_range).clamp(0, 400)) as u16;
            seen.push(KnownForce {
                node: other.id,
                est_pos: other.stats.centroid,
                est_strength: other.stats.alive,
                est_troop_type: 0,
                est_morale: other.stats.avg_morale,
                confidence,
                observed_tick: tick,
            });
        } else {
            unseen_ids.push(other.id);
        }
    }

    let Some(node) = command.node_mut(node_id) else {
        return;
    };
    for kf in seen {
        if let Some(existing) = node
            .blackboard
            .enemy_forces
            .iter_mut()
            .find(|f| f.node == kf.node)
        {
            *existing = kf;
        } else {
            node.blackboard.enemy_forces.push(kf);
        }
    }
    for id in unseen_ids {
        if let Some(existing) = node
            .blackboard
            .enemy_forces
            .iter_mut()
            .find(|f| f.node == id)
        {
            existing.confidence = existing
                .confidence
                .saturating_sub(CONFIDENCE_DECAY_PER_ASSESS);
        }
    }
    node.blackboard.enemy_forces.retain(|f| f.confidence > 0);
    node.blackboard.last_updated = tick;
}

// ----------------------------------------------------------------------
// 戦況評価
// ----------------------------------------------------------------------

struct EnemyView {
    pos: Vec2Fx,
    strength: u32,
    morale: u16,
}

fn enemy_view_true(command: &CommandTree, faction: FactionId) -> Vec<EnemyView> {
    command
        .nodes
        .iter()
        .filter(|n| n.faction != faction && n.unit.is_some() && n.stats.alive > 0)
        .map(|n| EnemyView {
            pos: n.stats.centroid,
            strength: n.stats.alive,
            morale: n.stats.avg_morale,
        })
        .collect()
}

fn enemy_view_perceived(node: &CommandNode) -> Vec<EnemyView> {
    node.blackboard
        .enemy_forces
        .iter()
        .filter(|f| f.confidence > 0)
        .map(|f| EnemyView {
            pos: f.est_pos,
            strength: (f.est_strength as u64 * f.confidence as u64 / 1000) as u32,
            morale: f.est_morale,
        })
        .collect()
}

fn reserve_available(command: &CommandTree, node_id: NodeId) -> u32 {
    let Some(node) = command.node(node_id) else {
        return 0;
    };
    node.children
        .iter()
        .filter_map(|&c| command.node(c))
        .filter(|n| n.stats.alive > 0 && n.stats.engaged_ratio < 200)
        .map(|n| n.stats.alive)
        .sum()
}

fn terrain_advantage(terrain: &Terrain, own_centroid: Vec2Fx, enemies: &[EnemyView]) -> i32 {
    if enemies.is_empty() {
        return 0;
    }
    let own_h = sim_math::fx_to_mm(terrain.height_at(own_centroid.x, own_centroid.y));
    let sum: i64 = enemies
        .iter()
        .map(|e| sim_math::fx_to_mm(terrain.height_at(e.pos.x, e.pos.y)) as i64)
        .sum();
    let avg = (sum / enemies.len() as i64) as i32;
    (own_h - avg) / 100
}

/// `SituationAssessment` を組み立てる（`use_blackboard=false` なら地形・NodeStats
/// から得られる真の値、`true` なら自分の Blackboard 越しの推定値）。
fn assess(
    node_id: NodeId,
    command: &CommandTree,
    terrain: &Terrain,
    use_blackboard: bool,
) -> SituationAssessment {
    let Some(node) = command.node(node_id) else {
        return SituationAssessment::default();
    };
    let own_strength = node.stats.alive.max(subtree_alive(command, node_id));
    let own_morale = node.stats.avg_morale;
    let own_centroid = node.stats.centroid;
    let own_facing = node.stats.facing;

    let enemies = if use_blackboard {
        enemy_view_perceived(node)
    } else {
        enemy_view_true(command, node.faction)
    };
    let enemy_strength: u32 = enemies.iter().map(|e| e.strength).sum();
    let enemy_morale_avg = if enemies.is_empty() {
        500
    } else {
        (enemies.iter().map(|e| e.morale as u64).sum::<u64>() / enemies.len() as u64) as u16
    };

    let force_ratio_permille = ((own_strength as i64 * 1000) / enemy_strength.max(1) as i64) as i32;
    let momentum = own_morale as i32 - enemy_morale_avg as i32;

    let (fc, fs) = unit_vec(own_facing);
    let facing_vec = Vec2Fx::new(fc, fs);
    let mut flank = [0u16; 2];
    let mut rear = 0u16;
    for e in &enemies {
        let rel = e.pos.sub(own_centroid);
        let weight = e.strength.min(u16::MAX as u32) as u16;
        if rel.dot(facing_vec) < 0 {
            rear = rear.saturating_add(weight);
        } else if facing_vec.cross(rel) > 0 {
            flank[0] = flank[0].saturating_add(weight);
        } else {
            flank[1] = flank[1].saturating_add(weight);
        }
    }

    SituationAssessment {
        force_ratio_permille,
        momentum,
        flank_threats: flank,
        rear_threat: rear,
        reserve_available: reserve_available(command, node_id),
        terrain_advantage: terrain_advantage(terrain, own_centroid, &enemies),
        time_pressure: node.stats.avg_fatigue as i32 / 20,
    }
}

fn subtree_alive(command: &CommandTree, node_id: NodeId) -> u32 {
    let Some(node) = command.node(node_id) else {
        return 0;
    };
    if node.unit.is_some() {
        return node.stats.alive;
    }
    node.children
        .iter()
        .map(|&c| subtree_alive(command, c))
        .sum()
}

/// `tactical_skill` が低いほど戦況評価にノイズが乗る（仕様 05 章 5.4 節）。
/// 状況を誤認させるためのもので、選択肢そのものは歪めない。
fn apply_tactical_noise(a: &mut SituationAssessment, tactical_skill: u8, rng: &mut Rng) {
    let noise_scale = (255 - tactical_skill as i32).max(0);
    a.force_ratio_permille += rng.range(-noise_scale, noise_scale) * 4;
    a.momentum += rng.range(-noise_scale, noise_scale) * 3;
    a.terrain_advantage += rng.range(-noise_scale, noise_scale) / 8;
}

/// UI 向け: この指揮官が実際に認識している評価と、地上の実際の状況を並べて返す
/// （仕様の受け入れ条件「認識 vs 実際」）。ノイズは含まない実際値と、
/// Blackboard の古さ・`tactical_skill` のノイズを両方含む認識値。
pub fn perceived_vs_true(
    node_id: NodeId,
    command: &CommandTree,
    terrain: &Terrain,
    world_seed: u64,
    tick: u32,
) -> (SituationAssessment, SituationAssessment) {
    let true_assessment = assess(node_id, command, terrain, false);
    let mut perceived = assess(node_id, command, terrain, true);
    if let Some(node) = command.node(node_id) {
        let mut rng = Rng::stream(world_seed, node.commander, Purpose::CommanderAssess, tick);
        apply_tactical_noise(
            &mut perceived,
            node.commander_attrs.tactical_skill,
            &mut rng,
        );
    }
    (perceived, true_assessment)
}

// ----------------------------------------------------------------------
// Objective の選択
// ----------------------------------------------------------------------

struct Scored {
    score: i32,
    breakdown: Vec<(&'static str, i32)>,
}

impl Scored {
    fn new() -> Self {
        Scored {
            score: 0,
            breakdown: Vec::new(),
        }
    }
    fn add(mut self, label: &'static str, v: i32) -> Self {
        self.score += v;
        self.breakdown.push((label, v));
        self
    }
}

fn score_envelop(a: &SituationAssessment, attrs: &CommanderAttrs, plan_bonus: i32) -> Scored {
    Scored::new()
        .add("force_ratio", (a.force_ratio_permille - 1000).max(0) / 30)
        .add("reserve", (a.reserve_available as i32) * 3)
        .add("boldness", attrs.boldness as i32 * 3 / 10)
        .add("tactical_skill", attrs.tactical_skill as i32 * 2 / 10)
        .add("caution", -(attrs.caution as i32) * 3 / 10)
        .add("plan_bias", plan_bonus)
}

fn score_hold_high_ground(
    a: &SituationAssessment,
    attrs: &CommanderAttrs,
    plan_bonus: i32,
) -> Scored {
    Scored::new()
        .add("terrain_advantage", a.terrain_advantage * 5)
        .add("caution", attrs.caution as i32 * 4 / 10)
        .add(
            "force_ratio_disadvantage",
            (1000 - a.force_ratio_permille).max(0) / 20,
        )
        .add("boldness", -(attrs.boldness as i32) * 2 / 10)
        .add("time_pressure", -a.time_pressure * 2)
        .add("plan_bias", plan_bonus)
}

fn score_commit_reserve(
    a: &SituationAssessment,
    attrs: &CommanderAttrs,
    plan_bonus: i32,
) -> Scored {
    Scored::new()
        .add("front_line_crisis", (a.rear_threat as i32) / 4)
        .add("boldness", attrs.boldness as i32 * 2 / 10)
        .add("momentum", a.momentum.max(0) * 3 / 10)
        .add("caution", -(attrs.caution as i32) * 3 / 10)
        .add("plan_bias", plan_bonus)
}

/// この `BattlePlan` が各 `Objective` のスコアへ与えるバイアス（本ファイル冒頭の
/// スコープ注記を参照。プランごとの命令分解を個別実装する代わりに使う）。
fn plan_objective_bias(plan: Option<BattlePlan>, kind: ObjectiveKind) -> i32 {
    use BattlePlan::*;
    use ObjectiveKind::*;
    match (plan, kind) {
        (Some(CenterPush), Envelop) => 250,
        (Some(DoubleEnvelopment), Envelop) => 400,
        (Some(RefusedFlank), HoldHighGround) => 200,
        (Some(DefendHighGround), HoldHighGround) => 450,
        (Some(FeignedRetreat), CommitReserve) => 300,
        (Some(EchelonAttack), CommitReserve) => 250,
        (Some(CavalryReserve), CommitReserve) => 350,
        (Some(Delay), HoldHighGround) => 300,
        _ => 0,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ObjectiveKind {
    Envelop,
    HoldHighGround,
    CommitReserve,
}

/// 最寄りの（信頼度で重み付けた）既知の敵を、包囲の標的として選ぶ。
fn best_known_target(node: &CommandNode, own_pos: Vec2Fx) -> Option<NodeId> {
    node.blackboard
        .enemy_forces
        .iter()
        .filter(|f| f.confidence > 200)
        .min_by_key(|f| dist_sq(own_pos, f.est_pos) / (f.confidence.max(1) as i64))
        .map(|f| f.node)
}

/// この非葉ノードの `Objective` を選び直すか判断する。`flexibility` が低いほど、
/// 現在の目的からの乗り換えに大きなスコア差が要る（仕様「頑固な指揮官は
/// 最初のプランに固執する」——これは会戦プランだけでなく Objective にも
/// 同じ考え方を適用している）。選ばれなかった場合は現在のものを維持する。
#[allow(clippy::too_many_arguments)]
fn choose_objective(
    node_id: NodeId,
    command: &CommandTree,
    terrain: &Terrain,
    world_seed: u64,
    tick: u32,
) -> Option<Decision<Objective>> {
    let node = command.node(node_id)?;
    let own_pos = node.stats.centroid;
    let assessment = {
        let mut a = assess(node_id, command, terrain, true);
        let mut rng = Rng::stream(world_seed, node.commander, Purpose::CommanderAssess, tick);
        apply_tactical_noise(&mut a, node.commander_attrs.tactical_skill, &mut rng);
        a
    };
    let attrs = node.commander_attrs;
    let plan = node.battle_plan;

    let mut candidates: Vec<(Objective, Scored)> = Vec::new();
    candidates.push((
        Objective::HoldHighGround,
        score_hold_high_ground(
            &assessment,
            &attrs,
            plan_objective_bias(plan, ObjectiveKind::HoldHighGround),
        ),
    ));
    candidates.push((
        Objective::CommitReserve,
        score_commit_reserve(
            &assessment,
            &attrs,
            plan_objective_bias(plan, ObjectiveKind::CommitReserve),
        ),
    ));
    if let Some(target) = best_known_target(node, own_pos) {
        candidates.push((
            Objective::Envelop { target },
            score_envelop(
                &assessment,
                &attrs,
                plan_objective_bias(plan, ObjectiveKind::Envelop),
            ),
        ));
    }

    candidates.sort_by_key(|(_, s)| -s.score);
    let (best_obj, best) = candidates.first()?;
    let current_score = node.strategic_objective.and_then(|cur| {
        candidates
            .iter()
            .find(|(o, _)| objective_kind(*o) == objective_kind(cur))
            .map(|(_, s)| s.score)
    });
    let switch_margin = (255 - attrs.flexibility as i32) * 2;
    let should_switch = match (node.strategic_objective, current_score) {
        (None, _) => true,
        (Some(_), None) => true,
        (Some(_), Some(cur)) => best.score > cur + switch_margin,
    };
    if !should_switch {
        return None;
    }
    let names: Vec<(&'static str, i32)> = candidates
        .iter()
        .take(3)
        .map(|(o, s)| (objective_name(*o), s.score))
        .collect();
    Some((*best_obj, best.score, names, best.breakdown.clone()))
}

fn objective_kind(o: Objective) -> ObjectiveKind {
    match o {
        Objective::Envelop { .. } => ObjectiveKind::Envelop,
        Objective::HoldHighGround => ObjectiveKind::HoldHighGround,
        Objective::CommitReserve => ObjectiveKind::CommitReserve,
    }
}

fn objective_name(o: Objective) -> &'static str {
    match o {
        Objective::Envelop { .. } => "Envelop",
        Objective::HoldHighGround => "HoldHighGround",
        Objective::CommitReserve => "CommitReserve",
    }
}

/// 子の部隊のうち騎乗兵の割合（‰）。
fn child_mounted_permille(command: &CommandTree, soldiers: &Soldiers, child: NodeId) -> u32 {
    let ids = command.subtree_soldiers(child);
    if ids.is_empty() {
        return 0;
    }
    let mounted = ids
        .iter()
        .filter(|&&id| {
            soldiers
                .index_if_present(id)
                .is_some_and(|i| soldiers.hot.flags[i] & flags::MOUNTED != 0)
        })
        .count();
    (mounted as u64 * 1000 / ids.len() as u64) as u32
}

/// 選んだ `Objective` を子ノードへの `Order` に分解する（仕様 05 章 5.3 節）。
fn decompose_objective(
    node_id: NodeId,
    objective: Objective,
    command: &mut CommandTree,
    soldiers: &Soldiers,
    tick: u32,
) {
    let Some(node) = command.node(node_id) else {
        return;
    };
    let children = node.children.clone();
    if children.is_empty() {
        return;
    }
    let own_facing = node.stats.facing;
    let own_centroid = node.stats.centroid;

    match objective {
        Objective::HoldHighGround => {
            for &child in &children {
                let pos = command
                    .node(child)
                    .map(|n| n.stats.centroid)
                    .unwrap_or(own_centroid);
                command.issue_order(
                    node_id,
                    child,
                    Intent::Hold {
                        pos,
                        facing: own_facing,
                        allow_pursuit: false,
                    },
                    Priority::Routine,
                    tick,
                    soldiers,
                );
            }
        }
        Objective::CommitReserve => {
            for &child in &children {
                let Some(c) = command.node(child) else {
                    continue;
                };
                if c.stats.alive == 0 || c.stats.engaged_ratio >= 200 {
                    continue;
                }
                let formation = c.unit.as_ref().map(|u| u.formation).unwrap_or_default();
                command.issue_order(
                    node_id,
                    child,
                    Intent::MoveTo {
                        pos: own_centroid,
                        facing: own_facing,
                        speed: MoveSpeed::Quick,
                        formation,
                    },
                    Priority::Urgent,
                    tick,
                    soldiers,
                );
            }
        }
        Objective::Envelop { target } => {
            let (fc, fs) = unit_vec(own_facing);
            let facing_vec = Vec2Fx::new(fc, fs);
            let mut scored: Vec<(NodeId, i64, u32)> = children
                .iter()
                .filter_map(|&c| {
                    let cn = command.node(c)?;
                    if cn.stats.alive == 0 {
                        return None;
                    }
                    let lateral = facing_vec.cross(cn.stats.centroid.sub(own_centroid));
                    let mounted = child_mounted_permille(command, soldiers, c);
                    Some((c, lateral, mounted))
                })
                .collect();
            if scored.is_empty() {
                return;
            }
            scored.sort_by_key(|&(_, lateral, _)| lateral);
            let n = scored.len();
            for (idx, &(child, _, mounted)) in scored.iter().enumerate() {
                let intent = if mounted >= 500 {
                    Intent::Charge { target }
                } else if idx == 0 && n > 1 {
                    Intent::Flank {
                        target,
                        side: Side::Left,
                    }
                } else if idx + 1 == n && n > 1 {
                    Intent::Flank {
                        target,
                        side: Side::Right,
                    }
                } else {
                    Intent::Attack {
                        target,
                        approach: crate::organization::ApproachStyle::Deliberate,
                    }
                };
                command.issue_order(node_id, child, intent, Priority::Urgent, tick, soldiers);
            }
        }
    }
}

// ----------------------------------------------------------------------
// 会戦プラン（軍全体）
// ----------------------------------------------------------------------

struct ArmyComposition {
    cavalry_permille: u32,
    ranged_permille: u32,
    unit_count: u32,
}

fn army_composition(
    command: &CommandTree,
    soldiers: &Soldiers,
    combat: &CombatSystem,
    node_id: NodeId,
) -> ArmyComposition {
    let ids = command.subtree_soldiers(node_id);
    let total = ids.len().max(1);
    let mut mounted = 0usize;
    let mut ranged = 0usize;
    for &id in &ids {
        if let Some(i) = soldiers.index_if_present(id) {
            if soldiers.hot.flags[i] & flags::MOUNTED != 0 {
                mounted += 1;
            }
            if combat.weapons.get(i).is_some_and(|w| w.ranged) {
                ranged += 1;
            }
        }
    }
    let unit_count = count_leaf_units(command, node_id);
    ArmyComposition {
        cavalry_permille: (mounted as u64 * 1000 / total as u64) as u32,
        ranged_permille: (ranged as u64 * 1000 / total as u64) as u32,
        unit_count,
    }
}

fn count_leaf_units(command: &CommandTree, node_id: NodeId) -> u32 {
    let Some(node) = command.node(node_id) else {
        return 0;
    };
    if node.unit.is_some() {
        return 1;
    }
    node.children
        .iter()
        .map(|&c| count_leaf_units(command, c))
        .sum()
}

fn score_battle_plans(
    a: &SituationAssessment,
    attrs: &CommanderAttrs,
    comp: &ArmyComposition,
) -> Vec<(BattlePlan, Scored)> {
    use BattlePlan::*;
    let force_adv = (a.force_ratio_permille - 1000) / 20;
    let force_disadv = (1000 - a.force_ratio_permille).max(0) / 20;
    vec![
        (
            CenterPush,
            Scored::new()
                .add("force_ratio", force_adv * 2)
                .add("boldness", attrs.boldness as i32 * 2 / 10)
                .add("caution", -(attrs.caution as i32) * 2 / 10),
        ),
        (
            DoubleEnvelopment,
            Scored::new()
                .add("force_ratio", force_adv)
                .add("cavalry", comp.cavalry_permille as i32 * 2 / 100)
                .add("tactical_skill", attrs.tactical_skill as i32 * 2 / 10)
                .add("boldness", attrs.boldness as i32 / 10),
        ),
        (
            RefusedFlank,
            Scored::new()
                .add("force_disadvantage", force_disadv)
                .add("terrain_advantage", a.terrain_advantage * 2)
                .add("caution", attrs.caution as i32 / 10),
        ),
        (
            DefendHighGround,
            Scored::new()
                .add("ranged", comp.ranged_permille as i32 * 3 / 100)
                .add("force_disadvantage", force_disadv * 2)
                .add("terrain_advantage", a.terrain_advantage * 3)
                .add("caution", attrs.caution as i32 * 2 / 10),
        ),
        (
            FeignedRetreat,
            Scored::new()
                .add("tactical_skill", attrs.tactical_skill as i32 * 2 / 10)
                .add("ambition_inverse", -(attrs.ambition as i32) / 10)
                .add("initiative", attrs.initiative as i32 / 10),
        ),
        (
            EchelonAttack,
            Scored::new()
                .add("unit_count", comp.unit_count as i32 * 5)
                .add("patience", attrs.patience as i32 * 2 / 10),
        ),
        (
            CavalryReserve,
            Scored::new()
                .add("cavalry", comp.cavalry_permille as i32 * 3 / 100)
                .add("patience", attrs.patience as i32 * 2 / 10),
        ),
        (
            Delay,
            Scored::new()
                .add("force_disadvantage", force_disadv * 3)
                .add("boldness_inverse", -(attrs.boldness as i32) / 10),
        ),
    ]
}

fn plan_name(p: BattlePlan) -> &'static str {
    use BattlePlan::*;
    match p {
        CenterPush => "CenterPush",
        DoubleEnvelopment => "DoubleEnvelopment",
        RefusedFlank => "RefusedFlank",
        DefendHighGround => "DefendHighGround",
        FeignedRetreat => "FeignedRetreat",
        EchelonAttack => "EchelonAttack",
        CavalryReserve => "CavalryReserve",
        Delay => "Delay",
    }
}

fn choose_battle_plan(
    node_id: NodeId,
    command: &CommandTree,
    soldiers: &Soldiers,
    terrain: &Terrain,
    combat: &CombatSystem,
    world_seed: u64,
    tick: u32,
) -> Option<Decision<BattlePlan>> {
    let node = command.node(node_id)?;
    let assessment = {
        let mut a = assess(node_id, command, terrain, true);
        let mut rng = Rng::stream(world_seed, node.commander, Purpose::CommanderAssess, tick);
        apply_tactical_noise(&mut a, node.commander_attrs.tactical_skill, &mut rng);
        a
    };
    let comp = army_composition(command, soldiers, combat, node_id);
    let mut candidates = score_battle_plans(&assessment, &node.commander_attrs, &comp);
    candidates.sort_by_key(|(_, s)| -s.score);
    let (best_plan, best) = candidates.first()?;

    let current_score = node.battle_plan.and_then(|cur| {
        candidates
            .iter()
            .find(|(p, _)| *p == cur)
            .map(|(_, s)| s.score)
    });
    let switch_margin = (255 - node.commander_attrs.flexibility as i32) * 2;
    let should_switch = match (node.battle_plan, current_score) {
        (None, _) => true,
        (Some(_), None) => true,
        (Some(_), Some(cur)) => best.score > cur + switch_margin,
    };
    if !should_switch {
        return None;
    }
    let names = candidates
        .iter()
        .take(3)
        .map(|(p, s)| (plan_name(*p), s.score))
        .collect();
    Some((*best_plan, best.score, names, best.breakdown.clone()))
}

// ----------------------------------------------------------------------
// 独断専行（ambition 駆動）
// ----------------------------------------------------------------------

/// 命令に反して独断で突撃するかを判定する（仕様の受け入れ条件「ambition の
/// 高い騎士が命令を無視して突撃する」）。既に攻勢的な行動中なら判定しない。
fn maybe_insubordinate(
    node_id: NodeId,
    command: &mut CommandTree,
    soldiers: &mut Soldiers,
    terrain: &Terrain,
    world_seed: u64,
    tick: u32,
) {
    let Some(node) = command.node(node_id) else {
        return;
    };
    if matches!(
        node.objective,
        Some(Intent::Charge { .. }) | Some(Intent::Attack { .. })
    ) {
        return;
    }
    let Some(ci) = soldiers.index_if_present(node.commander) else {
        return;
    };
    let own_pos = soldiers.pos(ci);
    let Some(target_node) = best_known_target(node, own_pos) else {
        return;
    };
    let Some(target_force) = node
        .blackboard
        .enemy_forces
        .iter()
        .find(|f| f.node == target_node)
    else {
        return;
    };
    let dist_mm = sim_math::fx_to_mm(dist(own_pos, target_force.est_pos));
    if dist_mm > INSUBORDINATION_RANGE_MM {
        return;
    }

    let cattrs = node.commander_attrs;
    let personal_self_preservation = soldiers.attrs[ci].self_preservation as i32;
    let urge = cattrs.ambition as i32 * 3 - cattrs.obedience as i32 * 2 + cattrs.initiative as i32
        - personal_self_preservation;
    let permille = urge.clamp(0, 950) as u32;

    let mut rng = Rng::stream(world_seed, node.commander, Purpose::CommanderAssess, tick);
    let breakdown = vec![
        ("ambition", cattrs.ambition as i32 * 3),
        ("obedience", -(cattrs.obedience as i32) * 2),
        ("initiative", cattrs.initiative as i32),
        ("self_preservation", -personal_self_preservation),
    ];
    if !rng.chance_permille(permille.min(950)) {
        if let Some(node) = command.node_mut(node_id) {
            node.decision_log.push(DecisionRecord {
                tick,
                chosen: "命令を維持",
                chosen_score: 0,
                candidates: vec![("独断で突撃", urge), ("命令を維持", 0)],
                breakdown,
            });
        }
        return;
    }

    command.override_intent(
        node_id,
        Intent::Charge {
            target: target_node,
        },
        soldiers,
        terrain,
        tick,
    );
    if let Some(node) = command.node_mut(node_id) {
        node.decision_log.push(DecisionRecord {
            tick,
            chosen: "独断で突撃",
            chosen_score: urge,
            candidates: vec![("独断で突撃", urge), ("命令を維持", 0)],
            breakdown,
        });
    }
}

// ----------------------------------------------------------------------
// 上位のオーケストレーション
// ----------------------------------------------------------------------

/// M7 の戦略層 AI を 1 tick 分進める。`World::tick` から呼ぶ。
pub fn tick(
    command: &mut CommandTree,
    soldiers: &mut Soldiers,
    terrain: &Terrain,
    combat: &CombatSystem,
    world_seed: u64,
    tick: u32,
) {
    let node_ids: Vec<NodeId> = command.nodes.iter().map(|n| n.id).collect();
    for node_id in node_ids {
        let Some(node) = command.node(node_id) else {
            continue;
        };
        if node.command_state != CommandState::Commanded {
            continue;
        }
        if !soldiers.is_active_id(node.commander) {
            continue;
        }
        if tick < node.next_assess_tick {
            continue;
        }
        let is_leaf = node.unit.is_some();
        let is_root = node.parent.is_none();

        update_blackboard(node_id, command, soldiers, terrain, tick);

        if is_leaf {
            maybe_insubordinate(node_id, command, soldiers, terrain, world_seed, tick);
        } else {
            if is_root {
                let assess_count = tick / ASSESS_INTERVAL_TICKS;
                let plan_due = command
                    .node(node_id)
                    .is_some_and(|n| n.battle_plan.is_none())
                    || assess_count % PLAN_REASSESS_STRIDE == 0;
                if plan_due {
                    if let Some((plan, score, candidates, breakdown)) = choose_battle_plan(
                        node_id, command, soldiers, terrain, combat, world_seed, tick,
                    ) {
                        if let Some(node) = command.node_mut(node_id) {
                            node.battle_plan = Some(plan);
                            node.decision_log.push(DecisionRecord {
                                tick,
                                chosen: plan_name(plan),
                                chosen_score: score,
                                candidates,
                                breakdown,
                            });
                        }
                    }
                }
            }
            if let Some((objective, score, candidates, breakdown)) =
                choose_objective(node_id, command, terrain, world_seed, tick)
            {
                if let Some(node) = command.node_mut(node_id) {
                    node.strategic_objective = Some(objective);
                    node.decision_log.push(DecisionRecord {
                        tick,
                        chosen: objective_name(objective),
                        chosen_score: score,
                        candidates,
                        breakdown,
                    });
                }
                decompose_objective(node_id, objective, command, soldiers, tick);
            }
        }

        if let Some(node) = command.node_mut(node_id) {
            node.next_assess_tick = tick + ASSESS_INTERVAL_TICKS;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::organization::{FormationId, Unit};
    use crate::soldiers::{Attrs, SoldierId};
    use sim_math::{brad_from_deg, fx};
    use sim_terrain::{generate, TerrainParams};

    fn small_terrain(seed: u64) -> Terrain {
        generate(&TerrainParams {
            seed,
            size_m: 400,
            cell_m: 2,
            relief: 50,
            thermal_iterations: 2,
            river_density: 0,
            road_count: 0,
            ..Default::default()
        })
    }

    fn leaf_unit(soldiers: Vec<SoldierId>, origin: Vec2Fx, facing: sim_math::Brad) -> Unit {
        Unit {
            soldiers,
            troop_type: 0,
            formation: 0 as FormationId,
            formation_origin: origin,
            formation_facing: facing,
            ranks: 1,
            file_spacing: sim_math::fx_from_mm(800),
            rank_spacing: sim_math::fx_from_mm(800),
            banner: None,
            formation_change: None,
            path: Vec::new(),
            path_final: origin,
            pursuit_leash: None,
        }
    }

    #[test]
    fn archetypes_generate_distinct_and_bounded_attrs() {
        let mut rng = Rng::from_state(42);
        let bold = generate_commander_attrs(&mut rng, archetype_index("reckless_youth").unwrap());
        let caut =
            generate_commander_attrs(&mut rng, archetype_index("cautious_commander").unwrap());
        assert!(bold.boldness > caut.boldness);
        assert!(caut.caution > bold.caution);
    }

    #[test]
    fn tactical_noise_scales_with_skill() {
        let assessment = SituationAssessment {
            force_ratio_permille: 1000,
            ..Default::default()
        };
        let mut skilled = assessment;
        let mut unskilled = assessment;
        let mut r1 = Rng::from_state(1);
        let mut r2 = Rng::from_state(1);
        apply_tactical_noise(&mut skilled, 250, &mut r1);
        apply_tactical_noise(&mut unskilled, 20, &mut r2);
        let skilled_dev = (skilled.force_ratio_permille - 1000).abs();
        let unskilled_dev = (unskilled.force_ratio_permille - 1000).abs();
        assert!(
            unskilled_dev >= skilled_dev,
            "低スキルの方がノイズが小さい: skilled={skilled_dev} unskilled={unskilled_dev}"
        );
    }

    #[test]
    fn blackboard_confidence_decays_when_not_observed() {
        let mut command = CommandTree::new();
        let terrain = small_terrain(1);
        let mut soldiers = Soldiers::default();
        let observer = soldiers.push(fx(10), fx(10), 0, 0, 0, Attrs::default(), 0);
        let enemy = soldiers.push(fx(390), fx(390), 0, 0, 1, Attrs::default(), 0);

        let observer_node = command.add_node(
            None,
            1,
            0,
            observer,
            vec![],
            Some(leaf_unit(vec![observer], Vec2Fx::new(fx(10), fx(10)), 0)),
        );
        let _enemy_node = command.add_node(
            None,
            1,
            1,
            enemy,
            vec![],
            Some(leaf_unit(vec![enemy], Vec2Fx::new(fx(390), fx(390)), 0)),
        );
        command.tick(&mut soldiers, &terrain, 0);

        update_blackboard(observer_node, &mut command, &soldiers, &terrain, 0);
        // 遠すぎて見えないはずなので、まだ何も知らない
        assert!(command
            .node(observer_node)
            .unwrap()
            .blackboard
            .enemy_forces
            .is_empty());
    }

    #[test]
    fn blackboard_sees_a_nearby_enemy_and_confidence_decays_once_it_moves_away() {
        let mut command = CommandTree::new();
        let terrain = small_terrain(1);
        let mut soldiers = Soldiers::default();
        let observer = soldiers.push(fx(200), fx(200), 0, 0, 0, Attrs::default(), 0);
        let enemy = soldiers.push(fx(210), fx(200), 0, 0, 1, Attrs::default(), 0);

        let observer_node = command.add_node(
            None,
            1,
            0,
            observer,
            vec![],
            Some(leaf_unit(vec![observer], Vec2Fx::new(fx(200), fx(200)), 0)),
        );
        let enemy_node = command.add_node(
            None,
            1,
            1,
            enemy,
            vec![],
            Some(leaf_unit(vec![enemy], Vec2Fx::new(fx(210), fx(200)), 0)),
        );
        command.tick(&mut soldiers, &terrain, 0);

        update_blackboard(observer_node, &mut command, &soldiers, &terrain, 0);
        let confidence_seen = command
            .node(observer_node)
            .unwrap()
            .blackboard
            .enemy_forces
            .iter()
            .find(|f| f.node == enemy_node)
            .map(|f| f.confidence)
            .unwrap_or(0);
        assert!(
            confidence_seen > 500,
            "近くの敵が見えていない: {confidence_seen}"
        );

        // 敵を遠ざけて見えなくする
        soldiers.set_pos(enemy as usize, Vec2Fx::new(fx(395), fx(395)));
        command.tick(&mut soldiers, &terrain, 100);
        update_blackboard(observer_node, &mut command, &soldiers, &terrain, 100);
        let confidence_after = command
            .node(observer_node)
            .unwrap()
            .blackboard
            .enemy_forces
            .iter()
            .find(|f| f.node == enemy_node)
            .map(|f| f.confidence)
            .unwrap_or(0);
        assert!(
            confidence_after < confidence_seen,
            "見えなくなっても confidence が下がっていない: before={confidence_seen} after={confidence_after}"
        );
    }

    /// 仕様 12 章 M7 の受け入れ条件「ambition の高い騎士が命令を無視して
    /// 突撃する事例が発生する」。
    #[test]
    fn ambitious_low_obedience_commander_eventually_disobeys_a_hold_order() {
        let trials = 20;
        let mut disobeyed = 0;
        for trial in 0..trials {
            let mut command = CommandTree::new();
            let terrain = small_terrain(1);
            let mut soldiers = Soldiers::default();
            let knight = soldiers.push(
                fx(200),
                fx(200),
                brad_from_deg(0),
                0,
                0,
                Attrs::default(),
                0,
            );
            let enemy = soldiers.push(fx(210), fx(200 + trial), 0, 0, 1, Attrs::default(), 0);

            let node = command.add_node(
                None,
                2,
                0,
                knight,
                vec![],
                Some(leaf_unit(vec![knight], Vec2Fx::new(fx(200), fx(200)), 0)),
            );
            let _enemy_node = command.add_node(
                None,
                2,
                1,
                enemy,
                vec![],
                Some(leaf_unit(
                    vec![enemy],
                    Vec2Fx::new(fx(210), fx(200 + trial)),
                    0,
                )),
            );
            command.set_commander_attrs(
                node,
                attrs(230, 40, 210, 30, 90, 250, 140, 70, 30, 120), // honor_hungry_knight 相当、より極端
            );
            command.node_mut(node).unwrap().objective = Some(Intent::Hold {
                pos: Vec2Fx::new(fx(200), fx(200)),
                facing: 0,
                allow_pursuit: false,
            });
            command.tick(&mut soldiers, &terrain, 0);
            update_blackboard(node, &mut command, &soldiers, &terrain, 0);

            maybe_insubordinate(
                node,
                &mut command,
                &mut soldiers,
                &terrain,
                0xC0FFEE + trial as u64,
                trial as u32,
            );
            if matches!(
                command.node(node).unwrap().objective,
                Some(Intent::Charge { .. })
            ) {
                disobeyed += 1;
            }
        }
        assert!(
            disobeyed * 2 >= trials,
            "野心の強い指揮官が十分な頻度で独断突撃していない: {disobeyed}/{trials}"
        );
    }

    /// 忠実で野心の薄い指揮官は、同じ状況でもほとんど独断しない。
    #[test]
    fn obedient_low_ambition_commander_rarely_disobeys() {
        let trials = 20;
        let mut disobeyed = 0;
        for trial in 0..trials {
            let mut command = CommandTree::new();
            let terrain = small_terrain(1);
            let mut soldiers = Soldiers::default();
            let commander = soldiers.push(
                fx(200),
                fx(200),
                brad_from_deg(0),
                0,
                0,
                Attrs::default(),
                0,
            );
            let enemy = soldiers.push(fx(210), fx(200 + trial), 0, 0, 1, Attrs::default(), 0);

            let node = command.add_node(
                None,
                2,
                0,
                commander,
                vec![],
                Some(leaf_unit(vec![commander], Vec2Fx::new(fx(200), fx(200)), 0)),
            );
            let _enemy_node = command.add_node(
                None,
                2,
                1,
                enemy,
                vec![],
                Some(leaf_unit(
                    vec![enemy],
                    Vec2Fx::new(fx(210), fx(200 + trial)),
                    0,
                )),
            );
            command.set_commander_attrs(node, attrs(110, 150, 60, 230, 170, 20, 150, 90, 170, 160));
            command.node_mut(node).unwrap().objective = Some(Intent::Hold {
                pos: Vec2Fx::new(fx(200), fx(200)),
                facing: 0,
                allow_pursuit: false,
            });
            command.tick(&mut soldiers, &terrain, 0);
            update_blackboard(node, &mut command, &soldiers, &terrain, 0);

            maybe_insubordinate(
                node,
                &mut command,
                &mut soldiers,
                &terrain,
                0xC0FFEE + trial as u64,
                trial as u32,
            );
            if matches!(
                command.node(node).unwrap().objective,
                Some(Intent::Charge { .. })
            ) {
                disobeyed += 1;
            }
        }
        assert!(
            disobeyed * 4 <= trials,
            "従順な指揮官が独断しすぎている: {disobeyed}/{trials}"
        );
    }
}
