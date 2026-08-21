//! 固定 seed の回帰シナリオ（B0）。
//!
//! 戦場での個体挙動を後続フェーズ（B1〜B6）で作り変えるとき、「良くなったか」
//! 「性能が落ちていないか」を毎回同じ状況で測るための台本を置く。どのシナリオも
//! 台本（[`RegressionScenario::script`]）以外に外乱を入れないので、同じ seed なら
//! 指標も状態ハッシュも毎回一致する。
//!
//! 状況は仕様上そろえたい 5 つ:
//!
//! 1. `reserve_passes_melee` — 正面衝突中の横を予備隊が通過する。
//! 2. `column_front_losses` — 縦隊の前列に損耗が生じ、後列が穴を埋める。
//! 3. `defile_then_reform` — 狭路を通過してから開けた場所で再整列する。
//! 4. `guard_area_feint`   — 防衛地点へ敵が接近し、一部が陽動で通過する。
//! 5. `hunt_person_lost`   — 指揮官を追跡中に対象が倒れる。
//!
//! `sim-headless regress` から実行でき、`cargo test` でも同じ関数を回す。

use sim_math::{cos_fx, fx, fx_from_mm, fx_mul, sin_fx, Brad, Purpose, Rng, Vec2Fx, BRAD_HALF};
use sim_terrain::{Terrain, WaterKind};

use crate::metrics::{BattlefieldObserver, MetricsConfig, MetricsReport};
use crate::organization::{
    FormationId, Intent, MoveSpeed, NodeId, Unit, FORMATION_COLUMN, FORMATION_LINE,
    FORMATION_SHIELDWALL,
};
use crate::soldiers::{Attrs, SoldierId};
use crate::World;

/// 地形セルの一辺（m）。回帰シナリオは地形の細部を問わないので固定でよい。
const CELL_M: u32 = 2;
/// 台本が兵士を倒すときのダメージ。最大体力を確実に上回る値。
const SCRIPTED_STRIKE_DAMAGE: u16 = 500;

/// 1 つの回帰シナリオ。
pub struct RegressionScenario {
    pub id: &'static str,
    pub name_ja: &'static str,
    /// 何を見るためのシナリオかの 1 行説明。
    pub watches_ja: &'static str,
    pub seed: u64,
    /// 既定の実行 tick 数。
    pub ticks: u32,
    build: fn(u64) -> World,
    /// 各 tick の直前に呼ぶ台本。`world.tick` を見て決められた時刻に介入する。
    script: fn(&mut World),
}

impl RegressionScenario {
    pub fn build(&self) -> World {
        (self.build)(self.seed)
    }
}

impl core::fmt::Debug for RegressionScenario {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RegressionScenario")
            .field("id", &self.id)
            .field("seed", &self.seed)
            .field("ticks", &self.ticks)
            .finish()
    }
}

pub const SCENARIOS: &[RegressionScenario] = &[
    RegressionScenario {
        id: "reserve_passes_melee",
        name_ja: "正面衝突の横を予備隊が通過する",
        watches_ja: "混戦のそばを通る部隊が、隊列を保ったまま通り抜けられるか",
        seed: 0x5EED_B001,
        ticks: 900,
        build: build_reserve_passes_melee,
        script: no_script,
    },
    RegressionScenario {
        id: "column_front_losses",
        name_ja: "縦隊の前列が損耗し後列が詰める",
        watches_ja: "穴の補充が局所で済むか、部隊全員が動き直さないか",
        seed: 0x5EED_B002,
        ticks: 700,
        build: build_column_front_losses,
        script: script_kill_front_rank,
    },
    RegressionScenario {
        id: "defile_then_reform",
        name_ja: "狭路を抜けて開けた場所で再整列する",
        watches_ja: "狭路での間隔の保ち方と、出口での隊列の戻り方",
        seed: 0x5EED_B003,
        ticks: 900,
        build: build_defile_then_reform,
        script: no_script,
    },
    RegressionScenario {
        id: "guard_area_feint",
        name_ja: "防衛地点へ接近する敵と、脇を通過する陽動",
        watches_ja: "守備兵が陽動につられて持ち場を離れないか",
        seed: 0x5EED_B004,
        ticks: 900,
        build: build_guard_area_feint,
        script: no_script,
    },
    RegressionScenario {
        id: "hunt_person_lost",
        name_ja: "追跡中の人物が倒れる",
        watches_ja: "対象を失った追跡隊が止まり続けないか",
        seed: 0x5EED_B005,
        ticks: 700,
        build: build_hunt_person_lost,
        script: script_kill_hunted_person,
    },
];

