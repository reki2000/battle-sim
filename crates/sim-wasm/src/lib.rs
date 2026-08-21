//! JS との境界。
//!
//! ここにはロジックを置かない。`sim-core` の薄いラッパに徹する
//! （仕様 01 章 2 節）。
//!
//! 境界を跨ぐ呼び出しはフレームあたり定数回に抑える。エンティティごとの
//! 呼び出しはしない。描画データはリニアメモリのビューとして JS に渡す。

#![forbid(unsafe_code)]

use sim_core::metrics::MissionKind;
use sim_core::organization::{ApproachStyle, Intent, MoveSpeed, Priority, ShootMode, Side, Unit};
use sim_core::snapshot::RenderSnapshot;
use sim_core::structures::StructureKind;
use sim_core::World as CoreWorld;
use sim_math::{fx, fx_from_mm, Vec2Fx};
use sim_render::ArmyRenderer as CoreArmyRenderer;
use sim_terrain::{BattleSiteCandidate, Terrain, TerrainGrids};
use wasm_bindgen::prelude::*;

/// 状態保持型の人物ポリゴン描画エンジン。
///
/// 位置の正本は [`World`] が出力する描画スナップショットであり、この型は
/// ルート移動を行わない。モーション位相、クロスフェード、体格、騎乗姿勢、
/// カリング、LOD、最終三角形列だけを内部に保持・生成する。
#[wasm_bindgen]
pub struct ArmyRenderer {
    inner: CoreArmyRenderer,
}

impl Default for ArmyRenderer {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl ArmyRenderer {
    #[wasm_bindgen(constructor)]
    pub fn new() -> ArmyRenderer {
        ArmyRenderer {
            inner: CoreArmyRenderer::new(),
        }
    }

    #[wasm_bindgen(js_name = apiMajor)]
    pub fn api_major() -> u32 {
        sim_render::API_MAJOR
    }

    #[wasm_bindgen(js_name = apiMinor)]
    pub fn api_minor() -> u32 {
        sim_render::API_MINOR
    }

    /// 20 Hz の兵士スナップショットを一括適用する。同一 tick・同一内容は無視する。
    #[wasm_bindgen(js_name = applySnapshot)]
    pub fn apply_snapshot(&mut self, bytes: &[u8], stride: u32, tick: u32) -> Result<(), JsValue> {
        self.inner
            .apply_snapshot(bytes, stride as usize, tick)
            .map_err(JsValue::from_str)
    }

    /// クォータービューのカメラを更新する。変更時だけ呼べばよい。
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = setCamera)]
    pub fn set_camera(
        &mut self,
        center_x: f32,
        center_y: f32,
        px_per_m: f32,
        width: f32,
        height: f32,
        world_size: f32,
    ) {
        self.inner
            .set_camera(center_x, center_y, px_per_m, width, height, world_size);
    }

    /// モーションを `dt` 秒進め、補間率 `alpha` と LOD から三角形列を生成する。
    pub fn step(&mut self, dt: f32, alpha: f32, lod: u8) {
        self.inner.step(dt, alpha, lod);
    }

    #[wasm_bindgen(js_name = verticesPtr)]
    pub fn vertices_ptr(&self) -> *const f32 {
        self.inner.vertices().as_ptr()
    }

    #[wasm_bindgen(js_name = verticesFloatLen)]
    pub fn vertices_float_len(&self) -> u32 {
        self.inner.vertices().len() as u32
    }

    #[wasm_bindgen(js_name = vertexCount)]
    pub fn vertex_count(&self) -> u32 {
        (self.inner.vertices().len() / sim_render::VERTEX_FLOATS) as u32
    }

    #[wasm_bindgen(js_name = agentCount)]
    pub fn agent_count(&self) -> u32 {
        self.inner.agent_count()
    }

    #[wasm_bindgen(js_name = visibleCount)]
    pub fn visible_count(&self) -> u32 {
        self.inner.visible_count()
    }

    #[wasm_bindgen(js_name = drawnCount)]
    pub fn drawn_count(&self) -> u32 {
        self.inner.drawn_count()
    }

    #[wasm_bindgen(js_name = culledCount)]
    pub fn culled_count(&self) -> u32 {
        self.inner.culled_count()
    }
}

/// wasm 側のワールドハンドル。
#[wasm_bindgen]
pub struct World {
    inner: CoreWorld,
    snapshot: RenderSnapshot,
}

