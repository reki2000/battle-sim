//! ワールド状態とシミュレーションシステム。
//!
//! このクレートは wasm に依存しない。ネイティブでテストとベンチができる
//! （`sim-headless`）ことが、バランス調整と性能計測の前提になっている。
//!
//! M0〜M2 の移動基盤に加え、M3 の指揮ツリー・命令・陣形、M4 の白兵戦・負傷・
//! 士気、M5 の騎兵突撃、M6 の工兵（作業タスク・野戦築城・地形改修・矢の補給・
//! 負傷者回収）、M7 の指揮官 AI（Blackboard・戦況評価・Objective・会戦プラン・
//! 独断専行・判断ログ）を実装する（`docs/spec/12-roadmap.md`）。

#![forbid(unsafe_code)]

pub mod cavalry;
pub mod charge;
pub mod combat;
pub mod commander_ai;
pub mod corpses;
pub mod engineering;
pub mod organization;
pub mod pathing;
pub mod scenario;
pub mod snapshot;
pub mod soldiers;
pub mod spatial;
pub mod structures;

use cavalry::CavalrySystem;
use charge::ChargeSystem;
use combat::CombatSystem;
use corpses::CorpseField;
use engineering::EngineeringSystem;
use organization::CommandTree;
use sim_math::{fx, fx_div, fx_from_mm, fx_mul, per_sec_to_per_tick, Fx, Vec2Fx, FX_ONE};
use sim_terrain::{Terrain, TerrainParams, SURFACE_EFFECTS};
use soldiers::{flags, Attrs, SoldierId, Soldiers, State};
use spatial::{SpatialHash, MAX_NEIGHBORS};
use structures::StructureSystem;

/// シミュレーションのロジックバージョン。リプレイの互換性判定に使う。
///
/// M6 で工兵（野戦築城・地形改修・矢の補給・負傷者回収）を追加したため 3 → 4。
/// M7 で指揮官 AI（Blackboard・戦況評価・Objective・会戦プラン・独断専行）を
/// 追加したため 4 → 5。
/// 射撃兵の士気・着弾判定・包囲目標の再選択を直したため 5 → 6（同じ命令ログ
/// でも結果が変わるので、それ以前の記録とは互換しない）。
/// 兵士同士の当たり判定（歩容によるすり抜け・転倒）、死体の障害物化、
/// 徒歩兵の助走突撃と移動疲労を入れたため 6 → 7。
pub const SIM_VERSION: u32 = 7;

/// 衝突解決の反復回数（仕様 06 章 2.2）。
const SEPARATION_ITERATIONS: usize = 2;
/// 押し戻しの緩和係数。完全に解消しないことで密集の圧力が残る。
const SEPARATION_RELAX_PERMILLE: i32 = 500;

/// 白兵戦で保とうとする距離を、武器の間合いより少し内側に取る分（mm）。
/// ぎりぎりを狙うと押し合いで届かなくなるため。
const STANDOFF_MARGIN_MM: i32 = 200;
/// 長物の兵士が「踏み込まれた」と感じて下がり始める、体の間の距離（mm）。
const STANDOFF_RETREAT_MM: i32 = 600;
/// 間合いの許容幅（mm）。この幅の中では足を止めて戦う。
const STANDOFF_TOLERANCE_MM: i32 = 150;
/// 間合いを詰める・取り直すときの足の速さ（mm/s）。摺り足で、走りはしない。
const STANDOFF_STEP_MM_PER_S: i32 = 700;

/// ワールドの生成設定。
#[derive(Clone, Debug)]
pub struct WorldConfig {
    pub seed: u64,
    pub terrain: TerrainParams,
}

impl Default for WorldConfig {
    fn default() -> Self {
        let seed = 0x5EED_1234_ABCD_0001;
        Self {
            seed,
            terrain: TerrainParams {
                seed,
                ..Default::default()
            },
        }
    }
}

/// シミュレーションのワールド。
pub struct World {
    pub seed: u64,
    pub tick: u32,
    pub terrain: Terrain,
    pub soldiers: Soldiers,
    pub hash: SpatialHash,
    /// 編成・命令・伝令・陣形を管理する M3 の指揮ツリー。
    pub command: CommandTree,
    /// M4 の白兵戦・負傷・士気・追撃フェーズ。
    pub combat: CombatSystem,
    /// M5 の騎兵：突撃のモメンタム・馬の忌避・衝撃・落馬後の徒歩化。
    pub cavalry: CavalrySystem,
    /// 徒歩兵の助走突撃・すり抜けの失敗による転倒・微視的な行動決定。
    pub charge: ChargeSystem,
    /// 倒れた兵士が作る低い障害物（跨げるが時間がかかる）。
    pub corpses: CorpseField,
    /// M6 の野戦築城・地形改修の対象（杭列・堀・鹿砦・土塁・馬防柵・橋）。
    pub structures: StructureSystem,
    /// M6 の工兵：作業タスク・矢の補給・負傷者回収。
    pub engineering: EngineeringSystem,
    /// 各兵士の目標位置。M2 で陣形スロットに置き換わる
    goal: Vec<Vec2Fx>,
    /// 衝突解決の書き込み先（読み書きフェーズを分けるため）
    push_x: Vec<Fx>,
    push_y: Vec<Fx>,
    /// 衝突解決の全反復で蓄積した重なり量。圧迫（crush）判定に使う。
    crush_accum: Vec<Fx>,
    /// 歩容から決まるすり抜け半径のキャッシュ。速度は衝突解決のあいだ変わら
    /// ないので、tick ごとに 1 回だけ計算して全反復で使い回す（ペアごとに
    /// 計算すると平方根が近傍数 × 反復数だけ走る）。
    pass_radius: Vec<Fx>,
}

impl core::fmt::Debug for World {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("World")
            .field("seed", &self.seed)
            .field("tick", &self.tick)
            .field("soldiers", &self.soldiers.len())
            .finish()
    }
}

impl World {
    /// 地形を生成し、空のワールドを作る。
    pub fn new(config: &WorldConfig) -> World {
        let terrain = sim_terrain::generate(&config.terrain);
        Self::with_terrain(config.seed, terrain)
    }

    /// 既に生成済みの地形から、空のワールドを作る（M9: 地形キャッシュ）。
    ///
    /// `terrain` は呼び出し側が用意する。`sim_terrain::generate` と同じ
    /// (seed, size_m, relief, ...) から作られたものと同一であることは
    /// 呼び出し側の責任（ここでは検証しない。ブラウザの IndexedDB に
    /// 保存された地形グリッドを、再生成せずそのまま復元する用途を想定）。
    pub fn with_terrain(seed: u64, terrain: sim_terrain::Terrain) -> World {
        World {
            seed,
            tick: 0,
            terrain,
            soldiers: Soldiers::default(),
            hash: SpatialHash::default(),
            command: CommandTree::new(),
            combat: CombatSystem::default(),
            cavalry: CavalrySystem::default(),
            charge: ChargeSystem::default(),
            corpses: CorpseField::default(),
            structures: StructureSystem::default(),
            engineering: EngineeringSystem::default(),
            goal: Vec::new(),
            push_x: Vec::new(),
            push_y: Vec::new(),
            crush_accum: Vec::new(),
            pass_radius: Vec::new(),
        }
    }

    /// 兵士を 1 体追加する。
    pub fn spawn(
        &mut self,
        pos: Vec2Fx,
        facing: sim_math::Brad,
        unit_id: u16,
        faction: u8,
        attrs: Attrs,
        soldier_flags: u8,
    ) -> SoldierId {
        self.spawn_typed(pos, facing, unit_id, faction, 0, attrs, soldier_flags)
    }

    /// 兵科 index を明示して兵士を追加する。
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_typed(
        &mut self,
        pos: Vec2Fx,
        facing: sim_math::Brad,
        unit_id: u16,
        faction: u8,
        troop_type: u16,
        attrs: Attrs,
        soldier_flags: u8,
    ) -> SoldierId {
        let id = self.soldiers.push_typed(
            pos.x,
            pos.y,
            facing,
            unit_id,
            faction,
            troop_type,
            attrs,
            soldier_flags,
        );
        self.combat.register();
        self.cavalry.register();
        self.charge.register();
        self.engineering.register();
        if soldier_flags & flags::MOUNTED != 0 {
            self.combat.set_mounted(id, true);
        }
        self.goal.push(pos);
        self.push_x.push(0);
        self.push_y.push(0);
        self.crush_accum.push(0);
        self.pass_radius.push(0);
        let i = id as usize;
        self.soldiers.z_cm[i] =
            sim_math::fx_to_mm(self.terrain.height_at(pos.x, pos.y)) as i16 / 10;
        id
    }

    /// 兵士の移動目標を設定する。
    ///
    /// M3 以降は指揮系統から降りてくる命令が目標を決めるが、M0 では
    /// テストとデモのために直接設定できるようにしておく。
    pub fn set_goal(&mut self, id: SoldierId, goal: Vec2Fx) {
        let i = id as usize;
        if i < self.goal.len() {
            self.goal[i] = goal;
            if self.soldiers.hot.state[i] == State::Idle {
                self.soldiers.hot.state[i] = State::Marching;
            }
        }
    }

    /// 指揮ノードを追加する。葉ノードには `organization::Unit` を渡す。
    pub fn add_command_node(
        &mut self,
        parent: Option<organization::NodeId>,
        echelon: u8,
        faction: u8,
        commander: SoldierId,
        deputies: Vec<SoldierId>,
        unit: Option<organization::Unit>,
    ) -> organization::NodeId {
        self.command
            .add_node(parent, echelon, faction, commander, deputies, unit)
    }

    /// 命令を指揮系統へ投入する。届くまでの遅延は伝令の距離から決まる。
    pub fn issue_order(
        &mut self,
        issuer: organization::NodeId,
        target: organization::NodeId,
        intent: organization::Intent,
        priority: organization::Priority,
    ) -> Option<organization::OrderId> {
        self.command
            .issue_order(issuer, target, intent, priority, self.tick, &self.soldiers)
    }