pub fn find(id: &str) -> Option<&'static RegressionScenario> {
    SCENARIOS.iter().find(|s| s.id == id)
}

/// 1 シナリオを回した結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegressionRun {
    pub id: &'static str,
    pub seed: u64,
    pub ticks: u32,
    pub metrics: MetricsReport,
    /// 最終 tick の状態ハッシュ。
    pub state_hash: u64,
    /// 全 tick の状態ハッシュを畳んだ値。途中の分岐もここで検出できる。
    pub trace_hash: u64,
}

impl RegressionRun {
    pub fn to_json(&self) -> String {
        format!(
            "{{\"id\":\"{}\",\"seed\":\"0x{:016X}\",\"ticks\":{},\
             \"state_hash\":\"0x{:016X}\",\"trace_hash\":\"0x{:016X}\",\"metrics\":{}}}",
            self.id,
            self.seed,
            self.ticks,
            self.state_hash,
            self.trace_hash,
            self.metrics.to_json()
        )
    }
}

/// 既定の tick 数と既定の観測設定で回す。
pub fn run(scenario: &RegressionScenario) -> RegressionRun {
    run_for(scenario, scenario.ticks, MetricsConfig::default())
}

/// tick 数と観測設定を指定して回す。テストは短く、CLI は既定の長さで回す。
pub fn run_for(scenario: &RegressionScenario, ticks: u32, cfg: MetricsConfig) -> RegressionRun {
    let mut world = scenario.build();
    let mut observer = BattlefieldObserver::new(cfg);
    let mut trace_hash = 0xcbf2_9ce4_8422_2325u64;
    for _ in 0..ticks {
        (scenario.script)(&mut world);
        world.tick();
        observer.observe(&world);
        trace_hash ^= world.state_hash();
        trace_hash = trace_hash.wrapping_mul(0x0100_0000_01b3);
    }
    RegressionRun {
        id: scenario.id,
        seed: scenario.seed,
        ticks,
        metrics: observer.report(),
        state_hash: world.state_hash(),
        trace_hash,
    }
}

// ---------------------------------------------------------------------------
// 共通の組み立て
// ---------------------------------------------------------------------------

fn flat_world(seed: u64, size_m: u32) -> World {
    World::with_terrain(seed, Terrain::flat(size_m / CELL_M, CELL_M, seed))
}

/// 1 部隊ぶんの配置指定。兵士は最初から自分の陣形スロットの上に立つので、
/// 「スロットからの距離」は 0 から始まる。
struct UnitSpec {
    /// 陣形アンカー（ランク 0 の中央）。
    origin: Vec2Fx,
    /// 何人か。
    count: u32,
    /// 何列（奥行き）か。file 数はここから決まる（`formation_goals` と同じ規則）。
    ranks: u32,
    spacing_mm: i32,
    faction: u8,
    unit_id: u16,
    /// ランクが伸びる向き。兵士はその反対、つまり敵の側を向く。
    formation_facing: Brad,
    formation: FormationId,
    /// 能力値生成のシード塩。部隊ごとに変えると同じ乱数列を共有しない。
    seed_salt: u32,
}