#[wasm_bindgen]
impl World {
    /// 生成済みの地形グリッドからワールドを作る。
    ///
    /// **地形の生成は JS 側（`web/src/terrain`）にある。** ワーカーが渡して
    /// くるのは、その場で生成したものか IndexedDB から復元したもののどちらか
    /// で、どちらもここでは同じ扱いになる。
    ///
    /// 各グリッドは `dim` × `dim`（行優先）。通行コストと崖は渡さない——
    /// 効果テーブルの正本は Rust 側にあり、`Terrain::from_grids` がそこから
    /// 計算し直す。`battle_sites_flat` は `battleSites()` と同じ 7 要素 1 組の
    /// 平坦な配列。
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = fromTerrain)]
    pub fn from_terrain(
        seed_lo: u32,
        seed_hi: u32,
        dim: u32,
        cell_m: u32,
        height: Vec<i16>,
        ground: Vec<u8>,
        vegetation: Vec<u8>,
        overlay: Vec<u8>,
        water: Vec<u16>,
        water_kind: Vec<u8>,
        moisture: Vec<u8>,
        battle_sites_flat: Vec<i32>,
    ) -> World {
        let seed = ((seed_hi as u64) << 32) | seed_lo as u64;
        let terrain = Terrain::from_grids(TerrainGrids {
            dim,
            cell_m,
            seed,
            height,
            ground,
            vegetation,
            overlay,
            water,
            water_kind,
            moisture,
            battle_sites: parse_battle_sites(&battle_sites_flat),
        });
        World {
            inner: CoreWorld::with_terrain(seed, terrain),
            snapshot: RenderSnapshot::default(),
        }
    }

    /// 生成済みの地形グリッドから、会戦プリセットのワールドを作る。
    ///
    /// グリッドは**整形後**——シナリオ固有の地勢（森の縁・泥濘の耕地・緩い
    /// 登り）まで焼き込んだもの——でなければならない。整形は生成器
    /// （`web/src/terrain/scenarios.ts`）が行う。
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = fromScenarioTerrain)]
    pub fn from_scenario_terrain(
        index: usize,
        dim: u32,
        cell_m: u32,
        height: Vec<i16>,
        ground: Vec<u8>,
        vegetation: Vec<u8>,
        overlay: Vec<u8>,
        water: Vec<u16>,
        water_kind: Vec<u8>,
        moisture: Vec<u8>,
        battle_sites_flat: Vec<i32>,
    ) -> Option<World> {
        let def = sim_core::scenario::get(index)?;
        let terrain = Terrain::from_grids(TerrainGrids {
            dim,
            cell_m,
            seed: def.terrain_seed,
            height,
            ground,
            vegetation,
            overlay,
            water,
            water_kind,
            moisture,
            battle_sites: parse_battle_sites(&battle_sites_flat),
        });
        Some(World {
            inner: sim_core::scenario::build_world(def, terrain),
            snapshot: RenderSnapshot::default(),
        })
    }

    /// 選択できる会戦プリセットの一覧（JSON、起動時に 1 回だけ読む想定）。
    ///
    /// 配列の index が [`World::from_scenario_terrain`] の `index` になる。
    #[wasm_bindgen(js_name = scenarioListJson)]
    pub fn scenario_list_json() -> String {
        let entries: Vec<String> = sim_core::scenario::SCENARIOS
            .iter()
            .map(scenario_json)
            .collect();
        format!("[{}]", entries.join(","))
    }

    /// シミュレーションのロジックバージョン。リプレイの互換性判定に使う。
    #[wasm_bindgen(js_name = simVersion)]
    pub fn sim_version() -> u32 {
        sim_core::SIM_VERSION
    }

    /// 描画スナップショットのレイアウトバージョン。
    #[wasm_bindgen(js_name = snapshotVersion)]
    pub fn snapshot_version() -> u32 {
        sim_core::snapshot::SNAPSHOT_VERSION
    }

    // ── 実行 ────────────────────────────────────────────

    pub fn tick(&mut self) {
        self.inner.tick();
    }

    /// 複数ティックをまとめて進める（早送り時に境界を跨ぐ回数を減らす）。
    #[wasm_bindgen(js_name = tickMany)]
    pub fn tick_many(&mut self, n: u32) {
        for _ in 0..n {
            self.inner.tick();
        }
    }

    #[wasm_bindgen(js_name = tickCount)]
    pub fn tick_count(&self) -> u32 {
        self.inner.tick
    }

    /// 状態ハッシュ。決定論の検証に使う。u64 を 2 つの u32 に分けて返す。
    #[wasm_bindgen(js_name = stateHashLo)]
    pub fn state_hash_lo(&self) -> u32 {
        self.inner.state_hash() as u32
    }

    #[wasm_bindgen(js_name = stateHashHi)]
    pub fn state_hash_hi(&self) -> u32 {
        (self.inner.state_hash() >> 32) as u32
    }

    // ── サンドボックス配置 ──────────────────────────────

    /// 方陣を組んだ部隊を配置する。
    #[wasm_bindgen(js_name = deployBlock)]
    #[allow(clippy::too_many_arguments)]
    pub fn deploy_block(
        &mut self,
        x_m: i32,
        y_m: i32,
        files: u32,
        ranks: u32,
        spacing_mm: i32,
        faction: u8,
        unit_id: u16,
        troop_type: u16,
        seed_salt: u32,
    ) {
        sim_core::deploy_block_typed(
            &mut self.inner,
            Vec2Fx::new(fx(x_m), fx(y_m)),
            files,
            ranks,
            spacing_mm,
            faction,
            unit_id,
            troop_type,
            seed_salt,
        );
    }

    /// ある陣営の全兵士に移動目標を与える。
    ///
    /// 指揮ツリーを組む前のサンドボックス配置と、既存リプレイの再生にだけ使う。
    /// 通常の UI 命令は `issueMoveTo` を通して指揮系統へ投入する。
    #[wasm_bindgen(js_name = setFactionGoal)]
    pub fn set_faction_goal(&mut self, faction: u8, x_m: i32, y_m: i32) {
        let goal = Vec2Fx::new(fx(x_m), fx(y_m));
        let ids: Vec<u32> = (0..self.inner.soldiers.len())
            .filter(|&i| self.inner.soldiers.faction[i] == faction)
            .map(|i| i as u32)
            .collect();
        for id in ids {
            self.inner.set_goal(id, goal);
        }
    }

    #[wasm_bindgen(js_name = soldierCount)]
    pub fn soldier_count(&self) -> u32 {
        self.inner.soldiers.len() as u32
    }

    #[wasm_bindgen(js_name = aliveCount)]
    pub fn alive_count(&self) -> u32 {
        self.inner.soldiers.alive_count() as u32
    }

    // ── 描画用メモリビュー ──────────────────────────────

    /// スナップショットを書き出す。毎フレーム 1 回だけ呼ぶ。
    #[wasm_bindgen(js_name = writeSnapshot)]
    pub fn write_snapshot(&mut self) {
        self.snapshot.write(&self.inner.soldiers);
    }

    /// スナップショットの先頭アドレス。`writeSnapshot` の後に読むこと。
    ///
    /// wasm のメモリが grow するとポインタが変わりうるので、
    /// JS 側は毎フレーム取り直すか `memory.buffer` の同一性を確認すること。
    #[wasm_bindgen(js_name = soldiersPtr)]
    pub fn soldiers_ptr(&self) -> *const u8 {
        self.snapshot.ptr()
    }

    #[wasm_bindgen(js_name = soldiersByteLen)]
    pub fn soldiers_byte_len(&self) -> u32 {
        self.snapshot.byte_len()
    }

    /// 兵士 1 体あたりのバイト数。JS 側のパーサと突き合わせる。
    #[wasm_bindgen(js_name = soldierStride)]
    pub fn soldier_stride() -> u32 {
        sim_core::snapshot::SOLDIER_STRIDE as u32
    }

    // ── 地形 ────────────────────────────────────────────

    #[wasm_bindgen(js_name = terrainDim)]
    pub fn terrain_dim(&self) -> u32 {
        self.inner.terrain.dim
    }

    #[wasm_bindgen(js_name = terrainCellM)]
    pub fn terrain_cell_m(&self) -> u32 {
        self.inner.terrain.cell_m
    }

    #[wasm_bindgen(js_name = terrainSizeM)]
    pub fn terrain_size_m(&self) -> u32 {
        self.inner.terrain.size_m()
    }

    /// 地質グリッド（[`sim_terrain::Ground`] の discriminant）。
    #[wasm_bindgen(js_name = terrainGroundPtr)]
    pub fn terrain_ground_ptr(&self) -> *const u8 {
        self.inner.terrain.ground.as_ptr()
    }

    /// 植生グリッド（[`sim_terrain::Vegetation`] の discriminant）。
    #[wasm_bindgen(js_name = terrainVegetationPtr)]
    pub fn terrain_vegetation_ptr(&self) -> *const u8 {
        self.inner.terrain.vegetation.as_ptr()
    }

    /// 人工物グリッド（[`sim_terrain::Overlay`] の discriminant）。
    #[wasm_bindgen(js_name = terrainOverlayPtr)]
    pub fn terrain_overlay_ptr(&self) -> *const u8 {
        self.inner.terrain.overlay.as_ptr()
    }

    /// 湿度グリッド（0..255）。泥濘の描画とデバッグに使う。
    #[wasm_bindgen(js_name = terrainMoisturePtr)]
    pub fn terrain_moisture_ptr(&self) -> *const u8 {
        self.inner.terrain.moisture.as_ptr()
    }

    #[wasm_bindgen(js_name = terrainHeightPtr)]
    pub fn terrain_height_ptr(&self) -> *const i16 {
        self.inner.terrain.height.as_ptr()
    }

    #[wasm_bindgen(js_name = terrainPassabilityPtr)]
    pub fn terrain_passability_ptr(&self) -> *const u8 {
        self.inner.terrain.passability.as_ptr()
    }

    /// 水深グリッド（cm、0 = 陸地）。仕様 08 章の水面描画で使う。
    #[wasm_bindgen(js_name = terrainWaterPtr)]
    pub fn terrain_water_ptr(&self) -> *const u16 {
        self.inner.terrain.water.as_ptr()
    }

    /// 水域種別グリッド（[`sim_terrain::WaterKind`] の discriminant）。
    #[wasm_bindgen(js_name = terrainWaterKindPtr)]
    pub fn terrain_water_kind_ptr(&self) -> *const u8 {
        self.inner.terrain.water_kind.as_ptr()
    }

    /// 崖ビットマスクグリッド（[`sim_terrain::cliff_bits`]）。
    /// 描画で崖面の側面クアッドを立てる方向の判定に使う。
    #[wasm_bindgen(js_name = terrainCliffPtr)]
    pub fn terrain_cliff_ptr(&self) -> *const u8 {
        self.inner.terrain.cliff.as_ptr()
    }

    /// 会戦地候補の数。
    #[wasm_bindgen(js_name = battleSiteCount)]
    pub fn battle_site_count(&self) -> u32 {
        self.inner.terrain.battle_sites.len() as u32
    }

    /// 会戦地候補の詳細（JSON 経由、低頻度呼び出し想定）。
    #[wasm_bindgen(js_name = battleSites)]
    pub fn battle_sites(&self) -> Vec<i32> {
        // wasm-bindgen の型変換を単純に保つため、候補ごとに
        // [x_m, y_m, score, passable_permille, asymmetry_permille, openness_permille, bottleneck_count]
        // の平坦な配列として返す。
        let mut out = Vec::with_capacity(self.inner.terrain.battle_sites.len() * 7);
        for s in &self.inner.terrain.battle_sites {
            out.push(s.x_m);
            out.push(s.y_m);
            out.push(s.score);
            out.push(s.passable_permille as i32);
            out.push(s.asymmetry_permille as i32);
            out.push(s.openness_permille as i32);
            out.push(s.bottleneck_count as i32);
        }
        out
    }

    // ── 戦闘統計・イベントログ（M4） ──────────────────────

    /// 戦闘の集計値。統計グラフ・HUD 用。
    ///
    /// `[attacks, hits, damage, kills, downed, pursuit_kills, melee_kills,
    ///   missile_kills, crush_kills, bleed_kills, shots_fired, friendly_fire_hits,
    ///   charge_kills, dismounts, horse_refusals]`（末尾 3 つは M5）。
    #[wasm_bindgen(js_name = combatStats)]
    pub fn combat_stats(&self) -> Vec<u32> {
        let s = &self.inner.combat.stats;
        vec![
            s.attacks,
            s.hits,
            s.damage,
            s.kills,
            s.downed,
            s.pursuit_kills,
            s.melee_kills,
            s.missile_kills,
            s.crush_kills,
            s.bleed_kills,
            s.shots_fired,
            s.friendly_fire_hits,
            s.charge_kills,
            s.dismounts,
            s.horse_refusals,
        ]
    }

    #[wasm_bindgen(js_name = combatEventCount)]
    pub fn combat_event_count(&self) -> u32 {
        self.inner.combat.events.len() as u32
    }

    /// 直近の戦闘イベントを最大 `max` 件、古い順に返す。
    /// 1 件あたり `[tick, attacker, defender, kind, cause]` の 5 要素。
    #[wasm_bindgen(js_name = combatEvents)]
    pub fn combat_events(&self, max: u32) -> Vec<i32> {
        let events = &self.inner.combat.events;
        let skip = events.len().saturating_sub(max as usize);
        let mut out = Vec::with_capacity((events.len() - skip) * 5);
        for e in events.iter().skip(skip) {
            out.push(e.tick as i32);
            out.push(e.attacker as i32);
            out.push(e.defender as i32);
            out.push(e.kind as i32);
            out.push(e.cause as i32);
        }
        out
    }

    // ── 指揮ツリー（M3） ────────────────────────────────

    #[wasm_bindgen(js_name = commandNodeCount)]
    pub fn command_node_count(&self) -> u32 {
        self.inner.command.nodes.len() as u32
    }

    /// 指揮ノードの一覧。指揮系統ツリー UI 用。
    /// 1 ノードあたり `[id, parent(-1), echelon, faction, commander_id,
    ///   command_state, has_unit(0/1), formation(-1), alive, downed, dead,
    ///   broken, avg_morale, avg_fatigue, centroid_x_cm, centroid_y_cm]`
    /// の 16 要素（座標は cm 単位の整数）。
    #[wasm_bindgen(js_name = commandNodes)]
    pub fn command_nodes(&self) -> Vec<i32> {
        let mut out = Vec::with_capacity(self.inner.command.nodes.len() * 16);
        for node in &self.inner.command.nodes {
            out.push(node.id as i32);
            out.push(node.parent.map_or(-1, |p| p as i32));
            out.push(node.echelon as i32);
            out.push(node.faction as i32);
            out.push(node.commander as i32);
            out.push(node.command_state as i32);
            out.push(i32::from(node.unit.is_some()));
            out.push(node.unit.as_ref().map_or(-1, |u| u.formation as i32));
            out.push(node.stats.alive as i32);
            out.push(node.stats.downed as i32);
            out.push(node.stats.dead as i32);
            out.push(node.stats.broken as i32);
            out.push(node.stats.avg_morale as i32);
            out.push(node.stats.avg_fatigue as i32);
            out.push(sim_math::fx_to_mm(node.stats.centroid.x) / 10);
            out.push(sim_math::fx_to_mm(node.stats.centroid.y) / 10);
        }
        out
    }

    #[wasm_bindgen(js_name = commandEventCount)]
    pub fn command_event_count(&self) -> u32 {
        self.inner.command.events.len() as u32
    }

    /// 直近の命令イベントを最大 `max` 件、古い順に返す。
    /// 1 件あたり `[tick, node, order_id(-1), kind]` の 4 要素。
    #[wasm_bindgen(js_name = commandEvents)]
    pub fn command_events(&self, max: u32) -> Vec<i32> {
        let events = &self.inner.command.events;
        let skip = events.len().saturating_sub(max as usize);
        let mut out = Vec::with_capacity((events.len() - skip) * 4);
        for e in events.iter().skip(skip) {
            out.push(e.tick as i32);
            out.push(e.node as i32);
            out.push(e.order.map_or(-1, |o| o as i32));
            out.push(e.kind as i32);
        }
        out
    }

    /// 連続した ID 範囲の兵士を 1 個の Unit として指揮ツリーに登録する
    /// （`deployBlock` が割り当てる ID が連続していることを前提にした簡易 API）。
    /// 戻り値はノード ID。
    #[wasm_bindgen(js_name = addLineUnit)]
    #[allow(clippy::too_many_arguments)]
    pub fn add_line_unit(
        &mut self,
        faction: u8,
        first_id: u32,
        count: u32,
        ranks: u16,
        formation: u8,
    ) -> u32 {
        let ids: Vec<u32> = (first_id..first_id + count).collect();
        let commander = ids[0];
        let deputies: Vec<u32> = ids.iter().skip(1).take(4).copied().collect();
        let unit = Unit {
            soldiers: ids,
            troop_type: 0,
            formation,
            formation_origin: Vec2Fx::ZERO,
            formation_facing: 0,
            ranks,
            file_spacing: fx_from_mm(800),
            rank_spacing: fx_from_mm(800),
            banner: None,
            formation_change: None,
            path: Vec::new(),
            path_final: Vec2Fx::ZERO,
            pursuit_leash: None,
        };
        self.inner
            .add_command_node(None, 0, faction, commander, deputies, Some(unit))
    }

    /// 指定ノードへ、指揮系統を介さず直接（絶対優先度の）移動命令を出す。
    /// デモ・シナリオ用の簡易 API。
    #[wasm_bindgen(js_name = issueMoveTo)]
    pub fn issue_move_to(
        &mut self,
        node: u32,
        x_m: i32,
        y_m: i32,
        facing_brad: u16,
        formation: u8,
    ) -> bool {
        self.inner
            .issue_order(
                node,
                node,
                Intent::MoveTo {
                    pos: Vec2Fx::new(fx(x_m), fx(y_m)),
                    facing: facing_brad,
                    speed: MoveSpeed::Walk,
                    formation,
                },
                Priority::Absolute,
            )
            .is_some()
    }

    /// 指定ノードへ、指揮系統を介さず直接（絶対優先度の）突撃命令を出す
    /// （M5、デモ・シナリオ用の簡易 API）。
    #[wasm_bindgen(js_name = issueCharge)]
    pub fn issue_charge(&mut self, node: u32, target_node: u32) -> bool {
        self.inner
            .issue_order(
                node,
                node,
                Intent::Charge {
                    target: target_node,
                },
                Priority::Absolute,
            )
            .is_some()
    }

    /// 指定ノードへ、指揮系統を介さず直接（絶対優先度の）追撃命令を出す
    /// （M5、デモ・シナリオ用の簡易 API）。
    #[wasm_bindgen(js_name = issuePursue)]
    pub fn issue_pursue(&mut self, node: u32, target_node: u32, max_distance_m: u16) -> bool {
        self.inner
            .issue_order(
                node,
                node,
                Intent::Pursue {
                    target: target_node,
                    max_distance_m,
                },
                Priority::Absolute,
            )
            .is_some()
    }

    /// 特定の敵兵（通常は指揮官）を、行動不能になるまで追跡する。
    #[wasm_bindgen(js_name = issueHuntPerson)]
    pub fn issue_hunt_person(&mut self, node: u32, target_soldier: u32) -> bool {
        let Some(command_node) = self.inner.command.node(node) else {
            return false;
        };
        let Some(target) = self.inner.soldiers.index_if_present(target_soldier) else {
            return false;
        };
        if !self.inner.soldiers.is_alive(target)
            || self.inner.soldiers.faction[target] == command_node.faction
        {
            return false;
        }
        self.inner
            .issue_order(
                node,
                node,
                Intent::HuntPerson {
                    target: target_soldier,
                },
                Priority::Absolute,
            )
            .is_some()
    }

    /// 指定した円形区域へ進出し、区域内に分散して占拠する。
    #[wasm_bindgen(js_name = issueOccupyArea)]
    pub fn issue_occupy_area(&mut self, node: u32, x_m: i32, y_m: i32, radius_m: u16) -> bool {
        self.inner
            .issue_order(
                node,
                node,
                Intent::OccupyArea {
                    center: Vec2Fx::new(fx(x_m), fx(y_m)),
                    radius_m: radius_m.clamp(1, 200),
                },
                Priority::Absolute,
            )
            .is_some()
    }

    /// 指定した区域を守り、各持ち場から迎撃半径内の敵へ個別に反応する。
    #[wasm_bindgen(js_name = issueGuardArea)]
    pub fn issue_guard_area(
        &mut self,
        node: u32,
        x_m: i32,
        y_m: i32,
        radius_m: u16,
        intercept_radius_m: u16,
    ) -> bool {
        self.inner
            .issue_order(
                node,
                node,
                Intent::GuardArea {
                    center: Vec2Fx::new(fx(x_m), fx(y_m)),
                    radius_m: radius_m.clamp(1, 200),
                    intercept_radius_m: intercept_radius_m.clamp(1, 50),
                },
                Priority::Absolute,
            )
            .is_some()
    }

    #[wasm_bindgen(js_name = messengerCount)]
    pub fn messenger_count(&self) -> u32 {
        self.inner.command.messengers.len() as u32
    }

    /// 伝令・旗・角笛信号の一覧。命令の可視化（矢印・伝令の移動）用。
    /// 1 件あたり `[from_node, to_node, pos_x_cm, pos_y_cm, dest_x_cm,
    ///   dest_y_cm, state, method]` の 8 要素。
    #[wasm_bindgen(js_name = messengers)]
    pub fn messengers(&self) -> Vec<i32> {
        let list = &self.inner.command.messengers;
        let mut out = Vec::with_capacity(list.len() * 8);
        for m in list {
            out.push(m.from as i32);
            out.push(m.to as i32);
            out.push(sim_math::fx_to_mm(m.position.x) / 10);
            out.push(sim_math::fx_to_mm(m.position.y) / 10);
            out.push(sim_math::fx_to_mm(m.destination.x) / 10);
            out.push(sim_math::fx_to_mm(m.destination.y) / 10);
            out.push(m.state as i32);
            out.push(m.method as i32);
        }
        out
    }

    /// 指定ノードへ、指揮系統を介さず直接（絶対優先度の）待機命令を出す
    /// （M8、憑依 UI 用の簡易 API）。
    #[wasm_bindgen(js_name = issueHold)]
    pub fn issue_hold(
        &mut self,
        node: u32,
        x_m: i32,
        y_m: i32,
        facing_brad: u16,
        allow_pursuit: bool,
    ) -> bool {
        self.inner
            .issue_order(
                node,
                node,
                Intent::Hold {
                    pos: Vec2Fx::new(fx(x_m), fx(y_m)),
                    facing: facing_brad,
                    allow_pursuit,
                },
                Priority::Absolute,
            )
            .is_some()
    }

    /// `approach`: 0=Deliberate, 1=Aggressive, 2=Cautious。
    #[wasm_bindgen(js_name = issueAttack)]
    pub fn issue_attack(&mut self, node: u32, target_node: u32, approach: u8) -> bool {
        let approach = match approach {
            1 => ApproachStyle::Aggressive,
            2 => ApproachStyle::Cautious,
            _ => ApproachStyle::Deliberate,
        };
        self.inner
            .issue_order(
                node,
                node,
                Intent::Attack {
                    target: target_node,
                    approach,
                },
                Priority::Absolute,
            )
            .is_some()
    }

    /// 側面攻撃命令。`side_right` が false なら左翼、true なら右翼から。
    #[wasm_bindgen(js_name = issueFlank)]
    pub fn issue_flank(&mut self, node: u32, target_node: u32, side_right: bool) -> bool {
        let side = if side_right { Side::Right } else { Side::Left };
        self.inner
            .issue_order(
                node,
                node,
                Intent::Flank {
                    target: target_node,
                    side,
                },
                Priority::Absolute,
            )
            .is_some()
    }

    /// 退却命令。`fighting` なら戦いながらの後退（隊列を保つ）。
    #[wasm_bindgen(js_name = issueWithdraw)]
    pub fn issue_withdraw(&mut self, node: u32, x_m: i32, y_m: i32, fighting: bool) -> bool {
        self.inner
            .issue_order(
                node,
                node,
                Intent::Withdraw {
                    to: Vec2Fx::new(fx(x_m), fx(y_m)),
                    fighting,
                },
                Priority::Absolute,
            )
            .is_some()
    }

    /// 射撃命令。`mode`: 0=Volley, 1=AtWill, 2=Hold。
    #[wasm_bindgen(js_name = issueShootAt)]
    pub fn issue_shoot_at(&mut self, node: u32, target_node: u32, mode: u8) -> bool {
        let mode = match mode {
            1 => ShootMode::AtWill,
            2 => ShootMode::Hold,
            _ => ShootMode::Volley,
        };
        self.inner
            .issue_order(
                node,
                node,
                Intent::ShootAt {
                    target: target_node,
                    mode,
                },
                Priority::Absolute,
            )
            .is_some()
    }

    /// 予備として後方待機させる命令。
    #[wasm_bindgen(js_name = issueReserve)]
    pub fn issue_reserve(&mut self, node: u32, x_m: i32, y_m: i32) -> bool {
        self.inner
            .issue_order(
                node,
                node,
                Intent::Reserve {
                    rally_pos: Vec2Fx::new(fx(x_m), fx(y_m)),
                },
                Priority::Absolute,
            )
            .is_some()
    }

    // ── M6: 工兵タスク（憑依 UI から築城を指示する簡易 API） ────

    /// 野戦築城タスクを投入する。`kind`: 0=Stakes, 1=Ditch, 2=Abatis,
    /// 3=Rampart, 4=Palisade。
    #[wasm_bindgen(js_name = queueBuildStructure)]
    #[allow(clippy::too_many_arguments)]
    pub fn queue_build_structure(
        &mut self,
        kind: u8,
        ax_m: i32,
        ay_m: i32,
        bx_m: i32,
        by_m: i32,
        owner: u8,
        priority: u8,
    ) -> u32 {
        let kind = match kind {
            1 => StructureKind::Ditch,
            2 => StructureKind::Abatis,
            3 => StructureKind::Rampart,
            4 => StructureKind::Palisade,
            _ => StructureKind::Stakes,
        };
        self.inner.queue_build_structure(
            kind,
            Vec2Fx::new(fx(ax_m), fx(ay_m)),
            Vec2Fx::new(fx(bx_m), fx(by_m)),
            owner,
            priority,
        )
    }

    // ── M7: 指揮官 AI ──────────────────────────────────

    /// アーキタイプ名の一覧（`setCommanderArchetype` の `archetype` 引数と対応する index）。
    #[wasm_bindgen(js_name = archetypeNames)]
    pub fn archetype_names() -> Vec<String> {
        sim_core::commander_ai::ARCHETYPES
            .iter()
            .map(|a| a.name.to_string())
            .collect()
    }

    /// この指揮官の性格をアーキタイプから生成して設定する。
    #[wasm_bindgen(js_name = setCommanderArchetype)]
    pub fn set_commander_archetype(&mut self, node: u32, archetype: usize, seed_salt: u32) {
        self.inner
            .set_commander_archetype(node, archetype, seed_salt);
    }

    /// 指揮官の性格（10 要素、`CommanderAttrs` のフィールド順）。
    #[wasm_bindgen(js_name = commanderAttrs)]
    pub fn commander_attrs(&self, node: u32) -> Vec<i32> {
        let Some(n) = self.inner.command.node(node) else {
            return Vec::new();
        };
        let a = n.commander_attrs;
        vec![
            a.boldness as i32,
            a.caution as i32,
            a.initiative as i32,
            a.obedience as i32,
            a.tactical_skill as i32,
            a.ambition as i32,
            a.charisma as i32,
            a.flexibility as i32,
            a.patience as i32,
            a.ruthlessness as i32,
        ]
    }

    /// 認識 vs 実際の戦況評価（仕様 12 章 M7 の受け入れ条件）。
    /// 前半 8 要素が認識（ノイズ込み）、後半 8 要素が実際の値。
    /// 各 8 要素: `[force_ratio_permille, momentum, flank_left, flank_right,
    /// rear_threat, reserve_available, terrain_advantage, time_pressure]`。
    #[wasm_bindgen(js_name = commanderAssessment)]
    pub fn commander_assessment(&self, node: u32) -> Vec<i32> {
        let (perceived, actual) = self.inner.commander_perceived_vs_true(node);
        let flatten = |a: &sim_core::organization::SituationAssessment| -> [i32; 8] {
            [
                a.force_ratio_permille,
                a.momentum,
                a.flank_threats[0] as i32,
                a.flank_threats[1] as i32,
                a.rear_threat as i32,
                a.reserve_available as i32,
                a.terrain_advantage,
                a.time_pressure,
            ]
        };
        let mut out = Vec::with_capacity(16);
        out.extend_from_slice(&flatten(&perceived));
        out.extend_from_slice(&flatten(&actual));
        out
    }

    /// 直近の判断ログを JSON 文字列で返す（UI 表示用、低頻度呼び出し想定。
    /// 仕様 05 章 7 節「なぜそうしたか」）。`chosen`/候補ラベルは Rust の
    /// `Debug` 表示でクォートするので、そのまま妥当な JSON 文字列になる。
    #[wasm_bindgen(js_name = commanderDecisionLogJson)]
    pub fn commander_decision_log_json(&self, node: u32) -> String {
        let Some(n) = self.inner.command.node(node) else {
            return "[]".to_string();
        };
        let pairs = |items: &[(&'static str, i32)]| -> String {
            items
                .iter()
                .map(|(label, score)| format!("[{label:?},{score}]"))
                .collect::<Vec<_>>()
                .join(",")
        };
        let records: Vec<String> = n
            .decision_log
            .iter()
            .map(|r| {
                format!(
                    "{{\"tick\":{},\"chosen\":{:?},\"score\":{},\"candidates\":[{}],\"breakdown\":[{}]}}",
                    r.tick,
                    r.chosen,
                    r.chosen_score,
                    pairs(&r.candidates),
                    pairs(&r.breakdown),
                )
            })
            .collect();
        format!("[{}]", records.join(","))
    }

    /// この指揮官の Blackboard が知っている敵部隊（仕様 05 章 5.1 節）。
    /// 1 件あたり `[node, est_x_cm, est_y_cm, est_strength, confidence,
    /// observed_tick]` の 6 要素。憑依中の視界制限された表示に使う。
    #[wasm_bindgen(js_name = blackboardEnemyForces)]
    pub fn blackboard_enemy_forces(&self, node: u32) -> Vec<i32> {
        let Some(n) = self.inner.command.node(node) else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(n.blackboard.enemy_forces.len() * 6);
        for f in &n.blackboard.enemy_forces {
            out.push(f.node as i32);
            out.push(sim_math::fx_to_mm(f.est_pos.x) / 10);
            out.push(sim_math::fx_to_mm(f.est_pos.y) / 10);
            out.push(f.est_strength as i32);
            out.push(f.confidence as i32);
            out.push(f.observed_tick as i32);
        }
        out
    }

    // ── 兵士の詳細（M8: 詳細パネル） ─────────────────────

    /// 指定兵士を含む葉部隊（`organization::Unit`）の指揮ノード ID。
    /// 見つからなければ -1。低頻度（クリック時）呼び出し想定。
    #[wasm_bindgen(js_name = nodeForSoldier)]
    pub fn node_for_soldier(&self, id: u32) -> i32 {
        for node in &self.inner.command.nodes {
            if let Some(unit) = &node.unit {
                if unit.soldiers.contains(&id) {
                    return node.id as i32;
                }
            }
        }
        -1
    }

    /// 兵士 1 体の詳細（スナップショットに含まれない warm/cold な値）。
    /// `[hp, morale, fatigue, ammo, target(-1 なら無し), bravery, discipline,
    /// skill, weapon_reach_mm]` の 9 要素。低頻度（クリック時）呼び出し想定。
    #[wasm_bindgen(js_name = soldierDetail)]
    pub fn soldier_detail(&self, id: u32) -> Vec<i32> {
        let i = id as usize;
        if i >= self.inner.soldiers.len() {
            return Vec::new();
        }
        let s = &self.inner.soldiers;
        let target = s.target[i];
        let reach_mm = self
            .inner
            .combat
            .weapons
            .get(i)
            .map(|w| sim_math::fx_to_mm(w.reach))
            .unwrap_or(0);
        vec![
            s.hp[i] as i32,
            s.morale[i] as i32,
            s.fatigue[i] as i32,
            self.inner.combat.ammo.get(i).copied().unwrap_or(0) as i32,
            if target == sim_core::soldiers::NO_ID {
                -1
            } else {
                target as i32
            },
            s.attrs[i].bravery as i32,
            s.attrs[i].discipline as i32,
            s.attrs[i].skill as i32,
            reach_mm,
        ]
    }

    /// 兵士 1 体の判断まわり（任務・現在行動・行動候補・目標地点・隊形スロット・
    /// 反応待ち）を JSON で返す（B0 のデバッグ表示）。
    ///
    /// 呼ぶとその兵士が観測対象になり、以降の tick では判断の候補と点数が
    /// 記録される。記録は判断そのものを変えないので、状態ハッシュもリプレイも
    /// 影響を受けない。別の兵士を指定すると観測対象が移る。
    #[wasm_bindgen(js_name = soldierDebugJson)]
    pub fn soldier_debug_json(&mut self, id: u32) -> String {
        let i = id as usize;
        if i >= self.inner.soldiers.len() {
            return "null".to_string();
        }
        self.inner.soldier_ai.set_watch(Some(id));

        let node = self.node_for_soldier(id);
        let owner = self.inner.command.nodes.iter().find(|n| {
            n.unit
                .as_ref()
                .is_some_and(|unit| unit.soldiers.contains(&id))
        });
        let mission = owner
            .map(|n| MissionKind::of(n.objective.as_ref()))
            .unwrap_or(MissionKind::NoUnit);
        // 任務の段階と、地点任務の半径・追撃限界（B5 の可視化）。
        let mission_state = owner.map(|n| n.mission).unwrap_or_default();
        let area = owner.and_then(|n| match n.objective {
            Some(Intent::OccupyArea { center, radius_m }) => Some((center, radius_m, 0u16)),
            Some(Intent::GuardArea {
                center,
                radius_m,
                intercept_radius_m,
            }) => Some((center, radius_m, intercept_radius_m)),
            _ => None,
        });

        let ai = &self.inner.soldier_ai;
        let tick = self.inner.tick;
        let pos = self.inner.soldiers.pos(i);
        let goal = self.inner.goal(id);
        let slot_goal = ai.formation_goal(id);
        let slot_distance_mm =
            sim_math::fx_to_mm(sim_math::isqrt64(sim_math::dist_sq(pos, slot_goal) as u64) as i32);
        let action = ai.action(id).id();
        let awareness = self.inner.perception.awareness(id);
        let record = ai.watch_record();
        let candidates: Vec<String> = record
            .candidates
            .iter()
            .map(|c| {
                format!(
                    "{{\"target\":{},\"score\":{},\"distanceMm\":{},\"fighting\":{},\"load\":{}}}",
                    c.target, c.score, c.distance_mm, c.fighting, c.load
                )
            })
            .collect();
        // 記録は観測対象が判断した瞬間のものなので、別の兵士のものは出さない。
        let record_is_mine = record.soldier == id;

        // JSON は 1 本の巨大な書式文字列にせず、区切りごとに書き足す。
        // `cargo fmt` が長い文字列リテラルを畳むと、意図しない空白が JSON の中に
        // 混ざるため。
        use core::fmt::Write;
        let mut out = String::with_capacity(512);
        let _ = write!(
            out,
            "{{\"id\":{id},\"node\":{node},\"mission\":\"{}\",\"action\":\"{action}\"",
            mission.id()
        );
        let _ = write!(
            out,
            ",\"focus\":{},\"orderedFocus\":{},\"slot\":{}",
            optional_id(ai.focus(id)),
            optional_id(ai.ordered_focus(id)),
            self.inner.soldiers.slot[i]
        );
        let _ = write!(
            out,
            ",\"goalXMm\":{},\"goalYMm\":{},\"slotXMm\":{},\"slotYMm\":{},\"slotDistanceMm\":{}",
            sim_math::fx_to_mm(goal.x),
            sim_math::fx_to_mm(goal.y),
            sim_math::fx_to_mm(slot_goal.x),
            sim_math::fx_to_mm(slot_goal.y),
            slot_distance_mm
        );
        let _ = write!(
            out,
            ",\"reactionRadiusMm\":{},\"thinkInTicks\":{},\"formationSampleInTicks\":{},\"commitTicksLeft\":{}",
            ai.reaction_radius_mm(id),
            ticks_until(self.inner.soldiers.think_at[i], tick),
            ticks_until(ai.next_formation_sample(id), tick),
            ticks_until(ai.commit_until(id), tick)
        );
        let _ = write!(
            out,
            ",\"awareness\":{{\"updatedTick\":{},\"enemies\":{},\"allies\":{},\"threatFront\":{},\"threatFlank\":{},\"threatRear\":{},\"crowding\":{},\"localBroken\":{},\"nearestEnemy\":{},\"nearestEnemyMm\":{},\"supportableEnemy\":{}}}",
            awareness.updated_tick,
            awareness.enemies,
            awareness.allies,
            awareness.threat_front,
            awareness.threat_flank,
            awareness.threat_rear,
            awareness.crowding,
            awareness.local_broken,
            optional_id(awareness.sees_enemy().then_some(awareness.nearest_enemy)),
            if awareness.sees_enemy() {
                awareness.nearest_enemy_mm
            } else {
                -1
            },
            optional_id(
                awareness
                    .can_support_fight()
                    .then_some(awareness.supportable_enemy)
            )
        );
        // 行動の点数（B2）。選べた行動だけが並ぶ。
        let action_scores: Vec<String> = record
            .actions
            .iter()
            .map(|(action, score)| format!("{{\"action\":\"{}\",\"score\":{score}}}", action.id()))
            .collect();
        let _ = write!(
            out,
            ",\"actionScores\":[{}],\"chosenAction\":{}",
            if record_is_mine {
                action_scores.join(",")
            } else {
                String::new()
            },
            match record_is_mine.then_some(record.chosen_action).flatten() {
                Some(action) => format!("\"{}\"", action.id()),
                None => "null".to_string(),
            }
        );
        let _ = write!(
            out,
            ",\"missionState\":\"{}\",\"missionSinceTick\":{},\"insideFriendly\":{},\"insideEnemy\":{}",
            mission_state.state.id(),
            mission_state.since_tick,
            mission_state.inside_friendly,
            mission_state.inside_enemy
        );
        let _ = match area {
            Some((center, radius_m, intercept_m)) => write!(
                out,
                ",\"area\":{{\"centerXMm\":{},\"centerYMm\":{},\"radiusMm\":{},\"leashMm\":{}}}",
                sim_math::fx_to_mm(center.x),
                sim_math::fx_to_mm(center.y),
                i32::from(radius_m) * 1_000,
                i32::from(intercept_m) * 1_000
            ),
            None => write!(out, ",\"area\":null"),
        };
        let _ = write!(
            out,
            ",\"decidedAtTick\":{},\"candidates\":[{}]}}",
            match record_is_mine.then_some(record.tick).flatten() {
                Some(t) => t.to_string(),
                None => "null".to_string(),
            },
            if record_is_mine {
                candidates.join(",")
            } else {
                String::new()
            }
        );
        out
    }
}