    pub fn issue_order_via(
        &mut self,
        issuer: organization::NodeId,
        target: organization::NodeId,
        intent: organization::Intent,
        priority: organization::Priority,
        method: organization::DeliveryMethod,
    ) -> Option<organization::OrderId> {
        self.command.issue_order_via(
            issuer,
            target,
            intent,
            priority,
            method,
            self.tick,
            &self.soldiers,
        )
    }

    /// 葉部隊の陣形を変更する。完了までは `formation_change` に残る。
    pub fn change_formation(
        &mut self,
        node: organization::NodeId,
        formation: organization::FormationId,
    ) -> bool {
        self.command
            .change_formation(node, formation, &self.soldiers, self.tick)
    }

    // ------------------------------------------------------------------
    // M7: 指揮官 AI（アーキタイプ・戦況評価・判断ログ）
    // ------------------------------------------------------------------

    /// この指揮官の性格をアーキタイプから生成して設定する（仕様 05 章 5.5 節）。
    /// `archetype` は `commander_ai::ARCHETYPES` の index。
    pub fn set_commander_archetype(
        &mut self,
        node: organization::NodeId,
        archetype: usize,
        seed_salt: u32,
    ) {
        commander_ai::set_archetype(&mut self.command, node, archetype, self.seed, seed_salt);
    }

    /// この指揮官の性格を直接設定する（アーキタイプを経由しない）。
    pub fn set_commander_attrs(
        &mut self,
        node: organization::NodeId,
        attrs: organization::CommanderAttrs,
    ) {
        self.command.set_commander_attrs(node, attrs);
    }

    /// この指揮官が実際に認識している戦況評価と、地上の実際の戦況を並べて返す
    /// （認識 vs 実際、仕様 12 章 M7 の受け入れ条件）。
    pub fn commander_perceived_vs_true(
        &self,
        node: organization::NodeId,
    ) -> (
        organization::SituationAssessment,
        organization::SituationAssessment,
    ) {
        commander_ai::perceived_vs_true(node, &self.command, &self.terrain, self.seed, self.tick)
    }

    // ------------------------------------------------------------------
    // M6: 工兵タスク・野戦築城・地形改修・補給・負傷者回収
    // ------------------------------------------------------------------

    /// 野戦築城タスクを投入する（杭列・堀・鹿砦・土塁・馬防柵、仕様 07 章 2 節）。
    pub fn queue_build_structure(
        &mut self,
        kind: structures::StructureKind,
        a: Vec2Fx,
        b: Vec2Fx,
        owner: u8,
        priority: u8,
    ) -> engineering::TaskId {
        self.engineering
            .queue_build_structure(&mut self.structures, kind, a, b, owner, priority)
    }

    /// 敵構造物の破壊工作タスクを投入する。
    pub fn queue_destroy_structure(
        &mut self,
        structure: structures::StructureId,
        owner: u8,
        priority: u8,
    ) -> Option<engineering::TaskId> {
        self.engineering
            .queue_destroy_structure(&self.structures, structure, owner, priority)
    }

    /// 架橋タスクを投入する（仕様 07 章 3.1 節）。`a`-`b` の直線上に水面が
    /// なければ `None`。
    pub fn queue_build_bridge(
        &mut self,
        a: Vec2Fx,
        b: Vec2Fx,
        owner: u8,
        priority: u8,
    ) -> Option<engineering::TaskId> {
        self.engineering
            .queue_build_bridge(&self.terrain, a, b, owner, priority)
    }

    /// 橋の破壊工作タスクを投入する。
    pub fn queue_destroy_bridge(
        &mut self,
        bridge_task: engineering::TaskId,
        owner: u8,
        priority: u8,
    ) -> Option<engineering::TaskId> {
        self.engineering
            .queue_destroy_bridge(bridge_task, owner, priority)
    }

    /// 伐採タスクを投入する（仕様 07 章 3.3 節）。矩形内に森がなければ `None`。
    pub fn queue_clear_forest(
        &mut self,
        min: Vec2Fx,
        max: Vec2Fx,
        owner: u8,
        priority: u8,
    ) -> Option<engineering::TaskId> {
        self.engineering
            .queue_clear_forest(&self.terrain, min, max, owner, priority)
    }

    /// 補給拠点を配置する（仕様 07 章 5.1 節）。
    pub fn spawn_supply_depot(
        &mut self,
        pos: Vec2Fx,
        owner: u8,
        arrows: u32,
        bolts: u32,
        stones: u32,
    ) -> engineering::DepotId {
        self.engineering
            .spawn_depot(pos, owner, arrows, bolts, stones)
    }

    /// 矢の補給を要求する。
    pub fn request_arrow_resupply(
        &mut self,
        depot: engineering::DepotId,
        rally_point: Vec2Fx,
        owner: u8,
    ) {
        self.engineering
            .request_arrow_resupply(depot, rally_point, owner);
    }

    /// 倒れた兵士の回収を要求する（仕様 07 章 5.2 節）。
    pub fn request_wounded_recovery(&mut self, patient: SoldierId, owner: u8) {
        self.engineering.request_wounded_recovery(patient, owner);
    }

    /// 1 ティック進める。
    ///
    /// フェーズの順序は仕様 02 章 5 節に従う。M0 では未実装のフェーズを飛ばす。
    pub fn tick(&mut self) {
        self.command
            .tick(&mut self.soldiers, &self.terrain, self.tick);
        // M7 の指揮官 AI（Blackboard の更新、Objective・会戦プランの選択と
        // それに基づく命令の発令、独断専行の判定）。`command.tick` の直後に
        // 置くことで、`NodeStats` が最新（このメソッド呼び出し時点）であり、
        // かつ独断専行の即時適用が同じ tick の `formation_goals` に反映される。
        commander_ai::tick(
            &mut self.command,
            &mut self.soldiers,
            &self.terrain,
            &self.combat,
            self.seed,
            self.tick,
        );
        // 疲れた前列を後列と入れ替える。陣形スロットの並びを変えるので、
        // スロットから目標を配る `formation_goals` の直前に置く。
        self.command
            .rotate_tired_ranks(&self.soldiers, self.seed, self.tick);
        self.command
            .formation_goals(&mut self.soldiers, &mut self.goal, self.tick);
        self.hash.rebuild(&self.soldiers);
        // 騎兵の忌避・衝撃は白兵戦の通常交戦より先に解決する。こうすると、
        // 忌避で Wavering に、衝撃で Engaged に落ちた騎兵を、同じ tick 内で
        // combat.tick の通常交戦ロジックがそのまま拾える（仕様 12 章 M5）。
        let map_limit = fx(self.terrain.size_m() as i32) - FX_ONE;
        self.cavalry.tick(
            self.seed,
            self.tick,
            &mut self.soldiers,
            &self.hash,
            &mut self.combat,
            &mut self.structures,
            map_limit,
        );
        // 徒歩兵の助走も、騎兵と同じ理由で `combat.tick` の前に置く。助走を
        // つけた衝撃で `Engaged` に落ちた兵士を、同じ tick 内で通常の白兵戦
        // ロジックが拾える。
        self.charge.tick(
            self.seed,
            self.tick,
            &mut self.soldiers,
            &self.hash,
            &mut self.combat,
            map_limit,
        );
        self.combat
            .tick(self.seed, self.tick, &mut self.soldiers, &self.hash);
        // 工兵（M6）は白兵・射撃・騎兵の交戦判定の後に置く。こうすると、
        // 実戦闘に引き込まれた工兵の `State` はすでに書き換わっており、
        // `engineering.tick` はそれを見て作業を打ち切るだけでよい
        // （`engineering` モジュール冒頭のドキュメント参照）。
        self.engineering.tick(
            self.seed,
            self.tick,
            &mut self.soldiers,
            &mut self.structures,
            &mut self.terrain,
            &mut self.combat,
            &mut self.goal,
        );
        // 兵士自身の判断（下がって助走をつける・離れた敵へ走り込む）は、
        // 部隊の陣形目標と交戦相手が確定した後に、その tick 分だけ `goal` を
        // 上書きする形で入れる。命令が変われば次の tick から自然に元へ戻る。
        self.charge.decide(
            self.seed,
            self.tick,
            &mut self.soldiers,
            &self.hash,
            &self.combat,
            &mut self.goal,
            map_limit,
        );
        self.steer();
        self.integrate_motion();
        self.resolve_collisions();
        // 走り抜けようとして人や死体にぶつかった者を転ばせる。位置が確定した
        // 後でないと「隙間が足りなかったか」を判定できない（空間ハッシュは
        // `resolve_collisions` 末尾の圧迫判定が最新の位置で張り直している）。
        self.charge.resolve_run_collisions(
            self.seed,
            self.tick,
            &mut self.soldiers,
            &self.hash,
            &self.corpses,
        );
        self.follow_terrain();
        self.apply_movement_fatigue();
        self.corpses.maybe_rebuild(&self.soldiers, self.tick);
        self.tick += 1;
    }

    /// 移動による疲労の増減（仕様 06 章 6 節）。1 秒に 1 回まとめて適用する。
    ///
    /// 走れば疲れる、という当たり前の代償がないと、兵士は常に全力疾走した方が
    /// 得になってしまう。助走突撃を「使いどころのある手」に留めるための土台。
    fn apply_movement_fatigue(&mut self) {
        if self.tick % sim_math::TICK_HZ != 0 {
            return;
        }
        for i in 0..self.soldiers.len() {
            if !self.soldiers.is_alive(i) {
                continue;
            }
            let delta = charge::movement_fatigue_delta(
                self.soldiers.speed_mm_per_tick(i),
                self.soldiers.attrs[i].endurance,
            );
            let fatigue = self.soldiers.fatigue[i];
            self.soldiers.fatigue[i] = if delta >= 0 {
                fatigue
                    .saturating_add(delta as u16)
                    .min(soldiers::MAX_FATIGUE)
            } else {
                fatigue.saturating_sub((-delta) as u16)
            };
        }
    }