/// 指定どおりに兵士を並べ、指揮ノードを 1 つ作る。
fn spawn_unit(world: &mut World, spec: &UnitSpec) -> NodeId {
    let spacing = fx_from_mm(spec.spacing_mm);
    let ranks = spec.ranks.max(1);
    let files = spec.count.div_ceil(ranks).max(1);
    let (sin, cos) = (sin_fx(spec.formation_facing), cos_fx(spec.formation_facing));
    // 兵士は隊列の伸びる向きと逆——つまり進行方向側——を向く。
    let soldier_facing = spec
        .formation_facing
        .wrapping_add(sim_math::BRAD_QUARTER as Brad);
    let mut rng = Rng::stream(world.seed, spec.seed_salt, Purpose::Spawn, 0);
    let mut ids = Vec::with_capacity(spec.count as usize);
    for slot in 0..spec.count {
        let file = slot % files;
        let rank = slot / files;
        let local_x = (file as i32 * spacing) - ((files.saturating_sub(1) as i32 * spacing) / 2);
        let local_y = rank as i32 * spacing;
        let offset = Vec2Fx::new(
            fx_mul(local_x, cos) - fx_mul(local_y, sin),
            fx_mul(local_x, sin) + fx_mul(local_y, cos),
        );
        let attrs = Attrs::new(
            rng.attr(140, 20),
            rng.attr(130, 20),
            rng.attr(150, 22),
            rng.attr(140, 24),
            rng.attr(150, 22),
            rng.attr(140, 25),
            rng.attr(130, 25),
            rng.attr(145, 20),
            rng.attr(120, 35),
            rng.attr(125, 30),
            rng.attr(150, 25),
            rng.attr(140, 22),
        );
        ids.push(world.spawn(
            spec.origin.add(offset),
            soldier_facing,
            spec.unit_id,
            spec.faction,
            attrs,
            0,
        ));
    }
    let commander = ids[0];
    let deputies: Vec<SoldierId> = ids.iter().skip(1).take(2).copied().collect();
    let unit = Unit {
        soldiers: ids,
        troop_type: 0,
        formation: spec.formation,
        formation_origin: spec.origin,
        formation_facing: spec.formation_facing,
        ranks: ranks as u16,
        file_spacing: spacing,
        rank_spacing: spacing,
        banner: None,
        formation_change: None,
        path: Vec::new(),
        path_final: spec.origin,
        pursuit_leash: None,
    };
    world.add_command_node(None, 0, spec.faction, commander, deputies, Some(unit))
}

/// 組み立て時点で任務を与える。回帰シナリオは「命令が届くまでの遅延」ではなく
/// 「届いた後の個体挙動」を見るので、伝令を待たずに任務から始める。
fn order_move(world: &mut World, node: NodeId, to: Vec2Fx, facing: Brad, formation: FormationId) {
    world.set_objective(
        node,
        Intent::MoveTo {
            pos: to,
            facing,
            speed: MoveSpeed::Walk,
            formation,
        },
    );
}

fn no_script(_world: &mut World) {}

/// 台本が兵士を確実に倒す。通常のダメージ経路を通すので、死因統計・士気・
/// 死体の扱いは実戦と同じになる。
fn strike_down(world: &mut World, attacker: SoldierId, victim: SoldierId) {
    if !world.soldiers.is_active_id(victim) {
        return;
    }
    let tick = world.tick;
    world.combat.apply_impact_damage(
        attacker,
        victim,
        SCRIPTED_STRIKE_DAMAGE,
        &mut world.soldiers,
        tick,
    );
}

// ---------------------------------------------------------------------------
// 1. 正面衝突の横を予備隊が通過する
// ---------------------------------------------------------------------------

/// 会戦線は中央で噛み合い、その東側 40 m を予備隊が北へ通り抜ける。
fn build_reserve_passes_melee(seed: u64) -> World {
    let size_m = 400;
    let mut world = flat_world(seed, size_m);
    let mid = fx((size_m / 2) as i32);

    let south = spawn_unit(
        &mut world,
        &UnitSpec {
            origin: Vec2Fx::new(mid, mid - fx(10)),
            count: 96,
            ranks: 4,
            spacing_mm: 900,
            faction: 0,
            unit_id: 0,
            formation_facing: 0,
            formation: FORMATION_SHIELDWALL,
            seed_salt: 1,
        },
    );
    let north = spawn_unit(
        &mut world,
        &UnitSpec {
            origin: Vec2Fx::new(mid, mid + fx(10)),
            count: 96,
            ranks: 4,
            spacing_mm: 900,
            faction: 1,
            unit_id: 1,
            formation_facing: BRAD_HALF as Brad,
            formation: FORMATION_SHIELDWALL,
            seed_salt: 2,
        },
    );
    // 予備隊は混戦の東を、北へ抜ける。
    let reserve = spawn_unit(
        &mut world,
        &UnitSpec {
            origin: Vec2Fx::new(mid + fx(18), mid - fx(25)),
            count: 48,
            ranks: 6,
            spacing_mm: 900,
            faction: 0,
            unit_id: 2,
            formation_facing: 0,
            formation: FORMATION_COLUMN,
            seed_salt: 3,
        },
    );

    order_move(
        &mut world,
        south,
        Vec2Fx::new(mid, mid - fx(2)),
        0,
        FORMATION_SHIELDWALL,
    );
    order_move(
        &mut world,
        north,
        Vec2Fx::new(mid, mid + fx(2)),
        BRAD_HALF as Brad,
        FORMATION_SHIELDWALL,
    );
    order_move(
        &mut world,
        reserve,
        Vec2Fx::new(mid + fx(18), mid + fx(25)),
        0,
        FORMATION_COLUMN,
    );
    world
}