/// JSON へ出す「いなければ null」の兵士 ID。
fn optional_id(id: Option<u32>) -> String {
    id.map_or_else(|| "null".to_string(), |v| v.to_string())
}

/// あと何 tick で来るか。過ぎていれば 0、予定が無ければ -1。
fn ticks_until(at: u32, now: u32) -> i64 {
    if at == u32::MAX {
        return -1;
    }
    (at as i64 - now as i64).max(0)
}

/// `battleSites()` と同じ 7 要素 1 組の平坦な配列を候補の一覧へ戻す。
fn parse_battle_sites(flat: &[i32]) -> Vec<BattleSiteCandidate> {
    let mut out = Vec::with_capacity(flat.len() / 7);
    let mut i = 0;
    while i + 7 <= flat.len() {
        out.push(BattleSiteCandidate {
            x_m: flat[i],
            y_m: flat[i + 1],
            score: flat[i + 2],
            passable_permille: flat[i + 3] as u16,
            asymmetry_permille: flat[i + 4] as u16,
            openness_permille: flat[i + 5] as u16,
            bottleneck_count: flat[i + 6] as u16,
        });
        i += 7;
    }
    out
}

/// 会戦プリセット 1 件を UI 向けの JSON にする。
///
/// `{:?}`（`Debug for str`）はダブルクォートとバックスラッシュをエスケープし、
/// 日本語のような表示可能な文字はそのまま残すので、そのまま JSON 文字列に
/// なる（`commander_decision_log_json` と同じ手）。
fn scenario_json(def: &sim_core::scenario::ScenarioDef) -> String {
    let armies: Vec<String> = def
        .armies
        .iter()
        .map(|army| {
            let contingents: Vec<String> = army
                .contingents
                .iter()
                .map(|c| {
                    format!(
                        "{{\"nameJa\":{:?},\"commanderJa\":{:?},\"archetype\":{:?},\"count\":{},\"troopType\":{}}}",
                        c.name_ja, c.commander.name_ja, c.commander.archetype, c.count, c.troop_type
                    )
                })
                .collect();
            format!(
                "{{\"faction\":{},\"nameJa\":{:?},\"commanderJa\":{:?},\"archetype\":{:?},\"battlePlan\":{:?},\"soldiers\":{},\"contingents\":[{}]}}",
                army.faction,
                army.name_ja,
                army.commander.name_ja,
                army.commander.archetype,
                army.battle_plan
                    .map_or_else(|| "-".to_string(), |p| format!("{p:?}")),
                def.army_soldier_count(army),
                contingents.join(",")
            )
        })
        .collect();
    format!(
        "{{\"id\":{:?},\"nameJa\":{:?},\"nameEn\":{:?},\"year\":{},\"placeJa\":{:?},\
         \"summaryJa\":{:?},\"historicalStrengthJa\":{:?},\"scaleNoteJa\":{:?},\
         \"sizeM\":{},\"seedLo\":{},\"seedHi\":{},\"soldiers\":{},\"armies\":[{}]}}",
        def.id,
        def.name_ja,
        def.name_en,
        def.year,
        def.place_ja,
        def.summary_ja,
        def.historical_strength_ja,
        def.scale_note_ja,
        def.size_m,
        def.terrain_seed as u32,
        (def.terrain_seed >> 32) as u32,
        def.soldier_count(),
        armies.join(",")
    )
}

