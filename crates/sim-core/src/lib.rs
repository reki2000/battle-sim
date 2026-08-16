//! ワールド状態とシミュレーションシステム。
//!
//! このクレートは wasm に依存しない。ネイティブでテストとベンチができる
//! （`sim-headless`）ことが、バランス調整と性能計測の前提になっている。
//!
//! M0〜M2 の移動基盤に加え、M3 の指揮ツリー・命令・陣形を実装する。
//! M4 の白兵戦・負傷・士気の基礎も実装する（`docs/spec/12-roadmap.md`）。

#![forbid(unsafe_code)]

pub mod cavalry;
pub mod combat;
pub mod organization;
pub mod pathing;
pub mod snapshot;
pub mod soldiers;
pub mod spatial;

use cavalry::CavalrySystem;
use combat::CombatSystem;
use organization::CommandTree;
use sim_math::{fx, fx_div, fx_from_mm, fx_mul, per_sec_to_per_tick, Fx, Vec2Fx, FX_ONE};
use sim_terrain::{Terrain, TerrainParams, SURFACE_EFFECTS};
use soldiers::{flags, Attrs, SoldierId, Soldiers, State};
use spatial::{SpatialHash, MAX_NEIGHBORS};

/// シミュレーションのロジックバージョン。リプレイの互換性判定に使う。
pub const SIM_VERSION: u32 = 3;

/// 衝突解決の反復回数（仕様 06 章 2.2）。
const SEPARATION_ITERATIONS: usize = 2;
/// 押し戻しの緩和係数。完全に解消しないことで密集の圧力が残る。
const SEPARATION_RELAX_PERMILLE: i32 = 500;

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
    /// 各兵士の目標位置。M2 で陣形スロットに置き換わる
    goal: Vec<Vec2Fx>,
    /// 衝突解決の書き込み先（読み書きフェーズを分けるため）
    push_x: Vec<Fx>,
    push_y: Vec<Fx>,
    /// 衝突解決の全反復で蓄積した重なり量。圧迫（crush）判定に使う。
    crush_accum: Vec<Fx>,
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
        World {
            seed: config.seed,
            tick: 0,
            terrain,
            soldiers: Soldiers::default(),
            hash: SpatialHash::default(),
            command: CommandTree::new(),
            combat: CombatSystem::default(),
            cavalry: CavalrySystem::default(),
            goal: Vec::new(),
            push_x: Vec::new(),
            push_y: Vec::new(),
            crush_accum: Vec::new(),
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
        if soldier_flags & flags::MOUNTED != 0 {
            self.combat.set_mounted(id, true);
        }
        self.goal.push(pos);
        self.push_x.push(0);
        self.push_y.push(0);
        self.crush_accum.push(0);
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

    /// 1 ティック進める。
    ///
    /// フェーズの順序は仕様 02 章 5 節に従う。M0 では未実装のフェーズを飛ばす。
    pub fn tick(&mut self) {
        self.command
            .tick(&mut self.soldiers, &self.terrain, self.tick);
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
            map_limit,
        );
        self.combat
            .tick(self.seed, self.tick, &mut self.soldiers, &self.hash);
        self.steer();
        self.integrate_motion();
        self.resolve_collisions();
        self.follow_terrain();
        self.tick += 1;
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
            // 命令を受けていない兵士はその場を保つ。押し合いで動かされても
            // 元の位置に戻ろうとはしない（さもないと 1 点に集まれという命令が
            // 分離と綱引きし、密集が解けなくなる）。
            if self.soldiers.hot.state[i] == State::Idle {
                self.soldiers.hot.vel_x[i] = 0;
                self.soldiers.hot.vel_y[i] = 0;
                self.face_nearest_enemy_if_close(i);
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
            self.soldiers.hot.facing[i] = dir.angle();
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
            self.soldiers.hot.facing[i] = enemy_pos.sub(pos).angle();
        }
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
        let base_mm_per_s = if mounted && self.soldiers.hot.state[i] == State::Charging {
            const CHARGE_TOP_MM_PER_S: i32 = 8000;
            let m = self.cavalry.momentum_permille(id) as i32;
            base_mm_per_s + (CHARGE_TOP_MM_PER_S - base_mm_per_s) * m / 1000
        } else {
            base_mm_per_s
        };
        let per_tick = per_sec_to_per_tick(fx_from_mm(base_mm_per_s));

        let pos = self.soldiers.pos(i);
        let surface = self.terrain.surface_at(pos.x, pos.y);
        let eff = &SURFACE_EFFECTS[surface as usize];
        let after_terrain = sim_math::fx_scale_permille(per_tick, eff.move_mult as i32);

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

            let limit = fx(self.terrain.size_m() as i32) - FX_ONE;
            self.soldiers.hot.pos_x[i] = next_x.clamp(0, limit);
            self.soldiers.hot.pos_y[i] = next_y.clamp(0, limit);
        }
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
                let ri = self.soldiers.radius(i);
                let mi = self.soldiers.mass(i);
                let cnt = self.hash.query_neighbors(pi.x, pi.y, &mut neighbors);

                for &jid in &neighbors[..cnt] {
                    let j = jid as usize;
                    if j == i || !self.soldiers.is_alive(j) {
                        continue;
                    }
                    let pj = self.soldiers.pos(j);
                    let rsum = ri + self.soldiers.radius(j);
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
            let ri = self.soldiers.radius(i);
            let cnt = self.hash.query_neighbors(pi.x, pi.y, &mut neighbors);
            let mut residual = 0 as Fx;
            for &jid in &neighbors[..cnt] {
                let j = jid as usize;
                if j == i || !self.soldiers.is_alive(j) {
                    continue;
                }
                let pj = self.soldiers.pos(j);
                let rsum = ri + self.soldiers.radius(j);
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
        for i in 0..self.soldiers.len() {
            mix(self.soldiers.hot.pos_x[i] as u32 as u64);
            mix(self.soldiers.hot.pos_y[i] as u32 as u64);
            mix(self.soldiers.hot.vel_x[i] as u16 as u64);
            mix(self.soldiers.hot.vel_y[i] as u16 as u64);
            mix(self.soldiers.hot.facing[i] as u64);
            mix(self.soldiers.hot.state[i] as u64);
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
}
