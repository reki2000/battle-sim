//! JS との境界。
//!
//! ここにはロジックを置かない。`sim-core` の薄いラッパに徹する
//! （仕様 01 章 2 節）。
//!
//! 境界を跨ぐ呼び出しはフレームあたり定数回に抑える。エンティティごとの
//! 呼び出しはしない。描画データはリニアメモリのビューとして JS に渡す。

#![forbid(unsafe_code)]

use sim_core::organization::{ApproachStyle, Intent, MoveSpeed, Priority, ShootMode, Side, Unit};
use sim_core::snapshot::RenderSnapshot;
use sim_core::soldiers::Attrs;
use sim_core::structures::StructureKind;
use sim_core::{World as CoreWorld, WorldConfig};
use sim_math::{fx, fx_from_mm, Vec2Fx};
use sim_terrain::battle_site::BattleSiteCandidate;
use sim_terrain::{Terrain, TerrainParams, WaterKind};
use wasm_bindgen::prelude::*;

/// wasm 側のワールドハンドル。
#[wasm_bindgen]
pub struct World {
    inner: CoreWorld,
    snapshot: RenderSnapshot,
}

#[wasm_bindgen]
impl World {
    /// 地形を生成してワールドを作る。
    #[wasm_bindgen(constructor)]
    pub fn new(seed_lo: u32, seed_hi: u32, size_m: u32, relief: u16) -> World {
        let seed = ((seed_hi as u64) << 32) | seed_lo as u64;
        let config = WorldConfig {
            seed,
            terrain: TerrainParams {
                seed,
                size_m,
                relief,
                ..Default::default()
            },
        };
        World {
            inner: CoreWorld::new(&config),
            snapshot: RenderSnapshot::default(),
        }
    }