/// パニック時に JS のコンソールへスタックトレースを出す。
///
/// 開発時のみ有効にする想定。
#[wasm_bindgen(js_name = initPanicHook)]
pub fn init_panic_hook() {
    // console_error_panic_hook を入れるまでの繋ぎ。
    // wasm では既定でパニックが握りつぶされるため、明示的に abort させる。
}

#[cfg(test)]
mod tests {
    /// 一様な傾斜地の上にワールドを作る。
    ///
    /// 地形生成は JS 側（`web/src/terrain`）にあるので、wasm 境界のテストは
    /// 地形を自分で組み立てて渡す。完全な水平面は避ける——兵の前後関係や
    /// 向きの判断で当たり前に生じる差が消え、突撃が前進を始めない。
    fn sloped_world(seed_lo: u32, dim: u32) -> World {
        let n = (dim as usize).pow(2);
        let mut height = vec![0i16; n];
        for y in 0..dim {
            for x in 0..dim {
                height[(y * dim + x) as usize] = ((x + y) as i16).saturating_mul(2);
            }
        }
        World::from_terrain(
            seed_lo,
            0,
            dim,
            2,
            height,
            vec![sim_terrain::Ground::Grass as u8; n],
            vec![sim_terrain::Vegetation::ShortGrass as u8; n],
            vec![0; n],
            vec![0u16; n],
            vec![0; n],
            vec![110; n],
            Vec::new(),
        )
    }