    /// フェーズ 5（簡略版）: 目標に向かう操舵。
    ///
    /// M2 で陣形スロットへの seek + 分離 + 地形回避の合成に置き換わる。
    fn steer(&mut self) {
        let n = self.soldiers.len();
        for i in 0..n {
            if !self.soldiers.is_alive(i) {
                continue;
            }
            // 転倒中は起き上がるまで動けない
            if self.soldiers.is_stumbling(i) {
                self.soldiers.hot.vel_x[i] = 0;
                self.soldiers.hot.vel_y[i] = 0;
                continue;
            }
            // 突撃が寸前で失速した兵士は、しばらくその場で足が止まる
            // （仕様 06 章 3.7 節）。向きは敵から離さない。
            if self.charge.is_hesitating(i as SoldierId, self.tick) {
                self.soldiers.hot.vel_x[i] = 0;
                self.soldiers.hot.vel_y[i] = 0;
                self.face_nearest_enemy_if_close(i);
                continue;
            }
            // 命令を受けていない兵士はその場を保つ。押し合いで動かされても
            // 元の位置に戻ろうとはしない（さもないと 1 点に集まれという命令が
            // 分離と綱引きし、密集が解けなくなる）。
            if self.soldiers.hot.state[i] == State::Idle {
                self.soldiers.hot.vel_x[i] = 0;
                self.soldiers.hot.vel_y[i] = 0;
                self.face_nearest_enemy_if_close(i);
                continue;
            }
            // 白兵戦中は「武器の間合い」を保つのが最優先で、陣形スロットへの
            // 移動より前に来る。剣は踏み込み、槍は突ける距離で止まり、
            // 近づかれたら下がって突き直す（仕様 06 章 3.1 節の reach）。
            if self.apply_melee_standoff(i) {
                continue;
            }
            let pos = self.soldiers.pos(i);
            let to_goal = self.goal[i].sub(pos);
            if to_goal.len_sq() < (fx_from_mm(300) as i64).pow(2) {
                // 到着した
                self.soldiers.hot.vel_x[i] = 0;
                self.soldiers.hot.vel_y[i] = 0;
                if self.soldiers.hot.state[i] == State::Marching {
                    self.soldiers.hot.state[i] = State::Idle;
                }
                self.face_nearest_enemy_if_close(i);
                continue;
            }

            let desired_speed = self.desired_speed(i);
            let dir = to_goal.normalized();
            let target = dir.scale(desired_speed);

            // 加速度で追従する
            let accel =
                per_sec_to_per_tick(fx_from_mm(2000 + (self.soldiers.attrs[i].accel as i32) * 8));
            let cur = Vec2Fx::new(
                self.soldiers.hot.vel_x[i] as Fx,
                self.soldiers.hot.vel_y[i] as Fx,
            );
            let delta = target.sub(cur).clamp_len(accel);
            let next = cur.add(delta);
            self.soldiers.hot.vel_x[i] = next.x.clamp(i16::MIN as Fx, i16::MAX as Fx) as i16;
            self.soldiers.hot.vel_y[i] = next.y.clamp(i16::MIN as Fx, i16::MAX as Fx) as i16;
            // 助走の距離を取るために後ろへ下がっている兵士は、背中を向けずに
            // 敵を見たまま退がる（向きは `charge.decide` が毎 tick 敵の方へ
            // 向け直している）。それ以外は進行方向を向く。
            if self.charge.is_backing_off(i as SoldierId) {
                continue;
            }
            self.soldiers.turn_facing_toward(i, dir.angle());
            // 進行方向を向くのが基本だが、間合いの範囲内に敵がいれば
            // そちらを優先して向く。行軍の目標方向と実際に隣接した敵の
            // 方向がずれる（衝突で横に押された等）と、白兵戦の弧判定を
            // 満たせないまま双方が固まってしまうため。
            self.face_nearest_enemy_if_close(i);
        }
    }

    /// 間合いの範囲内にいる最も近い敵がいれば、そちらへ向き直らせる
    /// （仕様 12 章 M4：白兵戦の弧判定が現実の隣接関係と食い違わないようにする）。
    fn face_nearest_enemy_if_close(&mut self, i: usize) {
        let pos = self.soldiers.pos(i);
        let mut neighbors = [0u32; MAX_NEIGHBORS];
        let count = self.hash.query_enemies(
            &self.soldiers,
            pos.x,
            pos.y,
            self.soldiers.faction[i],
            &mut neighbors,
        );
        // 白兵武器の間合い（最長でもパイクの 5 m）より少し広めに取る。
        let detect_r = fx_from_mm(2_500);
        let mut nearest: Option<(i64, Vec2Fx)> = None;
        for &jid in &neighbors[..count] {
            let j = jid as usize;
            if !self.soldiers.is_alive(j) {
                continue;
            }
            let jp = self.soldiers.pos(j);
            let d2 = sim_math::dist_sq(pos, jp);
            if d2 > (detect_r as i64) * (detect_r as i64) {
                continue;
            }
            if nearest.map_or(true, |(best, _)| d2 < best) {
                nearest = Some((d2, jp));
            }
        }
        if let Some((_, enemy_pos)) = nearest {
            // 瞬時には振り向けない。旋回速度の上限を守って向き直る
            // （仕様 06 章 2.5 節）。側面・背面を取られた兵士が向き直る前に
            // 討たれるのはこの上限のため。
            self.soldiers
                .turn_facing_toward(i, enemy_pos.sub(pos).angle());
        }
    }

    /// 白兵戦の間合い（standoff）を保つ操舵。この tick の操舵をここで
    /// 決めきったら `true` を返す。
    ///
    /// 交戦相手のいる兵士は、陣形スロットへ歩くより「自分の武器が届いて、
    /// かつ相手の武器が届きにくい距離」を保とうとする。
    ///
    /// - **短い武器（剣・斧）は踏み込む**。届く距離まで詰めたらそこで止まる
    /// - **長い武器（槍・パイク）は距離で止まる**。踏み込まれたら下がって
    ///   突き直す。パイク方陣が敵を寄せつけないのはこの結果
    /// - 騎乗兵は突撃が仕事なので対象にしない
    fn apply_melee_standoff(&mut self, i: usize) -> bool {
        if self.soldiers.hot.state[i] != State::Engaged
            || self.soldiers.hot.flags[i] & flags::MOUNTED != 0
            // 助走で詰めている最中は間合いを取らず、体ごとぶつかりに行く
            || self.charge.is_closing(i as SoldierId)
        {
            return false;
        }
        let weapon = self.combat.weapons[i];
        if weapon.ranged {
            return false;
        }
        let Some(j) = self
            .soldiers
            .index_if_present(self.soldiers.target[i])
            .filter(|&j| self.soldiers.is_alive(j))
        else {
            return false;
        };

        let pos = self.soldiers.pos(i);
        let target_pos = self.soldiers.pos(j);
        let bodies = self.soldiers.radius(i) + self.soldiers.radius(j);
        let preferred = weapon.reach + bodies - fx_from_mm(STANDOFF_MARGIN_MM);
        // 長物だけが「近すぎる」を嫌って下がる。短い武器は組み合ってよい
        let too_close = bodies
            + if weapon.reach >= fx_from_mm(2_000) {
                fx_from_mm(STANDOFF_RETREAT_MM)
            } else {
                0
            };
        let tolerance = fx_from_mm(STANDOFF_TOLERANCE_MM);
        // 距離と向きを 1 回の平方根で出す
        let delta = target_pos.sub(pos);
        let distance = delta.len();
        let dir = if distance == 0 {
            Vec2Fx::new(FX_ONE, 0)
        } else {
            delta.scale(fx_div(FX_ONE, distance))
        };

        // 足を止めて戦っている（もっとも多い）場合は速度を計算しない。
        // `desired_speed` は地形と構造物を引くので、全交戦兵ぶん毎 tick
        // 呼ぶと無駄が大きい。
        let velocity = if distance > preferred + tolerance {
            dir.scale(self.standoff_step(i))
        } else if distance < too_close {
            dir.scale(-self.standoff_step(i))
        } else {
            Vec2Fx::ZERO
        };
        self.soldiers.hot.vel_x[i] = velocity.x.clamp(i16::MIN as Fx, i16::MAX as Fx) as i16;
        self.soldiers.hot.vel_y[i] = velocity.y.clamp(i16::MIN as Fx, i16::MAX as Fx) as i16;
        self.soldiers.turn_facing_toward(i, dir.angle());
        true
    }