    /// キャッシュ済みの地形グリッドからワールドを作る（M9: 地形キャッシュ、
    /// IndexedDB に保存された前回生成分を再利用し、地形生成をスキップする）。
    ///
    /// 各グリッドは `terrainDim()` × `terrainDim()`（行優先）で、既存の
    /// `terrain*Ptr()` 系バインディングが指す先と同じレイアウト。
    /// `battleSitesFlat` は `battleSites()` と同じ 7 要素 1 組の平坦な配列。
    /// 呼び出し側は、これらが同じ (seed, sizeM, relief) から生成されたもので
    /// あることを保証すること（ここでは検証しない）。
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = fromCachedTerrain)]
    pub fn from_cached_terrain(
        seed_lo: u32,
        seed_hi: u32,
        dim: u32,
        cell_m: u32,
        height: Vec<i16>,
        surface: Vec<u8>,
        passability: Vec<u8>,
        water: Vec<u8>,
        water_kind: Vec<u8>,
        cliff: Vec<u8>,
        battle_sites_flat: Vec<i32>,
    ) -> World {
        let seed = ((seed_hi as u64) << 32) | seed_lo as u64;
        let mut battle_sites = Vec::with_capacity(battle_sites_flat.len() / 7);
        let mut i = 0;
        while i + 7 <= battle_sites_flat.len() {
            battle_sites.push(BattleSiteCandidate {
                x_m: battle_sites_flat[i],
                y_m: battle_sites_flat[i + 1],
                score: battle_sites_flat[i + 2],
                passable_permille: battle_sites_flat[i + 3] as u16,
                asymmetry_permille: battle_sites_flat[i + 4] as u16,
                openness_permille: battle_sites_flat[i + 5] as u16,
                bottleneck_count: battle_sites_flat[i + 6] as u16,
            });
            i += 7;
        }
        let terrain = Terrain {
            dim,
            cell_m,
            seed,
            height,
            surface,
            passability,
            water,
            water_kind: water_kind.iter().map(|&b| WaterKind::from_u8(b)).collect(),
            cliff,
            battle_sites,
        };
        World {
            inner: CoreWorld::with_terrain(seed, terrain),
            snapshot: RenderSnapshot::default(),
        }
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

    // ── 配置（M3 でシナリオ読み込みに置き換わる） ──────────

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

    /// 1 体だけ追加する（デバッグ用）。
    #[wasm_bindgen(js_name = spawnOne)]
    pub fn spawn_one(&mut self, x_m: i32, y_m: i32, faction: u8, unit_id: u16) -> u32 {
        self.inner.spawn(
            Vec2Fx::new(fx(x_m), fx(y_m)),
            0,
            unit_id,
            faction,
            Attrs::default(),
            0,
        )
    }

    /// 騎乗した兵士を 1 体追加する（M5、デバッグ用）。
    #[wasm_bindgen(js_name = spawnCavalryOne)]
    pub fn spawn_cavalry_one(&mut self, x_m: i32, y_m: i32, faction: u8, unit_id: u16) -> u32 {
        self.inner.spawn(
            Vec2Fx::new(fx(x_m), fx(y_m)),
            0,
            unit_id,
            faction,
            Attrs::default(),
            sim_core::soldiers::flags::MOUNTED,
        )
    }

    /// ある陣営の全兵士に移動目標を与える。
    ///
    /// M3 で指揮系統を通す命令 API（`pushOrder`）に置き換わる。
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

    #[wasm_bindgen(js_name = terrainSurfacePtr)]
    pub fn terrain_surface_ptr(&self) -> *const u8 {
        self.inner.terrain.surface.as_ptr()
    }

    #[wasm_bindgen(js_name = terrainHeightPtr)]
    pub fn terrain_height_ptr(&self) -> *const i16 {
        self.inner.terrain.height.as_ptr()
    }

    #[wasm_bindgen(js_name = terrainPassabilityPtr)]
    pub fn terrain_passability_ptr(&self) -> *const u8 {
        self.inner.terrain.passability.as_ptr()
    }

    /// 水深グリッド（10 cm 単位、0 = 陸地）。仕様 08 章の水面描画で使う。
    #[wasm_bindgen(js_name = terrainWaterPtr)]
    pub fn terrain_water_ptr(&self) -> *const u8 {
        self.inner.terrain.water.as_ptr()
    }

    /// 水域種別グリッド（[`sim_terrain::WaterKind`] の discriminant）。
    #[wasm_bindgen(js_name = terrainWaterKindPtr)]
    pub fn terrain_water_kind_ptr(&self) -> *const u8 {
        self.inner.terrain.water_kind.as_ptr() as *const u8
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
    use super::*;

    #[test]
    fn world_can_be_created_and_ticked() {
        let mut w = World::new(7, 0, 600, 200);
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
        let mut w = World::new(7, 0, 600, 200);
        w.deploy_block(100, 100, 4, 4, 900, 0, 0, 0, 1);
        w.write_snapshot();
        assert_eq!(
            w.soldiers_byte_len(),
            w.soldier_count() * World::soldier_stride()
        );
    }

    #[test]
    fn state_hash_halves_reconstruct_the_full_value() {
        let mut w = World::new(7, 0, 600, 200);
        w.deploy_block(100, 100, 3, 3, 900, 0, 0, 0, 1);
        w.tick();
        let lo = w.state_hash_lo() as u64;
        let hi = w.state_hash_hi() as u64;
        assert_eq!((hi << 32) | lo, w.inner.state_hash());
    }

    #[test]
    fn terrain_metadata_is_consistent() {
        let w = World::new(7, 0, 600, 200);
        assert_eq!(w.terrain_size_m(), 600);
        assert_eq!(w.terrain_cell_m(), 2);
        assert_eq!(w.terrain_dim(), 300);
    }

    #[test]
    fn water_cliff_and_battle_site_pointers_are_accessible() {
        let w = World::new(7, 0, 1600, 700);
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
        let mut w = World::new(7, 0, 600, 200);
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
        // 存在しないノードへは出せない。
        assert!(!w.issue_attack(999, enemy, 0));

        assert!(w.command_event_count() > 0);
    }

    #[test]
    fn queue_build_structure_binding_creates_a_task() {
        let mut w = World::new(7, 0, 600, 200);
        let before = w.inner.engineering.tasks.len();
        w.queue_build_structure(0, 100, 100, 100, 110, 0, 5);
        assert_eq!(w.inner.engineering.tasks.len(), before + 1);
    }

    #[test]
    fn commander_attrs_reflect_the_chosen_archetype() {
        let mut w = World::new(7, 0, 600, 200);
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
        let mut w = World::new(7, 0, 600, 200);
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
        let mut w = World::new(7, 0, 600, 200);
        w.deploy_block(100, 100, 3, 3, 900, 0, 0, 0, 1);
        let node = w.add_line_unit(0, 0, 9, 3, 0);
        assert_eq!(w.node_for_soldier(0), node as i32);
        assert_eq!(w.node_for_soldier(9999), -1);

        let detail = w.soldier_detail(0);
        assert_eq!(detail.len(), 9);
        assert!(detail[0] > 0, "hp が 0 以下: {}", detail[0]);
    }

    /// M9: 地形キャッシュ（IndexedDB からの復元）が、再生成した地形と
    /// 完全に同じ挙動になることを確認する。地形グリッド自体の一致に加え、
    /// 同じ操作を続けたときの `state_hash` まで一致することまで見る
    /// （経路探索・通行判定・地形速度倍率など、地形に依存する全ロジックが
    /// 復元後も壊れていないことの証明）。
    #[test]
    fn from_cached_terrain_matches_a_freshly_generated_world() {
        let fresh = World::new(42, 0, 500, 300);
        let t = &fresh.inner.terrain;
        let mut cached = World::from_cached_terrain(
            42,
            0,
            t.dim,
            t.cell_m,
            t.height.clone(),
            t.surface.clone(),
            t.passability.clone(),
            t.water.clone(),
            t.water_kind.iter().map(|k| *k as u8).collect(),
            t.cliff.clone(),
            fresh.battle_sites(),
        );
        let mut fresh = fresh;

        assert_eq!(fresh.inner.terrain.height, cached.inner.terrain.height);
        assert_eq!(fresh.inner.terrain.surface, cached.inner.terrain.surface);
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