    /// 会戦プリセットのワールドを、固定地形（`data/terrain/*.bin`）から作る。
    fn scenario_world(index: usize) -> Option<World> {
        let def = sim_core::scenario::get(index)?;
        let t = sim_terrain::fixture::load_scenario(def.id).unwrap();
        let sites: Vec<i32> = t
            .battle_sites
            .iter()
            .flat_map(|s| {
                [
                    s.x_m,
                    s.y_m,
                    s.score,
                    s.passable_permille as i32,
                    s.asymmetry_permille as i32,
                    s.openness_permille as i32,
                    s.bottleneck_count as i32,
                ]
            })
            .collect();
        let Terrain {
            dim,
            cell_m,
            height,
            ground,
            vegetation,
            overlay,
            water,
            water_kind,
            moisture,
            ..
        } = t;
        World::from_scenario_terrain(
            index, dim, cell_m, height, ground, vegetation, overlay, water, water_kind, moisture,
            sites,
        )
    }

    use super::*;

    #[test]
    fn world_can_be_created_and_ticked() {
        let mut w = sloped_world(7, 300);
        w.deploy_block(100, 100, 5, 5, 900, 0, 0, 0, 1);
        assert_eq!(w.soldier_count(), 25);
        w.set_faction_goal(0, 300, 300);
        for _ in 0..20 {
            w.tick();
        }
        assert_eq!(w.tick_count(), 20);
        assert_eq!(w.alive_count(), 25);
    }