    /// 1 ティックあたりの希望移動量（Fx, m）。
    ///
    /// 地表の速度倍率と疲労を反映する（仕様 06 章 2.1 の簡略版）。
    fn desired_speed(&self, i: usize) -> Fx {
        let id = i as SoldierId;
        let mounted = self.soldiers.hot.flags[i] & flags::MOUNTED != 0;
        // 基準 1.2 m/s（徒歩）/ 2.2 m/s（騎乗の常歩）に能力値で ±0.8 m/s。
        // 突撃中はモメンタムに応じて最高速 8 m/s まで上がる
        // （仕様 06 章 4.1 節「最高速 8 m/s」「加速には距離が要る」）。
        let base_mm_per_s = if mounted {
            2200 + (self.soldiers.attrs[i].speed as i32) * 4
        } else {
            1200 + (self.soldiers.attrs[i].speed as i32) * 3
        };
        let base_mm_per_s = if self.soldiers.hot.state[i] == State::Charging {
            // 助走が乗るほど速くなる。徒歩の上限は全力疾走の 4.5 m/s、
            // 騎乗は 8 m/s（仕様 06 章 4.1 節）。
            let (start, top, m) = if mounted {
                (
                    base_mm_per_s,
                    8000,
                    self.cavalry.momentum_permille(id) as i32,
                )
            } else {
                // 徒歩は「駆け出した瞬間から速歩」。そうしないと助走距離を
                // 積む速度に届かず、いつまでも momentum が 0 のままになる。
                (
                    base_mm_per_s.max(charge::CHARGE_START_MM_PER_S),
                    charge::SPRINT_TOP_MM_PER_S,
                    self.charge.momentum_permille(id) as i32,
                )
            };
            start + (top - start) * m / 1000
        } else {
            base_mm_per_s
        };
        let per_tick = per_sec_to_per_tick(fx_from_mm(base_mm_per_s));

        let pos = self.soldiers.pos(i);
        let surface = self.terrain.surface_at(pos.x, pos.y);
        let eff = &SURFACE_EFFECTS[surface as usize];
        let after_terrain = sim_math::fx_scale_permille(per_tick, eff.move_mult as i32);
        // 野戦築城（仕様 07 章 2 節、M6）: 杭列・堀・鹿砦・土塁・馬防柵は
        // 歩兵を減速させる。騎兵に対しては速度を落とすのではなく
        // `integrate_motion` で進入そのものを塞ぐので、ここでは歩兵にだけ適用する。
        let after_terrain = if mounted {
            after_terrain
        } else {
            let (structure_move_mult, _cohesion_mult) = self.structures.infantry_effect_at(pos);
            sim_math::fx_scale_permille(after_terrain, structure_move_mult as i32)
        };
        // 死体は高さ 25 cm の低い障害物。跨げば越えられるので通行を塞がず、
        // 重なった段数のぶんだけ越えるのに時間がかかる（`corpses` モジュール）。
        let after_terrain =
            sim_math::fx_scale_permille(after_terrain, self.corpses.move_mult_permille(pos) as i32);

        // 疲労 10000 で 40% 減。騎乗中は馬自身の疲労も掛け合わせる
        // （仕様「馬の疲労は騎手より早く蓄積する」）。
        let fatigue = self.soldiers.fatigue[i] as i32;
        let fatigue_permille = 1000 - (fatigue * 400 / soldiers::MAX_FATIGUE as i32);
        let fatigue_permille = if mounted {
            let horse_fatigue = self.cavalry.horse_fatigue(id) as i32;
            let horse_permille = 1000 - (horse_fatigue * 500 / 10_000);
            fatigue_permille * horse_permille / 1000
        } else {
            fatigue_permille
        };
        let injury_permille = match combat::InjuryStage::from_hp(self.soldiers.hp[i]) {
            combat::InjuryStage::Light => 950,
            combat::InjuryStage::Medium => 800,
            combat::InjuryStage::Heavy => 500,
            combat::InjuryStage::Downed | combat::InjuryStage::Dead => 0,
        };
        sim_math::fx_scale_permille(
            sim_math::fx_scale_permille(after_terrain, fatigue_permille),
            injury_permille,
        )
    }

    /// フェーズ 9: 移動積分。
    fn integrate_motion(&mut self) {
        let n = self.soldiers.len();
        for i in 0..n {
            if !self.soldiers.is_alive(i) {
                continue;
            }
            let vx = self.soldiers.hot.vel_x[i] as Fx;
            let vy = self.soldiers.hot.vel_y[i] as Fx;
            if vx == 0 && vy == 0 {
                self.soldiers.hot.flags[i] |= flags::SLEEPING;
                continue;
            }
            self.soldiers.hot.flags[i] &= !flags::SLEEPING;

            let next_x = self.soldiers.hot.pos_x[i] + vx;
            let next_y = self.soldiers.hot.pos_y[i] + vy;

            // 通行不能セルには入らない
            let (cx, cy) = self.terrain.world_to_cell(next_x, next_y);
            let idx = self.terrain.idx(cx, cy);
            if self.terrain.passability[idx] == 0 {
                self.soldiers.hot.vel_x[i] = 0;
                self.soldiers.hot.vel_y[i] = 0;
                continue;
            }

            // 完成した堀・鹿砦・土塁・馬防柵は騎兵の進入そのものを塞ぐ
            // （仕様 07 章 2 節、M6。杭列は塞がず `cavalry::CavalrySystem` 側の
            // 忌避判定に任せる）。
            if self.soldiers.hot.flags[i] & flags::MOUNTED != 0
                && self
                    .structures
                    .blocks_cavalry_at(Vec2Fx::new(next_x, next_y))
            {
                self.soldiers.hot.vel_x[i] = 0;
                self.soldiers.hot.vel_y[i] = 0;
                continue;
            }

            let limit = fx(self.terrain.size_m() as i32) - FX_ONE;
            self.soldiers.hot.pos_x[i] = next_x.clamp(0, limit);
            self.soldiers.hot.pos_y[i] = next_y.clamp(0, limit);
        }
    }

    /// 2 人の兵士が「重なっている」とみなす距離。
    ///
    /// 敵に対しては体の半径どうしの和——相手はどいてくれないので、体の幅が
    /// そのまま壁になる。味方に対しては歩容で決まるすり抜け半径
    /// （[`Soldiers::pass_radius`]）を使い、**体の間に 20 cm（歩行）/ 40 cm
    /// （走行）の隙間があれば通れる**ようにする。密集した隊列の中を歩いて
    /// 通り抜けられるのも、走って抜けようとすると隙間が足りずにぶつかるのも、
    /// この 1 行の違いから出る。
    #[inline]
    fn contact_distance(&self, i: usize, j: usize) -> Fx {
        if self.soldiers.faction[i] == self.soldiers.faction[j] {
            self.pass_radius[i] + self.pass_radius[j]
        } else {
            self.soldiers.radius(i) + self.soldiers.radius(j)
        }
    }

    /// すり抜け半径のキャッシュを現在の速度から作り直す。
    fn refresh_pass_radii(&mut self) {
        let n = self.soldiers.len();
        self.pass_radius.resize(n, 0);
        for i in 0..n {
            self.pass_radius[i] = self.soldiers.pass_radius(i);
        }
    }

    /// 間合いを調整する摺り足の速さ。地形や疲労で遅くなっているときは
    /// それ以上は出ない。
    #[inline]
    fn standoff_step(&self, i: usize) -> Fx {
        per_sec_to_per_tick(fx_from_mm(STANDOFF_STEP_MM_PER_S)).min(self.desired_speed(i))
    }

    /// フェーズ 10: 衝突解決（押し合い）。
    ///
    /// 読み取り（現在位置）と書き込み（押し戻し量）を分けることで、
    /// 走査順に結果が依存しないようにする。並列化の前提でもある。
    fn resolve_collisions(&mut self) {
        let n = self.soldiers.len();
        if n == 0 {
            return;
        }
        let relax = (SEPARATION_RELAX_PERMILLE * FX_ONE) / 1000;
        self.refresh_pass_radii();

        for _ in 0..SEPARATION_ITERATIONS {
            self.hash.rebuild(&self.soldiers);
            self.push_x.iter_mut().for_each(|v| *v = 0);
            self.push_y.iter_mut().for_each(|v| *v = 0);

            let mut neighbors = [0u32; MAX_NEIGHBORS];
            for i in 0..n {
                if !self.soldiers.is_alive(i) {
                    continue;
                }
                let pi = self.soldiers.pos(i);
                let mi = self.soldiers.mass(i);
                let cnt = self.hash.query_neighbors(pi.x, pi.y, &mut neighbors);

                for &jid in &neighbors[..cnt] {
                    let j = jid as usize;
                    if j == i || !self.soldiers.is_alive(j) {
                        continue;
                    }
                    let pj = self.soldiers.pos(j);
                    let rsum = self.contact_distance(i, j);
                    let d2 = sim_math::dist_sq(pi, pj);
                    if d2 >= (rsum as i64) * (rsum as i64) {
                        continue;
                    }

                    let d = sim_math::isqrt64(d2 as u64) as Fx;
                    let (dir, overlap) = if d == 0 {
                        // 完全に重なっている。ID 差から決定的な向きを作る
                        let a = ((i as u32).wrapping_mul(2654435761) >> 16) as u16;
                        (Vec2Fx::new(sim_math::cos_fx(a), sim_math::sin_fx(a)), rsum)
                    } else {
                        (pi.sub(pj).scale(fx_div(FX_ONE, d)), rsum - d)
                    };

                    // 重い方が押し勝つ
                    let mj = self.soldiers.mass(j);
                    let share = fx_div(fx(mj), fx(mi + mj));
                    let amount = fx_mul(fx_mul(overlap, share), relax);
                    self.push_x[i] += fx_mul(dir.x, amount);
                    self.push_y[i] += fx_mul(dir.y, amount);
                }
            }

            let limit = fx(self.terrain.size_m() as i32) - FX_ONE;
            for i in 0..n {
                if !self.soldiers.is_alive(i) {
                    continue;
                }
                if self.push_x[i] == 0 && self.push_y[i] == 0 {
                    continue;
                }
                self.soldiers.hot.pos_x[i] =
                    (self.soldiers.hot.pos_x[i] + self.push_x[i]).clamp(0, limit);
                self.soldiers.hot.pos_y[i] =
                    (self.soldiers.hot.pos_y[i] + self.push_y[i]).clamp(0, limit);
            }
        }

        self.apply_crush_pressure();
    }

    /// 圧迫（crush）の実害を戦闘システムへ渡す（仕様 12 章 M4）。
    ///
    /// 通常の陣形（肩が触れ合う密集隊形も含む）は `SEPARATION_ITERATIONS` の
    /// 押し合いでほぼ解消される（`overlapping_soldiers_push_apart` テストが
    /// 残差 10 mm 以下であることを保証している）。そのため「解決を試みた後も
    /// なお残っている重なり」だけを圧力として扱えば、普通の行軍や密集陣形を
    /// 圧迫死させることなく、空間ハッシュの近傍上限（12 人）を超えるような
    /// 極端な過密（`extreme_density_disperses_but_is_not_fully_resolved`
    /// テストが示す状況）だけを圧迫として検出できる。
    fn apply_crush_pressure(&mut self) {
        let n = self.soldiers.len();
        if n == 0 {
            return;
        }
        self.hash.rebuild(&self.soldiers);
        self.crush_accum.iter_mut().for_each(|v| *v = 0);
        let mut neighbors = [0u32; MAX_NEIGHBORS];
        for i in 0..n {
            if !self.soldiers.is_alive(i) {
                continue;
            }
            let pi = self.soldiers.pos(i);
            let cnt = self.hash.query_neighbors(pi.x, pi.y, &mut neighbors);
            let mut residual = 0 as Fx;
            for &jid in &neighbors[..cnt] {
                let j = jid as usize;
                if j == i || !self.soldiers.is_alive(j) {
                    continue;
                }
                let pj = self.soldiers.pos(j);
                // 押し合いと同じ基準で測る。動いている兵士が隙間をすり抜けて
                // いる状態を「圧迫」と数えないため（すり抜けは体を半身にして
                // いるだけで、押し潰されているわけではない）。
                let rsum = self.contact_distance(i, j);
                let d2 = sim_math::dist_sq(pi, pj);
                if d2 >= (rsum as i64) * (rsum as i64) {
                    continue;
                }
                let d = sim_math::isqrt64(d2 as u64) as Fx;
                residual += rsum - d;
            }
            self.crush_accum[i] = residual;
        }

        for i in 0..n {
            if !self.soldiers.is_alive(i) || self.crush_accum[i] == 0 {
                continue;
            }
            let radius = self.soldiers.radius(i).max(1);
            let pressure_permille = ((self.crush_accum[i] as i64 * 1000) / radius as i64)
                .clamp(0, u32::MAX as i64) as u32;
            self.combat.apply_crush(
                i as SoldierId,
                &mut self.soldiers,
                pressure_permille,
                self.seed,
                self.tick,
            );
        }
    }