// ---------------------------------------------------------------------------
// 2. 縦隊の前列に損耗が生じ、後列が穴を埋める
// ---------------------------------------------------------------------------

/// 前進中の縦隊の最前列を、一定間隔で倒していく。敵の部隊は置かず、
/// 「穴の補充」だけを他の要因から切り離して観測する。
fn build_column_front_losses(seed: u64) -> World {
    let size_m = 400;
    let mut world = flat_world(seed, size_m);
    let mid = fx((size_m / 2) as i32);
    let column = spawn_unit(
        &mut world,
        &UnitSpec {
            origin: Vec2Fx::new(mid, mid - fx(60)),
            count: 120,
            ranks: 10,
            spacing_mm: 900,
            faction: 0,
            unit_id: 0,
            formation_facing: 0,
            formation: FORMATION_COLUMN,
            seed_salt: 1,
        },
    );
    // 損耗を与える役（矢の飛来に相当）。戦列には加えない。
    world.spawn(
        Vec2Fx::new(mid, mid + fx(120)),
        BRAD_HALF as Brad,
        1,
        1,
        Attrs::default(),
        0,
    );
    order_move(
        &mut world,
        column,
        Vec2Fx::new(mid, mid + fx(40)),
        0,
        FORMATION_COLUMN,
    );
    world
}

/// 40 tick（2 秒）ごとに、最前列の生存者を 2 人倒す。
fn script_kill_front_rank(world: &mut World) {
    const INTERVAL_TICKS: u32 = 40;
    const VICTIMS_PER_INTERVAL: usize = 2;
    const FIRST_TICK: u32 = 120;
    if world.tick < FIRST_TICK || world.tick % INTERVAL_TICKS != 0 {
        return;
    }
    let Some(unit) = world.command.node(0).and_then(|n| n.unit.as_ref()) else {
        return;
    };
    // 陣形スロットの並びで後ろにいるほど前列（`formation_goals` は rank が
    // 大きいほど進行方向側へ置く）。生存している最後尾のスロットから倒す。
    let victims: Vec<SoldierId> = unit
        .soldiers
        .iter()
        .rev()
        .copied()
        .filter(|&id| world.soldiers.is_active_id(id))
        .take(VICTIMS_PER_INTERVAL)
        .collect();
    let attacker = world.soldiers.len() as SoldierId - 1;
    for victim in victims {
        strike_down(world, attacker, victim);
    }
}

// ---------------------------------------------------------------------------
// 3. 狭路を通過してから開けた場所で再整列する
// ---------------------------------------------------------------------------

/// 東西いっぱいの水域に 12 m の隙間だけを開け、その先の開けた場所を目標にする。
fn build_defile_then_reform(seed: u64) -> World {
    let size_m = 400;
    let dim = size_m / CELL_M;
    let mut terrain = Terrain::flat(dim, CELL_M, seed);
    let wall_y = (size_m / 2) / CELL_M;
    let gap_center = dim / 2;
    let gap_half_cells = 3; // 片側 3 セル = 6 m、隙間は 12 m
    for y in wall_y..wall_y + 2 {
        for x in 0..dim {
            if x + gap_half_cells >= gap_center && x < gap_center + gap_half_cells {
                continue;
            }
            let idx = terrain.idx(x, y);
            terrain.set_water(idx, 400, WaterKind::Lake);
        }
    }
    terrain.recompute_derived();

    let mut world = World::with_terrain(seed, terrain);
    let mid = fx((size_m / 2) as i32);
    let column = spawn_unit(
        &mut world,
        &UnitSpec {
            origin: Vec2Fx::new(mid, mid - fx(70)),
            count: 96,
            ranks: 8,
            spacing_mm: 900,
            faction: 0,
            unit_id: 0,
            formation_facing: 0,
            formation: FORMATION_LINE,
            seed_salt: 1,
        },
    );
    order_move(
        &mut world,
        column,
        Vec2Fx::new(mid, mid + fx(60)),
        0,
        FORMATION_LINE,
    );
    world
}

// ---------------------------------------------------------------------------
// 4. 防衛地点へ敵が接近し、一部が陽動で通過する
// ---------------------------------------------------------------------------