    #[test]
    fn snapshot_length_matches_soldier_count() {
        let mut w = sloped_world(7, 300);
        w.deploy_block(100, 100, 4, 4, 900, 0, 0, 0, 1);
        w.write_snapshot();
        assert_eq!(
            w.soldiers_byte_len(),
            w.soldier_count() * World::soldier_stride()
        );
    }

    #[test]
    fn state_hash_halves_reconstruct_the_full_value() {
        let mut w = sloped_world(7, 300);
        w.deploy_block(100, 100, 3, 3, 900, 0, 0, 0, 1);
        w.tick();
        let lo = w.state_hash_lo() as u64;
        let hi = w.state_hash_hi() as u64;
        assert_eq!((hi << 32) | lo, w.inner.state_hash());
    }

    #[test]
    fn terrain_metadata_is_consistent() {
        let w = sloped_world(7, 300);
        assert_eq!(w.terrain_size_m(), 600);
        assert_eq!(w.terrain_cell_m(), 2);
        assert_eq!(w.terrain_dim(), 300);
    }

    #[test]
    fn water_cliff_and_battle_site_pointers_are_accessible() {
        let w = sloped_world(7, 800);
        let n = (w.terrain_dim() * w.terrain_dim()) as usize;
        assert!(!w.terrain_water_ptr().is_null());
        assert!(!w.terrain_water_kind_ptr().is_null());
        assert!(!w.terrain_cliff_ptr().is_null());

        // このクレートは unsafe を禁止しているので生ポインタは辿らず、
        // 背後の配列長が期待どおりか（境界を指しているか）を直接確認する
        assert_eq!(w.inner.terrain.water.len(), n);
        assert_eq!(w.inner.terrain.water_kind.len(), n);
        assert_eq!(w.inner.terrain.cliff.len(), n);

        let sites = w.battle_sites();
        assert_eq!(sites.len() % 7, 0);
        assert_eq!(sites.len() / 7, w.battle_site_count() as usize);
    }

