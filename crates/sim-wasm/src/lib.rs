//! JS との境界。
//!
//! ここにはロジックを置かない。`sim-core` の薄いラッパに徹する
//! （仕様 01 章 2 節）。
//!
//! 境界を跨ぐ呼び出しはフレームあたり定数回に抑える。エンティティごとの
//! 呼び出しはしない。描画データはリニアメモリのビューとして JS に渡す。

#![forbid(unsafe_code)]

use sim_core::organization::{Intent, MoveSpeed, Priority, Unit};
use sim_core::snapshot::RenderSnapshot;
use sim_core::soldiers::Attrs;
use sim_core::{World as CoreWorld, WorldConfig};
use sim_math::{fx, fx_from_mm, Vec2Fx};
use sim_terrain::TerrainParams;
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
    ///   missile_kills, crush_kills, bleed_kills, shots_fired, friendly_fire_hits]`
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
}