/// 守備隊は地点を守り、敵の本隊は地点へ、陽動隊は迎撃範囲の外側を素通りする。
fn build_guard_area_feint(seed: u64) -> World {
    let size_m = 400;
    let mut world = flat_world(seed, size_m);
    let mid = fx((size_m / 2) as i32);
    let post = Vec2Fx::new(mid, mid);

    let guard = spawn_unit(
        &mut world,
        &UnitSpec {
            origin: post,
            count: 64,
            ranks: 8,
            spacing_mm: 900,
            faction: 0,
            unit_id: 0,
            formation_facing: BRAD_HALF as Brad,
            formation: FORMATION_LINE,
            seed_salt: 1,
        },
    );
    let assault = spawn_unit(
        &mut world,
        &UnitSpec {
            origin: Vec2Fx::new(mid, mid - fx(70)),
            count: 64,
            ranks: 8,
            spacing_mm: 900,
            faction: 1,
            unit_id: 1,
            formation_facing: 0,
            formation: FORMATION_LINE,
            seed_salt: 2,
        },
    );
    // 陽動隊は地点の西 60 m を、北へ通り過ぎる。迎撃範囲（20 m）の外なので、
    // 守備隊はつられて持ち場を離れてはならない。
    let feint = spawn_unit(
        &mut world,
        &UnitSpec {
            origin: Vec2Fx::new(mid - fx(60), mid - fx(70)),
            count: 32,
            ranks: 8,
            spacing_mm: 900,
            faction: 1,
            unit_id: 2,
            formation_facing: 0,
            formation: FORMATION_COLUMN,
            seed_salt: 3,
        },
    );

    world.set_objective(
        guard,
        Intent::GuardArea {
            center: post,
            radius_m: 12,
            intercept_radius_m: 20,
        },
    );
    // 本隊は地点そのものを取りに来る。守備側の防衛と、攻め手の占拠率が
    // 同じシナリオで測れる。
    world.set_objective(
        assault,
        Intent::OccupyArea {
            center: post,
            radius_m: 12,
        },
    );
    order_move(
        &mut world,
        feint,
        Vec2Fx::new(mid - fx(60), mid + fx(70)),
        0,
        FORMATION_COLUMN,
    );
    world
}

// ---------------------------------------------------------------------------
// 5. 追跡中の人物が倒れる
// ---------------------------------------------------------------------------

/// 追跡隊は敵の指揮官 1 人を追い、途中でその対象が倒れる。
fn build_hunt_person_lost(seed: u64) -> World {
    let size_m = 400;
    let mut world = flat_world(seed, size_m);
    let mid = fx((size_m / 2) as i32);

    let hunters = spawn_unit(
        &mut world,
        &UnitSpec {
            origin: Vec2Fx::new(mid, mid - fx(60)),
            count: 48,
            ranks: 6,
            spacing_mm: 900,
            faction: 0,
            unit_id: 0,
            formation_facing: 0,
            formation: FORMATION_LINE,
            seed_salt: 1,
        },
    );
    let escort = spawn_unit(
        &mut world,
        &UnitSpec {
            origin: Vec2Fx::new(mid, mid + fx(60)),
            count: 32,
            ranks: 4,
            spacing_mm: 900,
            faction: 1,
            unit_id: 1,
            formation_facing: BRAD_HALF as Brad,
            formation: FORMATION_LINE,
            seed_salt: 2,
        },
    );
    let target = hunted_person(&world).expect("護衛部隊に指揮官がいる");
    world.set_objective(hunters, Intent::HuntPerson { target });
    // 護衛は北へ退がりながら守る。追跡隊は動く対象を追うことになる。
    order_move(
        &mut world,
        escort,
        Vec2Fx::new(mid, mid + fx(120)),
        BRAD_HALF as Brad,
        FORMATION_LINE,
    );
    world
}

/// 追跡の対象（護衛部隊＝ノード 1 の指揮官）。
fn hunted_person(world: &World) -> Option<SoldierId> {
    world.command.node(1).map(|node| node.commander)
}