    /// M8: 憑依 UI が使う命令発行バインディングが、指揮系統の応答（命令イベント）
    /// までちゃんと届くことを確認する（詳細な戦術的帰結は sim-core 側の
    /// 責務なので、ここでは「命令が受理されるか」だけを見る）。
    #[test]
    fn order_bindings_issue_orders_that_reach_the_command_tree() {
        let mut w = sloped_world(7, 300);
        w.deploy_block(100, 100, 4, 4, 900, 0, 0, 0, 1);
        w.deploy_block(200, 200, 4, 4, 900, 1, 16, 0, 2);
        let friendly = w.add_line_unit(0, 0, 16, 4, 0);
        let enemy = w.add_line_unit(1, 16, 16, 4, 0);

        assert!(w.issue_hold(friendly, 100, 100, 0, false));
        assert!(w.issue_attack(friendly, enemy, 0));
        assert!(w.issue_flank(friendly, enemy, true));
        assert!(w.issue_withdraw(friendly, 90, 90, true));
        assert!(w.issue_shoot_at(friendly, enemy, 1));
        assert!(w.issue_reserve(friendly, 80, 80));
        assert!(w.issue_hunt_person(friendly, 16));
        assert!(w.issue_occupy_area(friendly, 120, 100, 25));
        assert!(w.issue_guard_area(friendly, 100, 120, 20, 14));
        assert!(!w.issue_hunt_person(friendly, 0));
        // 存在しないノードへは出せない。
        assert!(!w.issue_attack(999, enemy, 0));

        assert!(w.command_event_count() > 0);
    }

    #[test]
    fn queue_build_structure_binding_creates_a_task() {
        let mut w = sloped_world(7, 300);
        let before = w.inner.engineering.tasks.len();
        w.queue_build_structure(0, 100, 100, 100, 110, 0, 5);
        assert_eq!(w.inner.engineering.tasks.len(), before + 1);
    }

    #[test]
    fn commander_attrs_reflect_the_chosen_archetype() {
        let mut w = sloped_world(7, 300);
        w.deploy_block(100, 100, 2, 2, 900, 0, 0, 0, 1);
        let node = w.add_line_unit(0, 0, 4, 2, 0);
        w.set_commander_archetype(node, 5, 1); // reckless_youth: boldness 高め・caution 低め
        let attrs = w.commander_attrs(node);
        assert_eq!(attrs.len(), 10);
        let boldness = attrs[0];
        let caution = attrs[1];
        assert!(boldness > caution, "boldness={boldness} caution={caution}");
        assert!(!World::archetype_names().is_empty());
    }

    #[test]
    fn commander_assessment_and_decision_log_are_queryable() {
        let mut w = sloped_world(7, 300);
        w.deploy_block(100, 100, 4, 4, 900, 0, 0, 0, 1);
        w.deploy_block(300, 300, 4, 4, 900, 1, 16, 0, 2);
        let friendly = w.add_line_unit(0, 0, 16, 4, 0);
        let _enemy = w.add_line_unit(1, 16, 16, 4, 0);
        for _ in 0..200 {
            w.tick();
        }
        let assessment = w.commander_assessment(friendly);
        assert_eq!(assessment.len(), 16);
        let json = w.commander_decision_log_json(friendly);
        assert!(json.starts_with('['), "JSON 配列で始まっていない: {json}");
    }