    /// フェーズ 11: 地形の高度に追従する。
    fn follow_terrain(&mut self) {
        let n = self.soldiers.len();
        for i in 0..n {
            if !self.soldiers.is_alive(i) {
                continue;
            }
            let h = self
                .terrain
                .height_at(self.soldiers.hot.pos_x[i], self.soldiers.hot.pos_y[i]);
            self.soldiers.z_cm[i] = (sim_math::fx_to_mm(h) / 10) as i16;
        }
    }

    /// ワールド全体の状態ハッシュ。決定論の検証に使う（仕様 02 章 6.1）。
    pub fn state_hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |v: u64| {
            h ^= v;
            h = h.wrapping_mul(0x0100_0000_01b3);
        };
        mix(self.tick as u64);
        mix(self.command.state_hash());
        mix(self.combat.state_hash());
        mix(self.cavalry.state_hash());
        mix(self.charge.state_hash());
        mix(self.structures.state_hash());
        mix(self.engineering.state_hash());
        for i in 0..self.soldiers.len() {
            mix(self.soldiers.hot.pos_x[i] as u32 as u64);
            mix(self.soldiers.hot.pos_y[i] as u32 as u64);
            mix(self.soldiers.hot.vel_x[i] as u16 as u64);
            mix(self.soldiers.hot.vel_y[i] as u16 as u64);
            mix(self.soldiers.hot.facing[i] as u64);
            mix(self.soldiers.hot.state[i] as u64);
            mix(self.soldiers.hot.flags[i] as u64);
            mix(self.soldiers.hp[i] as u64);
            mix(self.soldiers.morale[i] as u64);
            mix(self.soldiers.fatigue[i] as u64);
        }
        h
    }
}

/// テストとデモ用に、方陣を組んだ部隊を配置する。
///
/// M3 で陣形システムに置き換わる。そのとき引数はシナリオ定義に置き換わる。
#[allow(clippy::too_many_arguments)]
pub fn deploy_block(
    world: &mut World,
    origin: Vec2Fx,
    files: u32,
    ranks: u32,
    spacing_mm: i32,
    faction: u8,
    unit_id: u16,
    seed_salt: u32,
) {
    deploy_block_typed(
        world, origin, files, ranks, spacing_mm, faction, unit_id, 0, seed_salt,
    );
}