/// 決められた tick に、追跡対象を倒す。追跡隊はそこで目標を失う。
fn script_kill_hunted_person(world: &mut World) {
    const KILL_TICK: u32 = 300;
    if world.tick != KILL_TICK {
        return;
    }
    let Some(target) = hunted_person(world) else {
        return;
    };
    let attacker = world
        .command
        .node(0)
        .map(|node| node.commander)
        .unwrap_or(0);
    strike_down(world, attacker, target);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::MissionKind;

    /// テストは短めに回す。挙動の傾向を見るには十分で、`cargo test` が重くならない。
    const TEST_TICKS: u32 = 240;

    fn run_short(scenario: &RegressionScenario) -> RegressionRun {
        run_for(scenario, TEST_TICKS, MetricsConfig::default())
    }

    #[test]
    fn every_scenario_builds_and_runs() {
        for scenario in SCENARIOS {
            let run = run_short(scenario);
            assert!(run.metrics.alive > 0, "{} で全員が消えている", scenario.id);
            assert_eq!(run.metrics.ticks, TEST_TICKS, "{}", scenario.id);
            assert!(
                run.metrics.movement.starts > 0,
                "{} で誰も動き出していない",
                scenario.id
            );
        }
    }

    #[test]
    fn scenario_ids_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for scenario in SCENARIOS {
            assert!(seen.insert(scenario.id), "id が重複: {}", scenario.id);
            assert!(find(scenario.id).is_some());
        }
        assert!(find("そんなシナリオはない").is_none());
    }

    /// 同じ seed で 2 回回したら、指標も状態ハッシュも完全に一致すること。
    #[test]
    fn runs_are_reproducible() {
        for scenario in SCENARIOS {
            let a = run_short(scenario);
            let b = run_short(scenario);
            assert_eq!(a, b, "{} の実行が再現しない", scenario.id);
        }
    }

    /// 観測はシミュレーションを変更しない。観測ありと観測なしで、全 tick の
    /// 状態ハッシュが一致すること。
    #[test]
    fn observation_does_not_change_the_simulation() {
        for scenario in SCENARIOS {
            let mut observed = scenario.build();
            let mut plain = scenario.build();
            let mut observer = BattlefieldObserver::default();
            for _ in 0..TEST_TICKS {
                (scenario.script)(&mut observed);
                observed.tick();
                observer.observe(&observed);

                (scenario.script)(&mut plain);
                plain.tick();

                assert_eq!(
                    observed.state_hash(),
                    plain.state_hash(),
                    "{} の tick {} で観測がワールドを変えた",
                    scenario.id,
                    plain.tick
                );
            }
        }
    }

    /// 台本が意図した状況を実際に作れていること（シナリオが空回りしていない）。
    #[test]
    fn reserve_scenario_puts_soldiers_next_to_a_fight() {
        let run = run_for(
            find("reserve_passes_melee").unwrap(),
            400,
            MetricsConfig::default(),
        );
        assert!(
            run.metrics.vicinity.peak_idle_near_combat > 0,
            "戦闘の近くに非交戦兵がいない: {:?}",
            run.metrics.vicinity
        );
        assert!(run.metrics.formation.in_units > 0);
    }

    #[test]
    fn column_scenario_loses_front_rank_soldiers() {
        let scenario = find("column_front_losses").unwrap();
        let run = run_for(scenario, 400, MetricsConfig::default());
        assert!(
            run.metrics.alive < run.metrics.soldiers,
            "台本が誰も倒していない"
        );
        let column = run.metrics.mission[MissionKind::MoveTo as usize];
        assert!(column.soldiers > 0, "縦隊が任務を受けていない");
    }

    #[test]
    fn guard_scenario_assigns_both_a_defended_and_an_occupied_area() {
        let run = run_for(
            find("guard_area_feint").unwrap(),
            400,
            MetricsConfig::default(),
        );
        assert!(
            run.metrics.area.guard_assigned > 0,
            "地点防衛の任務が届いていない"
        );
        assert!(
            run.metrics.area.occupy_assigned > 0,
            "地点占拠の任務が届いていない"
        );
        assert!(
            run.metrics.mission[MissionKind::GuardArea as usize].soldiers > 0,
            "守備隊が任務別の集計に出ていない"
        );
    }

    #[test]
    fn hunt_scenario_ends_with_the_target_down() {
        let scenario = find("hunt_person_lost").unwrap();
        let mut world = scenario.build();
        let target = hunted_person(&world).unwrap();
        for _ in 0..400 {
            (scenario.script)(&mut world);
            world.tick();
        }
        assert!(
            !world.soldiers.is_active_id(target),
            "追跡対象が倒れていない"
        );
        // 対象を失った部隊は追跡を手放し、目的の無いまま固まらない（B1）。
        let node = world.command.node(0).unwrap();
        assert!(
            !matches!(node.objective, Some(Intent::HuntPerson { .. })),
            "追跡任務が残り続けている: {:?}",
            node.objective
        );
        assert!(node.objective.is_some(), "任務が空のまま放置されている");
        assert_eq!(
            node.hunt.map(|h| h.state),
            Some(crate::organization::HuntState::TargetDown)
        );
    }
}