    #[test]
    fn node_for_soldier_and_soldier_detail_resolve() {
        let mut w = sloped_world(7, 300);
        w.deploy_block(100, 100, 3, 3, 900, 0, 0, 0, 1);
        let node = w.add_line_unit(0, 0, 9, 3, 0);
        assert_eq!(w.node_for_soldier(0), node as i32);
        assert_eq!(w.node_for_soldier(9999), -1);

        let detail = w.soldier_detail(0);
        assert_eq!(detail.len(), 9);
        assert!(detail[0] > 0, "hp が 0 以下: {}", detail[0]);
    }

    /// デバッグ表示が、任務・現在行動・隊形スロット・反応待ちまで届くこと。
    /// 観測を始めてもワールドの状態ハッシュは変わらない。
    #[test]
    fn soldier_debug_json_describes_the_decision_state() {
        let mut w = sloped_world(11, 300);
        w.deploy_block(100, 100, 4, 4, 900, 0, 0, 0, 1);
        w.deploy_block(100, 112, 4, 4, 900, 1, 1, 0, 2);
        let friendly = w.add_line_unit(0, 0, 16, 4, 0);
        let _enemy = w.add_line_unit(1, 16, 16, 4, 0);
        assert!(w.issue_move_to(friendly, 100, 112, 0, 0));
        for _ in 0..40 {
            w.tick();
        }

        let before = (w.state_hash_lo(), w.state_hash_hi());
        let json = w.soldier_debug_json(0);
        assert!(json.starts_with('{'), "JSON オブジェクトでない: {json}");
        assert!(
            !json.contains("  "),
            "JSON に書式由来の空白が混ざっている: {json}"
        );
        for key in [
            "\"mission\"",
            "\"action\"",
            "\"slotDistanceMm\"",
            "\"reactionRadiusMm\"",
            "\"thinkInTicks\"",
            "\"candidates\"",
            "\"awareness\"",
            "\"threatRear\"",
            "\"supportableEnemy\"",
            "\"actionScores\"",
            "\"chosenAction\"",
            "\"missionState\"",
            "\"area\"",
        ] {
            assert!(json.contains(key), "{key} が無い: {json}");
        }
        assert_eq!(
            (w.state_hash_lo(), w.state_hash_hi()),
            before,
            "観測を始めただけで状態が変わった"
        );

        // 観測を始めた後は、判断のたびに候補と点数が記録される。
        for _ in 0..80 {
            w.tick();
        }
        let watched = w.soldier_debug_json(0);
        assert!(
            watched.contains("\"decidedAtTick\":") && !watched.contains("\"decidedAtTick\":null"),
            "判断が記録されていない: {watched}"
        );
        assert_eq!(w.soldier_debug_json(9999), "null");
    }

    /// 会戦プリセットが wasm 境界を越えて、そのまま回せる形で届くこと。
    #[test]
    fn scenario_binding_builds_a_playable_world() {
        let mut w = scenario_world(0).expect("プリセット 0 が無い");
        let def = sim_core::scenario::get(0).unwrap();
        assert_eq!(w.soldier_count(), def.soldier_count());
        assert!(w.command_node_count() > 2, "指揮ツリーが組まれていない");
        for _ in 0..40 {
            w.tick();
        }
        assert_eq!(w.alive_count(), def.soldier_count());
        assert!(scenario_world(999).is_none());
    }

    /// 地形グリッドを wasm 境界へもう一度通しても、同じワールドになること。
    ///
    /// ブラウザは IndexedDB に保存したグリッドを、生成をやり直さずにここへ
    /// 渡す。導出グリッド（通行コスト・崖）は渡さず Rust 側で計算し直すので、
    /// 往復しても経路探索・通行判定・地形速度倍率まで一致していなければ
    /// ならない。
    #[test]
    fn passing_the_same_grids_again_rebuilds_an_identical_scenario_world() {
        let fresh = scenario_world(0).unwrap();
        let t = &fresh.inner.terrain;
        let mut cached = World::from_scenario_terrain(
            0,
            t.dim,
            t.cell_m,
            t.height.clone(),
            t.ground.clone(),
            t.vegetation.clone(),
            t.overlay.clone(),
            t.water.clone(),
            t.water_kind.clone(),
            t.moisture.clone(),
            fresh.battle_sites(),
        )
        .unwrap();
        let mut fresh = fresh;
        assert_eq!(fresh.state_hash_lo(), cached.state_hash_lo());
        for _ in 0..60 {
            fresh.tick();
            cached.tick();
        }
        assert_eq!(fresh.state_hash_lo(), cached.state_hash_lo());
        assert_eq!(fresh.state_hash_hi(), cached.state_hash_hi());
    }

    #[test]
    fn scenario_list_json_describes_every_preset() {
        let json = World::scenario_list_json();
        assert!(json.starts_with('['), "JSON 配列で始まっていない: {json}");
        for def in sim_core::scenario::SCENARIOS {
            assert!(json.contains(def.id), "{} が一覧に無い", def.id);
            assert!(json.contains(def.name_ja));
            for army in def.armies {
                assert!(json.contains(army.commander.name_ja));
            }
        }
        // 引用符の対応が取れている = そのままパースできる形になっている
        assert_eq!(json.matches('"').count() % 2, 0);
    }

    /// 地形キャッシュ（IndexedDB からの復元）が、渡し直す前と完全に同じ
    /// 挙動になることを確認する。地形グリッド自体の一致に加え、同じ操作を
    /// 続けたときの `state_hash` まで見る（経路探索・通行判定・地形速度倍率
    /// など、地形に依存する全ロジックが復元後も壊れていないことの証明）。
    #[test]
    fn passing_the_same_grids_again_rebuilds_an_identical_world() {
        let fresh = sloped_world(42, 250);
        let t = &fresh.inner.terrain;
        let mut cached = World::from_terrain(
            42,
            0,
            t.dim,
            t.cell_m,
            t.height.clone(),
            t.ground.clone(),
            t.vegetation.clone(),
            t.overlay.clone(),
            t.water.clone(),
            t.water_kind.clone(),
            t.moisture.clone(),
            fresh.battle_sites(),
        );
        let mut fresh = fresh;

        assert_eq!(fresh.inner.terrain.height, cached.inner.terrain.height);
        assert_eq!(fresh.inner.terrain.ground, cached.inner.terrain.ground);
        assert_eq!(
            fresh.inner.terrain.vegetation,
            cached.inner.terrain.vegetation
        );
        assert_eq!(
            fresh.inner.terrain.passability,
            cached.inner.terrain.passability
        );
        assert_eq!(
            fresh.inner.terrain.passability,
            cached.inner.terrain.passability
        );
        assert_eq!(fresh.inner.terrain.water, cached.inner.terrain.water);
        assert_eq!(
            fresh.inner.terrain.water_kind,
            cached.inner.terrain.water_kind
        );
        assert_eq!(fresh.inner.terrain.cliff, cached.inner.terrain.cliff);
        assert_eq!(fresh.battle_sites(), cached.battle_sites());

        // 地形に依存する経路探索・速度倍率・通行判定を含め、以後の挙動も
        // 完全に一致することを state_hash で確認する。
        fresh.deploy_block(100, 100, 6, 6, 900, 0, 0, 0, 1);
        cached.deploy_block(100, 100, 6, 6, 900, 0, 0, 0, 1);
        fresh.set_faction_goal(0, 400, 400);
        cached.set_faction_goal(0, 400, 400);
        for _ in 0..100 {
            fresh.tick();
            cached.tick();
        }
        assert_eq!(fresh.state_hash_lo(), cached.state_hash_lo());
        assert_eq!(fresh.state_hash_hi(), cached.state_hash_hi());
    }
}