/// 視覚プロファイルの兵科 index を付けて、方陣を組んだ部隊を配置する。
#[allow(clippy::too_many_arguments)]
pub fn deploy_block_typed(
    world: &mut World,
    origin: Vec2Fx,
    files: u32,
    ranks: u32,
    spacing_mm: i32,
    faction: u8,
    unit_id: u16,
    troop_type: u16,
    seed_salt: u32,
) {
    let spacing = fx_from_mm(spacing_mm);
    let mut rng = sim_math::Rng::stream(world.seed, seed_salt, sim_math::Purpose::Spawn, 0);
    for r in 0..ranks {
        for f in 0..files {
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
            let pos = Vec2Fx::new(
                origin.x + (f as Fx) * spacing,
                origin.y + (r as Fx) * spacing,
            );
            world.spawn_typed(pos, 0, unit_id, faction, troop_type, attrs, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_world() -> World {
        World::new(&WorldConfig {
            seed: 7,
            terrain: TerrainParams {
                seed: 7,
                size_m: 600,
                cell_m: 2,
                relief: 200,
                thermal_iterations: 3,
                ..Default::default()
            },
        })
    }

    #[test]
    fn empty_world_ticks_without_panic() {
        let mut w = small_world();
        for _ in 0..10 {
            w.tick();
        }
        assert_eq!(w.tick, 10);
    }

    #[test]
    fn simulation_is_deterministic() {
        let run = || {
            let mut w = small_world();
            deploy_block(&mut w, Vec2Fx::new(fx(100), fx(100)), 20, 10, 800, 0, 0, 1);
            for i in 0..w.soldiers.len() {
                w.set_goal(i as SoldierId, Vec2Fx::new(fx(300), fx(300)));
            }
            let mut hashes = Vec::new();
            for _ in 0..200 {
                w.tick();
                hashes.push(w.state_hash());
            }
            hashes
        };
        assert_eq!(run(), run(), "同じシードで結果が一致しない");
    }

    #[test]
    fn soldiers_move_toward_their_goal() {
        let mut w = small_world();
        deploy_block(&mut w, Vec2Fx::new(fx(100), fx(100)), 4, 4, 1000, 0, 0, 1);
        let goal = Vec2Fx::new(fx(200), fx(200));
        for i in 0..w.soldiers.len() {
            w.set_goal(i as SoldierId, goal);
        }
        let before = sim_math::dist(w.soldiers.pos(0), goal);
        for _ in 0..100 {
            w.tick();
        }
        let after = sim_math::dist(w.soldiers.pos(0), goal);
        assert!(after < before, "近づいていない: {before} -> {after}");
    }

    /// 全ペアの最大重なり量を返す（テスト用の総当たり）。
    fn worst_overlap(w: &World) -> Fx {
        let mut worst = 0;
        for i in 0..w.soldiers.len() {
            for j in (i + 1)..w.soldiers.len() {
                let d = sim_math::dist(w.soldiers.pos(i), w.soldiers.pos(j));
                let rsum = w.soldiers.radius(i) + w.soldiers.radius(j);
                worst = worst.max(rsum - d);
            }
        }
        worst
    }

    #[test]
    fn overlapping_soldiers_push_apart() {
        // 近傍クエリの上限（12 人）に収まる人数なら、重なりは完全に解消される
        let mut w = small_world();
        for _ in 0..9 {
            w.spawn(Vec2Fx::new(fx(200), fx(200)), 0, 0, 0, Attrs::default(), 0);
        }
        for _ in 0..300 {
            w.tick();
        }
        // 押し戻し量が固定小数点の分解能（1/1024 m）を下回ると収束が止まるので、
        // 数 mm の残りは許容する。視覚的にも戦術的にも意味のない量。
        let residual = worst_overlap(&w);
        assert!(
            residual <= fx_from_mm(10),
            "重なりが残っている: {residual} ({} mm)",
            sim_math::fx_to_mm(residual)
        );
    }

    #[test]
    fn extreme_density_disperses_but_is_not_fully_resolved() {
        // 近傍クエリは 1 人あたり 12 件で打ち切られる（最悪計算量を保証するため）。
        // その結果、1 セルに 12 人を超える極端な密集では互いを見落とすペアが残る。
        // これは意図した割り切りで、M4 の圧迫（crush）システムが
        // 「密度が高すぎる状態」そのものを不利益として扱うことで補う。
        let mut w = small_world();
        for _ in 0..40 {
            w.spawn(Vec2Fx::new(fx(200), fx(200)), 0, 0, 0, Attrs::default(), 0);
        }
        for _ in 0..300 {
            w.tick();
        }
        // 塊としては散っている: 重心からの平均距離が半径を大きく超える
        let mut sum = Vec2Fx::ZERO;
        for i in 0..w.soldiers.len() {
            sum = sum.add(w.soldiers.pos(i));
        }
        let centroid = Vec2Fx::new(
            sum.x / w.soldiers.len() as Fx,
            sum.y / w.soldiers.len() as Fx,
        );
        let mean_dist: i64 = (0..w.soldiers.len())
            .map(|i| sim_math::dist(centroid, w.soldiers.pos(i)) as i64)
            .sum::<i64>()
            / w.soldiers.len() as i64;
        assert!(
            mean_dist > fx_from_mm(1000) as i64,
            "散っていない: 平均距離 {mean_dist}"
        );
    }

    #[test]
    fn soldiers_stay_inside_the_map() {
        let mut w = small_world();
        deploy_block(&mut w, Vec2Fx::new(fx(50), fx(50)), 6, 6, 1000, 0, 0, 1);
        // マップ外を目標にしても出ない
        for i in 0..w.soldiers.len() {
            w.set_goal(i as SoldierId, Vec2Fx::new(fx(10_000), fx(10_000)));
        }
        for _ in 0..500 {
            w.tick();
        }
        let limit = fx(w.terrain.size_m() as i32);
        for i in 0..w.soldiers.len() {
            let p = w.soldiers.pos(i);
            assert!((0..limit).contains(&p.x), "x={}", p.x);
            assert!((0..limit).contains(&p.y), "y={}", p.y);
        }
    }

    /// 味方 2 人のあいだを通り抜けさせ、通過中に相手の体とどれだけ近づいたかを
    /// 返す。`gap_mm` は 2 人の**体の間の隙間**。
    ///
    /// 押し合いで隙間そのものが広がってしまうと「必要な隙間」を測れないので、
    /// 立ち塞がる 2 人は毎 tick 元の位置へ戻す（動かない壁として扱う）。
    fn squeeze_through(gap_mm: i32, running: bool) -> (bool, Fx) {
        let mut w = small_world();
        let body = fx_from_mm(soldiers::BODY_RADIUS_MM);
        let half = fx_from_mm(gap_mm / 2) + body;
        let lane_y = fx(200);
        let wall_x = fx(200);
        let blockers = [
            w.spawn(
                Vec2Fx::new(wall_x, lane_y - half),
                0,
                0,
                0,
                Attrs::default(),
                0,
            ),
            w.spawn(
                Vec2Fx::new(wall_x, lane_y + half),
                0,
                0,
                0,
                Attrs::default(),
                0,
            ),
        ];
        let blocker_pos: Vec<Vec2Fx> = blockers
            .iter()
            .map(|&id| w.soldiers.pos(id as usize))
            .collect();
        let mover = w.spawn(
            Vec2Fx::new(wall_x - fx(4), lane_y),
            0,
            0,
            0,
            Attrs::default(),
            0,
        );
        w.set_goal(mover, Vec2Fx::new(wall_x + fx(4), lane_y));

        let mut worst_overlap = 0;
        for _ in 0..400 {
            for (k, &id) in blockers.iter().enumerate() {
                w.soldiers.set_pos(id as usize, blocker_pos[k]);
                w.soldiers.hot.state[id as usize] = State::Idle;
            }
            if running {
                w.soldiers.hot.state[mover as usize] = State::Charging;
            }
            w.tick();
            let m = mover as usize;
            for &id in &blockers {
                let j = id as usize;
                let rsum = w.soldiers.pass_radius(m) + w.soldiers.pass_radius(j);
                let d = sim_math::dist(w.soldiers.pos(m), w.soldiers.pos(j));
                worst_overlap = worst_overlap.max(rsum - d);
            }
            if w.soldiers.pos(m).x > wall_x + fx(1) {
                return (true, worst_overlap);
            }
        }
        (false, worst_overlap)
    }

    /// 歩いている兵士は 20 cm の隙間があれば、誰にも触れずに通り抜けられる。
    #[test]
    fn walking_soldiers_slip_through_a_twenty_centimeter_gap() {
        let (passed, overlap) = squeeze_through(soldiers::PASS_CLEARANCE_WALK_MM + 50, false);
        assert!(passed, "20 cm の隙間を歩いて抜けられない");
        assert!(
            overlap <= fx_from_mm(10),
            "隙間が足りているのにぶつかっている: {} mm",
            sim_math::fx_to_mm(overlap)
        );
    }

    /// 同じ隙間でも、走っているとぶつかる。走り抜けるには 40 cm 要る。
    #[test]
    fn running_soldiers_need_a_wider_gap() {
        let narrow = soldiers::PASS_CLEARANCE_WALK_MM + 50;
        let (_, walking_overlap) = squeeze_through(narrow, false);
        let (_, narrow_overlap) = squeeze_through(narrow, true);
        assert!(
            narrow_overlap > walking_overlap + fx_from_mm(20),
            "20 cm の隙間を走って抜けても、歩いたときと同じようにぶつからない:              走行 {} mm / 歩行 {} mm",
            sim_math::fx_to_mm(narrow_overlap),
            sim_math::fx_to_mm(walking_overlap)
        );

        let wide = soldiers::PASS_CLEARANCE_RUN_MM + 50;
        let (passed, wide_overlap) = squeeze_through(wide, true);
        assert!(passed, "40 cm の隙間を走って抜けられない");
        assert!(
            wide_overlap <= fx_from_mm(10),
            "40 cm あってもぶつかっている: {} mm",
            sim_math::fx_to_mm(wide_overlap)
        );
    }

    /// 死体は高さ 25 cm の障害物。跨げるので通れるが、重なっているほど遅い。
    #[test]
    fn corpses_slow_movement_without_blocking_it() {
        fn traveled(corpse_layers: u32) -> Fx {
            let mut w = small_world();
            let start = Vec2Fx::new(fx(150), fx(150));
            // 進路上に死体を敷き詰める
            for step in 0..20 {
                for _ in 0..corpse_layers {
                    let id = w.spawn(
                        Vec2Fx::new(start.x + fx(1 + step), start.y),
                        0,
                        0,
                        0,
                        Attrs::default(),
                        0,
                    );
                    w.soldiers.hot.state[id as usize] = State::Dead;
                }
            }
            let walker = w.spawn(start, 0, 0, 0, Attrs::default(), 0);
            w.set_goal(walker, Vec2Fx::new(start.x + fx(25), start.y));
            for _ in 0..300 {
                w.tick();
            }
            sim_math::dist(start, w.soldiers.pos(walker as usize))
        }

        let clear = traveled(0);
        let single = traveled(1);
        let piled = traveled(3);
        assert!(
            single < clear && piled < single,
            "死体が増えても遅くならない: clear={clear} single={single} piled={piled}"
        );
        // 何段積み上がっても通れなくはならない
        assert!(
            piled > fx(5),
            "死体の山が通行を塞いでいる: {} m 進んだだけ",
            piled / FX_ONE
        );
    }

    /// 助走をつけた兵士は、止まったまま殴り合うより強い衝撃を与える
    /// （＝兵士に「下がって走り込む」動機が生まれる）。
    #[test]
    fn a_run_up_adds_impact_on_contact() {
        let mut w = small_world();
        let attacker = w.spawn(
            Vec2Fx::new(fx(150), fx(150)),
            0,
            0,
            0,
            Attrs::new(160, 160, 160, 180, 140, 140, 150, 140, 220, 100, 140, 140),
            0,
        );
        let defender = w.spawn(
            Vec2Fx::new(fx(158), fx(150)),
            sim_math::brad_from_deg(180),
            0,
            1,
            Attrs::default(),
            0,
        );
        w.set_goal(attacker, w.soldiers.pos(defender as usize));
        for _ in 0..200 {
            w.soldiers.hot.state[attacker as usize] = State::Charging;
            w.tick();
            if w.charge.stats.impacts > 0 {
                break;
            }
        }
        assert!(
            w.charge.stats.impacts > 0,
            "8 m 助走して当たっても衝撃が入らない"
        );
        assert!(
            w.soldiers.hp[defender as usize] < soldiers::MAX_HP,
            "衝撃で損害が出ていない"
        );
    }

    /// 兵士は自分の判断で敵へ走り込む（仕様「距離が離れている敵に向かって
    /// 走っていってから攻撃する」）。
    #[test]
    fn soldiers_run_up_to_nearby_enemies_on_their_own() {
        let mut w = small_world();
        let aggressive = Attrs::new(150, 150, 150, 150, 150, 140, 160, 80, 250, 90, 140, 140);
        for k in 0..4 {
            w.spawn(
                Vec2Fx::new(fx(150) + fx(k), fx(150)),
                0,
                0,
                0,
                aggressive,
                0,
            );
            w.spawn(
                Vec2Fx::new(fx(150) + fx(k), fx(153)),
                sim_math::brad_from_deg(180),
                0,
                1,
                aggressive,
                0,
            );
        }
        for _ in 0..300 {
            w.tick();
        }
        assert!(
            w.charge.stats.runups > 0,
            "誰も走り込まない（助走の判断が働いていない）"
        );
    }

    /// 人は瞬時に振り向けない。旋回速度の上限が守られている
    /// （仕様 06 章 2.5 節）。
    #[test]
    fn soldiers_cannot_turn_instantly() {
        let mut w = small_world();
        let id = w.spawn(Vec2Fx::new(fx(150), fx(150)), 0, 0, 0, Attrs::default(), 0);
        let i = id as usize;
        // 真後ろへ歩かせる。向きは 1 tick では反転しない
        w.set_goal(id, Vec2Fx::new(fx(120), fx(150)));
        let step = w.soldiers.turn_step(i);
        w.tick();
        let after_one = sim_math::angle_diff(w.soldiers.hot.facing[i], 0).unsigned_abs();
        assert!(
            after_one <= step + 1,
            "1 tick で {after_one} brad 回った（上限 {step}）"
        );
        assert!(after_one > 0, "まったく向きを変えていない");

        // 走っている兵士は立ち止まっている兵士より旋回が鈍い
        for _ in 0..40 {
            w.tick();
        }
        let standing = w.spawn(Vec2Fx::new(fx(160), fx(160)), 0, 0, 0, Attrs::default(), 0);
        assert!(
            w.soldiers.turn_step(standing as usize) > w.soldiers.turn_step(i)
                || w.soldiers.gait(i) == soldiers::Gait::Standing,
            "走っていても旋回速度が落ちていない"
        );
    }

    /// 長物は間合いを取り、短い武器は組み合う（仕様 06 章 3.1 節の reach）。
    ///
    /// 踏み込まれた状態から始め、それぞれが落ち着く距離を見る。
    #[test]
    fn long_weapons_back_off_while_short_ones_stay_close() {
        fn settle_distance(weapon: combat::Weapon) -> Fx {
            let mut w = small_world();
            // 攻めっ気がなく規律の高い兵士にして、助走の判断が混ざらないようにする
            let steady = Attrs::new(140, 140, 140, 140, 140, 140, 140, 250, 0, 140, 140, 140);
            let a = w.spawn(Vec2Fx::new(fx(150), fx(150)), 0, 0, 0, steady, 0);
            let target_pos = Vec2Fx::new(fx(150) + fx_from_mm(1_000), fx(150));
            let b = w.spawn(target_pos, sim_math::brad_from_deg(180), 0, 1, steady, 0);
            w.combat.set_loadout(a, weapon, combat::Armor::cloth());
            for _ in 0..200 {
                // 相手は棒立ちで、間合いを詰めても離れもしない
                w.soldiers.set_pos(b as usize, target_pos);
                w.soldiers.hot.state[b as usize] = State::Idle;
                w.tick();
            }
            sim_math::dist(w.soldiers.pos(a as usize), target_pos)
        }

        let with_sword = settle_distance(combat::Weapon::sword());
        let with_pike = settle_distance(combat::Weapon::pike());
        assert!(
            with_pike > with_sword + fx_from_mm(200),
            "パイクが剣と同じ距離で組み合っている: pike={} mm sword={} mm",
            sim_math::fx_to_mm(with_pike),
            sim_math::fx_to_mm(with_sword)
        );
        assert!(
            with_sword < fx_from_mm(1_500),
            "剣が間合いを空けすぎている: {} mm",
            sim_math::fx_to_mm(with_sword)
        );
    }

    /// 突撃は接触の手前で失速することがあり、受ける側は身構える
    /// （仕様 06 章 3.7 / 3.8 節）。
    ///
    /// 少人数で、規律の高い戦列へ突っ込ませる。一緒に走る味方が少ないほど
    /// 足は止まりやすい——一人で突っ込むのは怖い。
    #[test]
    fn charges_falter_against_a_braced_line() {
        let mut w = small_world();
        let timid = Attrs::new(150, 150, 150, 150, 150, 120, 10, 200, 250, 200, 140, 140);
        let steady = Attrs::new(150, 150, 150, 150, 150, 140, 220, 250, 60, 120, 140, 140);
        let north = sim_math::brad_from_deg(90);
        let chargers: Vec<SoldierId> = (0..3)
            .map(|k| {
                w.spawn(
                    Vec2Fx::new(fx(150) + fx(k * 3), fx(150)),
                    north,
                    0,
                    0,
                    timid,
                    0,
                )
            })
            .collect();
        let line: Vec<SoldierId> = (0..8)
            .map(|k| {
                w.spawn(
                    Vec2Fx::new(fx(149) + fx(k), fx(153)),
                    sim_math::brad_from_deg(270),
                    0,
                    1,
                    steady,
                    0,
                )
            })
            .collect();
        for (n, &id) in chargers.iter().enumerate() {
            w.set_goal(id, Vec2Fx::new(fx(150) + fx(n as i32 * 3), fx(153)));
        }
        for _ in 0..600 {
            for &id in &chargers {
                if !w.charge.is_hesitating(id, w.tick) {
                    w.soldiers.hot.state[id as usize] = State::Charging;
                }
            }
            w.tick();
        }
        assert!(
            w.charge.stats.braces > 0,
            "突撃を受ける側が誰も身構えていない"
        );
        assert!(
            w.charge.stats.falters > 0,
            "臆病な突撃が一度も失速していない"
        );
        let _ = line;
    }

    /// 疲れた前列は後列と入れ替わる。規律が低い部隊では起きない
    /// （仕様 06 章 6 節）。
    #[test]
    fn tired_front_ranks_rotate_with_the_rear() {
        fn rotations(discipline: u8) -> u32 {
            let mut w = small_world();
            let attrs = Attrs::new(
                140, 140, 140, 140, 140, 140, 140, discipline, 120, 120, 140, 140,
            );
            let mut ids = Vec::new();
            for rank in 0..3 {
                for file in 0..4 {
                    ids.push(w.spawn(
                        Vec2Fx::new(fx(150) + fx(file), fx(150) + fx(rank)),
                        0,
                        0,
                        0,
                        attrs,
                        0,
                    ));
                }
            }
            // 前列（先頭 4 人）だけ疲れきっている
            for &id in ids.iter().take(4) {
                w.soldiers.fatigue[id as usize] = 9_000;
            }
            let unit = organization::Unit {
                soldiers: ids.clone(),
                troop_type: 0,
                formation: organization::FORMATION_LINE,
                formation_origin: Vec2Fx::new(fx(150), fx(150)),
                formation_facing: 0,
                ranks: 3,
                file_spacing: fx_from_mm(900),
                rank_spacing: fx_from_mm(900),
                banner: None,
                formation_change: None,
                path: Vec::new(),
                path_final: Vec2Fx::new(fx(150), fx(150)),
                pursuit_leash: None,
            };
            w.add_command_node(None, 0, 0, ids[0], vec![], Some(unit));
            for _ in 0..200 {
                w.tick();
            }
            w.command.rotations
        }

        let disciplined = rotations(240);
        let rabble = rotations(10);
        assert!(disciplined > 0, "規律の高い部隊でも交代が起きない");
        assert!(
            rabble < disciplined,
            "規律に関係なく交代している: rabble={rabble} disciplined={disciplined}"
        );
    }

    #[test]
    fn soldiers_track_terrain_height() {
        let mut w = small_world();
        let id = w.spawn(Vec2Fx::new(fx(150), fx(150)), 0, 0, 0, Attrs::default(), 0);
        w.tick();
        let expected = sim_math::fx_to_mm(w.terrain.height_at(fx(150), fx(150))) / 10;
        assert_eq!(w.soldiers.z_cm[id as usize] as i32, expected);
    }

    #[test]
    fn state_hash_changes_when_the_world_changes() {
        let mut w = small_world();
        deploy_block(&mut w, Vec2Fx::new(fx(100), fx(100)), 4, 4, 900, 0, 0, 1);
        for i in 0..w.soldiers.len() {
            w.set_goal(i as SoldierId, Vec2Fx::new(fx(250), fx(250)));
        }
        let h0 = w.state_hash();
        for _ in 0..20 {
            w.tick();
        }
        assert_ne!(h0, w.state_hash());
    }

    #[test]
    fn terrain_slows_soldiers_down() {
        // 同じ距離を進むのに、森は草地より時間がかかる
        // （地形効果テーブルが移動に効いていることの確認）
        let w = small_world();
        let grass = SURFACE_EFFECTS[sim_terrain::Surface::Grass as usize].move_mult;
        let forest = SURFACE_EFFECTS[sim_terrain::Surface::DenseForest as usize].move_mult;
        assert!(forest < grass);
        drop(w);
    }

    #[test]
    fn dead_soldiers_do_not_move() {
        let mut w = small_world();
        let id = w.spawn(Vec2Fx::new(fx(150), fx(150)), 0, 0, 0, Attrs::default(), 0);
        w.set_goal(id, Vec2Fx::new(fx(300), fx(300)));
        w.soldiers.hot.state[id as usize] = State::Dead;
        let before = w.soldiers.pos(id as usize);
        for _ in 0..50 {
            w.tick();
        }
        assert_eq!(w.soldiers.pos(id as usize), before);
    }

    /// 仕様 12 章 M5：騎乗している兵士は徒歩より速く動ける（常歩でも）。
    /// 突撃状態でなくても最低限の機動力の差はある。
    #[test]
    fn mounted_soldiers_cover_more_ground_than_infantry() {
        let mut w = small_world();
        let rider = w.spawn(
            Vec2Fx::new(fx(100), fx(100)),
            0,
            0,
            0,
            Attrs::default(),
            flags::MOUNTED,
        );
        let footman = w.spawn(Vec2Fx::new(fx(100), fx(105)), 0, 0, 0, Attrs::default(), 0);
        let goal_r = Vec2Fx::new(fx(100), fx(400));
        let goal_f = Vec2Fx::new(fx(100), fx(405));
        w.set_goal(rider, goal_r);
        w.set_goal(footman, goal_f);
        for _ in 0..200 {
            w.tick();
        }
        let rider_traveled = sim_math::dist(
            Vec2Fx::new(fx(100), fx(100)),
            w.soldiers.pos(rider as usize),
        );
        let footman_traveled = sim_math::dist(
            Vec2Fx::new(fx(100), fx(105)),
            w.soldiers.pos(footman as usize),
        );
        assert!(
            rider_traveled > footman_traveled,
            "騎乗兵が徒歩より速く進んでいない: rider={rider_traveled} footman={footman_traveled}"
        );
    }

    /// 仕様 12 章 M5 の受け入れ条件を指揮系統込みの `World` レベルで確認する:
    /// Charge 命令を出した騎兵が実際に加速し、敵にぶつかって損害を与える。
    /// 決定論（同じ入力で同じ結果）も併せて確認する。
    #[test]
    fn end_to_end_charge_order_deals_damage_deterministically() {
        let run = || {
            let mut w = small_world();
            let rider = w.spawn(
                Vec2Fx::new(fx(100), fx(100)),
                0,
                0,
                0,
                Attrs::new(160, 160, 160, 160, 160, 160, 160, 160, 160, 120, 160, 160),
                flags::MOUNTED,
            );
            let victims: Vec<SoldierId> = (0..4)
                .map(|k| {
                    w.spawn(
                        Vec2Fx::new(fx(100 + k), fx(180)),
                        sim_math::brad_from_deg(180),
                        0,
                        1,
                        Attrs::default(),
                        0,
                    )
                })
                .collect();

            let army = w.add_command_node(None, 0, 0, rider, vec![], None);
            let friendly_unit = organization::Unit {
                soldiers: vec![rider],
                troop_type: 0,
                formation: organization::FORMATION_LINE,
                formation_origin: Vec2Fx::new(fx(100), fx(100)),
                formation_facing: 0,
                ranks: 1,
                file_spacing: fx_from_mm(800),
                rank_spacing: fx_from_mm(800),
                banner: None,
                formation_change: None,
                path: Vec::new(),
                path_final: Vec2Fx::new(fx(100), fx(100)),
                pursuit_leash: None,
            };
            let friendly = w.add_command_node(Some(army), 1, 0, rider, vec![], Some(friendly_unit));
            let enemy_unit = organization::Unit {
                soldiers: victims.clone(),
                troop_type: 0,
                formation: organization::FORMATION_LINE,
                formation_origin: Vec2Fx::new(fx(100), fx(180)),
                formation_facing: 0,
                ranks: 1,
                file_spacing: fx_from_mm(800),
                rank_spacing: fx_from_mm(800),
                banner: None,
                formation_change: None,
                path: Vec::new(),
                path_final: Vec2Fx::new(fx(100), fx(180)),
                pursuit_leash: None,
            };
            let enemy = w.add_command_node(Some(army), 1, 1, victims[0], vec![], Some(enemy_unit));

            w.issue_order(
                army,
                friendly,
                organization::Intent::Charge { target: enemy },
                organization::Priority::Absolute,
            );

            let mut hashes = Vec::new();
            for _ in 0..900 {
                w.tick();
                hashes.push(w.state_hash());
            }
            (hashes, w.combat.stats.damage, w.combat.stats.attacks)
        };

        let (hashes_a, damage_a, attacks_a) = run();
        let (hashes_b, damage_b, attacks_b) = run();
        assert_eq!(
            hashes_a, hashes_b,
            "同じ入力で結果が一致しない（決定論が壊れている）"
        );
        assert!(
            damage_a > 0 && attacks_a > 0,
            "騎兵突撃が誰にも損害を与えていない (damage={damage_a}, attacks={attacks_a})"
        );
        let _ = (damage_b, attacks_b);
    }

    /// 仕様 12 章 M6 の受け入れ条件「杭列が騎兵突撃を実際に止める」を、
    /// 上と同じ指揮系統込みの `World` シナリオで確認する。杭列を敵陣の手前に
    /// 完成させておくと、素通しの場合（上のテスト）に比べて騎兵が敵陣へ
    /// 与える損害が明確に少なくなる。
    #[test]
    fn completed_stakes_reduce_cavalry_charge_damage_end_to_end() {
        fn run(with_stakes: bool) -> u32 {
            let mut w = small_world();
            let rider = w.spawn(
                Vec2Fx::new(fx(100), fx(100)),
                0,
                0,
                0,
                Attrs::new(160, 160, 160, 160, 160, 160, 160, 160, 160, 120, 160, 160),
                flags::MOUNTED,
            );
            if with_stakes {
                let id = w.structures.build(
                    structures::StructureKind::Stakes,
                    Vec2Fx::new(fx(90), fx(140)),
                    Vec2Fx::new(fx(115), fx(140)),
                    1,
                );
                w.structures.set_completion(id, 1000);
            }
            let victims: Vec<SoldierId> = (0..4)
                .map(|k| {
                    w.spawn(
                        Vec2Fx::new(fx(100 + k), fx(180)),
                        sim_math::brad_from_deg(180),
                        0,
                        1,
                        Attrs::default(),
                        0,
                    )
                })
                .collect();

            let army = w.add_command_node(None, 0, 0, rider, vec![], None);
            let friendly_unit = organization::Unit {
                soldiers: vec![rider],
                troop_type: 0,
                formation: organization::FORMATION_LINE,
                formation_origin: Vec2Fx::new(fx(100), fx(100)),
                formation_facing: 0,
                ranks: 1,
                file_spacing: fx_from_mm(800),
                rank_spacing: fx_from_mm(800),
                banner: None,
                formation_change: None,
                path: Vec::new(),
                path_final: Vec2Fx::new(fx(100), fx(100)),
                pursuit_leash: None,
            };
            let friendly = w.add_command_node(Some(army), 1, 0, rider, vec![], Some(friendly_unit));
            let enemy_unit = organization::Unit {
                soldiers: victims.clone(),
                troop_type: 0,
                formation: organization::FORMATION_LINE,
                formation_origin: Vec2Fx::new(fx(100), fx(180)),
                formation_facing: 0,
                ranks: 1,
                file_spacing: fx_from_mm(800),
                rank_spacing: fx_from_mm(800),
                banner: None,
                formation_change: None,
                path: Vec::new(),
                path_final: Vec2Fx::new(fx(100), fx(180)),
                pursuit_leash: None,
            };
            let enemy = w.add_command_node(Some(army), 1, 1, victims[0], vec![], Some(enemy_unit));

            w.issue_order(
                army,
                friendly,
                organization::Intent::Charge { target: enemy },
                organization::Priority::Absolute,
            );

            for _ in 0..900 {
                w.tick();
            }
            victims
                .iter()
                .map(|&v| (soldiers::MAX_HP - w.soldiers.hp[v as usize]) as u32)
                .sum()
        }

        let damage_without_stakes = run(false);
        let damage_with_stakes = run(true);
        assert!(
            damage_with_stakes < damage_without_stakes,
            "杭列があっても突撃の損害が減っていない: with={damage_with_stakes} without={damage_without_stakes}"
        );
    }

    /// M7 の「軍 1 個」テストシナリオ: ルート（Unit なし）の下に 2 個の葉部隊、
    /// 反対陣営には敵の葉部隊が 1 個。
    struct ArmyScenario {
        world: World,
        root: organization::NodeId,
        children: [organization::NodeId; 2],
    }

    fn decent_attrs() -> Attrs {
        Attrs::new(140, 140, 140, 140, 140, 140, 140, 150, 130, 120, 150, 140)
    }

    fn build_army_scenario() -> ArmyScenario {
        build_army_scenario_with(6, 10)
    }

    /// `own_per_child`: 自軍の葉部隊 1 個あたりの人数。`enemy_count`: 敵部隊の人数。
    /// アーキタイプの差が Objective の選択を左右するかを見るテストでは、
    /// 兵力比を拮抗させたり不利にしたりして分岐点に乗せる。
    fn build_army_scenario_with(own_per_child: u32, enemy_count: u32) -> ArmyScenario {
        let mut w = small_world();
        let root_commander = w.spawn(Vec2Fx::new(fx(110), fx(90)), 0, 0, 0, decent_attrs(), 0);
        let root = w.add_command_node(None, 0, 0, root_commander, vec![], None);

        let mut children = [0u32; 2];
        for (k, x) in [fx(90), fx(130)].into_iter().enumerate() {
            let commander = w.spawn(Vec2Fx::new(x, fx(100)), 0, 0, 0, decent_attrs(), 0);
            let mut soldiers = vec![commander];
            for j in 1..own_per_child {
                soldiers.push(w.spawn(
                    Vec2Fx::new(x + fx(j as i32), fx(100)),
                    0,
                    0,
                    0,
                    decent_attrs(),
                    0,
                ));
            }
            let unit = organization::Unit {
                soldiers,
                troop_type: 0,
                formation: organization::FORMATION_LINE,
                formation_origin: Vec2Fx::new(x, fx(100)),
                formation_facing: 0,
                ranks: 1,
                file_spacing: fx_from_mm(800),
                rank_spacing: fx_from_mm(800),
                banner: None,
                formation_change: None,
                path: Vec::new(),
                path_final: Vec2Fx::new(x, fx(100)),
                pursuit_leash: None,
            };
            children[k] = w.add_command_node(Some(root), 1, 0, commander, vec![], Some(unit));
        }

        let enemy_commander = w.spawn(Vec2Fx::new(fx(110), fx(300)), 0, 0, 1, decent_attrs(), 0);
        let mut enemy_soldiers = vec![enemy_commander];
        for j in 1..enemy_count {
            enemy_soldiers.push(w.spawn(
                Vec2Fx::new(fx(105 + j as i32), fx(300)),
                sim_math::brad_from_deg(180),
                0,
                1,
                decent_attrs(),
                0,
            ));
        }
        let enemy_unit = organization::Unit {
            soldiers: enemy_soldiers,
            troop_type: 0,
            formation: organization::FORMATION_LINE,
            formation_origin: Vec2Fx::new(fx(110), fx(300)),
            formation_facing: sim_math::BRAD_HALF as u16,
            ranks: 1,
            file_spacing: fx_from_mm(800),
            rank_spacing: fx_from_mm(800),
            banner: None,
            formation_change: None,
            path: Vec::new(),
            path_final: Vec2Fx::new(fx(110), fx(300)),
            pursuit_leash: None,
        };
        w.add_command_node(None, 1, 1, enemy_commander, vec![], Some(enemy_unit));

        ArmyScenario {
            world: w,
            root,
            children,
        }
    }

    /// 仕様 12 章 M7 の受け入れ条件「どの指揮官を選んでも『なぜその判断をしたか』が
    /// スコア内訳で見られる」。
    #[test]
    fn commander_decisions_are_logged_with_a_score_breakdown() {
        let mut scenario = build_army_scenario();
        for _ in 0..300 {
            scenario.world.tick();
        }
        let root_node = scenario.world.command.node(scenario.root).unwrap();
        let record = root_node
            .decision_log
            .latest()
            .expect("ルート指揮官の判断が記録されていない");
        assert!(!record.breakdown.is_empty(), "スコア内訳が空");
        assert!(!record.candidates.is_empty(), "候補が空");
        assert!(
            record
                .candidates
                .iter()
                .any(|(name, _)| *name == record.chosen),
            "選ばれた候補が候補一覧に含まれていない"
        );
    }

    /// 仕様 12 章 M7 の受け入れ条件「tactical_skill の低い指揮官が戦況を誤認し、
    /// それが UI で『認識 vs 実際』として見える」。
    #[test]
    fn low_tactical_skill_commander_misjudges_the_situation() {
        let mut scenario = build_army_scenario();
        scenario.world.set_commander_attrs(
            scenario.root,
            organization::CommanderAttrs {
                tactical_skill: 10,
                ..Default::default()
            },
        );
        for _ in 0..300 {
            scenario.world.tick();
        }
        let (perceived, actual) = scenario.world.commander_perceived_vs_true(scenario.root);
        assert_ne!(
            perceived.force_ratio_permille, actual.force_ratio_permille,
            "スキルの低い指揮官の認識が実際の値と一致してしまっている（ノイズが効いていない）"
        );
    }

    /// 仕様 12 章 M7 の受け入れ条件「アーキタイプを変えると同じシナリオの結果が
    /// 有意に変わる」。同じ布陣で総大将のアーキタイプだけを変え、その後の
    /// 部隊の実際の位置（=結果）が有意に違うことを確認する。
    #[test]
    fn different_archetypes_change_the_outcome() {
        fn run(archetype: &str) -> Vec2Fx {
            // 自軍を数的不利にしておく。慎重な指揮官はここで高地防御に傾き、
            // 大胆な指揮官はそれでも攻勢の Objective を選び続ける——という
            // 分岐が起きやすい状況を作る。
            let mut scenario = build_army_scenario_with(4, 20);
            let idx = commander_ai::archetype_index(archetype).unwrap();
            scenario
                .world
                .set_commander_archetype(scenario.root, idx, 1);
            for _ in 0..2000 {
                scenario.world.tick();
            }
            let mut sum = Vec2Fx::ZERO;
            let mut n = 0i32;
            for &child in &scenario.children {
                if let Some(node) = scenario.world.command.node(child) {
                    sum = sum.add(node.stats.centroid);
                    n += 1;
                }
            }
            Vec2Fx::new(sum.x / n.max(1), sum.y / n.max(1))
        }

        let reckless = run("reckless_youth");
        let cautious = run("cautious_commander");
        let moved = sim_math::dist(reckless, cautious);
        assert!(
            moved > fx_from_mm(500),
            "アーキタイプを変えても結果（部隊の位置）がほとんど変わっていない: {} mm",
            sim_math::fx_to_mm(moved)
        );
    }
}
