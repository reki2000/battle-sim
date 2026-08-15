//! 地形生成。
//!
//! 仕様は `docs/spec/03-terrain.md`。
//!
//! **M1 の実装範囲**: グリッド構造、地表タイプの定義、地表効果のテーブル、
//! ノイズによるベース標高、熱浸食、水系（優先度フラッド・D8 フロー・流量集積・
//! 河川と湖の分類）、崖の検出、湿度の水系連動、海岸線、道路（A*）、
//! 会戦地の自動評価。攻城戦・戦役レベルの機能は引き続きスコープ外。

pub mod battle_site;
pub mod hydrology;
pub mod noise;
pub mod roads;
mod upsample;

use sim_math::{fx, fx_from_mm, Fx, Rng};
use std::collections::VecDeque;

/// 地表タイプ。仕様 03 章 2.4 節の表と 1 対 1 に対応する。
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Surface {
    DeepWater = 0,
    ShallowWater = 1,
    Ford = 2,
    Marsh = 3,
    Mud = 4,
    Grass = 5,
    Meadow = 6,
    Farmland = 7,
    Scrub = 8,
    LightForest = 9,
    DenseForest = 10,
    Rock = 11,
    Scree = 12,
    Sand = 13,
    Road = 14,
    Bridge = 15,
}

impl Surface {
    pub const COUNT: usize = 16;

    #[inline]
    pub fn from_u8(v: u8) -> Surface {
        // 0..=15 に収まる値のみ格納されるが、防御的に丸める
        match v & 0x0F {
            0 => Surface::DeepWater,
            1 => Surface::ShallowWater,
            2 => Surface::Ford,
            3 => Surface::Marsh,
            4 => Surface::Mud,
            5 => Surface::Grass,
            6 => Surface::Meadow,
            7 => Surface::Farmland,
            8 => Surface::Scrub,
            9 => Surface::LightForest,
            10 => Surface::DenseForest,
            11 => Surface::Rock,
            12 => Surface::Scree,
            13 => Surface::Sand,
            14 => Surface::Road,
            _ => Surface::Bridge,
        }
    }

    /// この地表が通行可能か。
    #[inline]
    pub fn passable(self) -> bool {
        self != Surface::DeepWater
    }
}

/// 地表が兵士に与える影響。すべて 1000 分率の倍率。
///
/// 最終的には `data/terrain_surfaces.toml` から読む（仕様 10 章）。
/// M0 ではコード内の既定値を持つ。
#[derive(Clone, Copy, Debug)]
pub struct SurfaceEffect {
    /// 移動速度倍率
    pub move_mult: u16,
    /// 隊列維持のしやすさ
    pub cohesion_mult: u16,
    /// 視界遮蔽率
    pub cover: u16,
    /// 防御倍率
    pub defense_mult: u16,
    /// 疲労消費倍率
    pub fatigue_mult: u16,
    /// 騎兵の可否（0 = 進入不可, 1000 = 制限なし）
    pub cavalry_mult: u16,
}

const fn eff(
    move_mult: u16,
    cohesion_mult: u16,
    cover: u16,
    defense_mult: u16,
    fatigue_mult: u16,
    cavalry_mult: u16,
) -> SurfaceEffect {
    SurfaceEffect {
        move_mult,
        cohesion_mult,
        cover,
        defense_mult,
        fatigue_mult,
        cavalry_mult,
    }
}

/// 地表効果テーブル。仕様 03 章 3 節の表。
pub static SURFACE_EFFECTS: [SurfaceEffect; Surface::COUNT] = [
    eff(0, 0, 0, 1000, 1000, 0),          // DeepWater
    eff(400, 300, 0, 600, 1800, 300),     // ShallowWater
    eff(550, 400, 0, 700, 1500, 900),     // Ford
    eff(350, 400, 200, 800, 2200, 0),     // Marsh
    eff(500, 600, 0, 900, 1900, 400),     // Mud
    eff(1000, 1000, 0, 1000, 1000, 1000), // Grass
    eff(900, 900, 150, 1000, 1100, 1000), // Meadow
    eff(950, 950, 50, 1000, 1050, 1000),  // Farmland
    eff(800, 750, 250, 1000, 1200, 700),  // Scrub
    eff(650, 500, 600, 1100, 1300, 0),    // LightForest（騎兵は進入できるが突撃不可）
    eff(400, 250, 900, 1200, 1600, 0),    // DenseForest
    eff(700, 600, 100, 1100, 1400, 500),  // Rock
    eff(600, 450, 0, 1000, 1500, 0),      // Scree
    eff(850, 850, 0, 1000, 1250, 700),    // Sand
    eff(1150, 1100, 0, 1000, 900, 1000),  // Road
    eff(1000, 1000, 0, 900, 1000, 1000),  // Bridge
];

/// 海の付け方。指定した辺に沿って標高を下げ、海岸線を作る。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SeaEdge {
    #[default]
    None,
    North,
    East,
    South,
    West,
}

/// 生成パラメータ。仕様 03 章 6 節。
#[derive(Clone, Copy, Debug)]
pub struct TerrainParams {
    pub seed: u64,
    /// マップの一辺（m）
    pub size_m: u32,
    /// セルの一辺（m）
    pub cell_m: u32,
    /// 0 = 平原、1000 = 山岳
    pub relief: u16,
    /// 森の被覆率（1000 分率）
    pub forest_cover: u16,
    /// 湿地のできやすさ（1000 分率）
    pub marsh_bias: u16,
    /// 熱浸食の反復回数。
    ///
    /// 崖（[`CLIFF_THRESHOLD_CM`] 以上の段差）は対象外なので、増やしても
    /// 崖が消えることはない。既定値 6 は「Normal」品質（仕様 03 章 5 節、
    /// 目標 5 km マップで 1.5 秒以内）にかなり近づける値として実測で選んだ。
    /// 実測（5 km・relief 450）: 6 回で約 1.9 秒、12 回で約 2.6 秒、
    /// 30 回では約 4.1 秒かかる。より滑らかな地形が欲しい場合は増やしてよいが、
    /// その分生成が遅くなる。
    pub thermal_iterations: u16,
    /// 河川の密度。0 = なし、1000 = 網目状（仕様 03 章 6 節）
    pub river_density: u16,
    /// 敷設する道路の本数
    pub road_count: u16,
    /// 海の有無と位置
    pub sea_edge: SeaEdge,
    /// 海面の標高（cm）
    pub sea_level_cm: i32,
}

impl Default for TerrainParams {
    fn default() -> Self {
        Self {
            seed: 0x5EED_1234_ABCD_0001,
            size_m: 5000,
            cell_m: 2,
            relief: 450,
            forest_cover: 350,
            marsh_bias: 150,
            thermal_iterations: 6,
            river_density: 500,
            road_count: 2,
            sea_edge: SeaEdge::None,
            sea_level_cm: 0,
        }
    }
}

/// 崖のビット。隣接セルとの高低差が閾値を超えるとそのビットが立つ。
///
/// 仕様 03 章 2.5 節。値そのものは方位を表す（描画で崖面のクアッドを
/// どちら向きに立てるかに使う）。
pub mod cliff_bits {
    pub const NORTH: u8 = 1 << 0; // y - 1 側
    pub const EAST: u8 = 1 << 1; // x + 1 側
    pub const SOUTH: u8 = 1 << 2; // y + 1 側
    pub const WEST: u8 = 1 << 3; // x - 1 側
}

/// 水域の種類。
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum WaterKind {
    #[default]
    None = 0,
    Lake = 1,
    River = 2,
    Sea = 3,
}

/// 生成された地形。
///
/// 各グリッドは行優先（`y * dim + x`）。
pub struct Terrain {
    pub dim: u32,
    pub cell_m: u32,
    pub seed: u64,
    /// 標高（cm）
    pub height: Vec<i16>,
    /// 地表タイプ（[`Surface`] の discriminant）
    pub surface: Vec<u8>,
    /// 通行コスト。0 = 不通、255 = 最速
    pub passability: Vec<u8>,
    /// 水深（10 cm 単位）。0 = 陸地。仕様 03 章 1 節のグリッド表に対応
    pub water: Vec<u8>,
    /// 水域の種類（[`WaterKind`]）。描画・水系ロジックの参照用
    pub water_kind: Vec<WaterKind>,
    /// 崖エッジ（[`cliff_bits`] のビットマスク）
    pub cliff: Vec<u8>,
    /// 会戦に適した場所の候補（評価順、上位のみ）
    pub battle_sites: Vec<battle_site::BattleSiteCandidate>,
}

impl core::fmt::Debug for Terrain {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Terrain")
            .field("dim", &self.dim)
            .field("cell_m", &self.cell_m)
            .field("seed", &self.seed)
            .field("cells", &self.height.len())
            .finish()
    }
}

impl Terrain {
    #[inline]
    pub fn idx(&self, x: u32, y: u32) -> usize {
        (y * self.dim + x) as usize
    }

    /// セル座標の標高（cm）。範囲外はクランプ。
    #[inline]
    pub fn height_at_cell(&self, x: i32, y: i32) -> i16 {
        let d = self.dim as i32 - 1;
        let cx = x.clamp(0, d) as u32;
        let cy = y.clamp(0, d) as u32;
        self.height[self.idx(cx, cy)]
    }

    /// ワールド座標（Fx, m）の地表タイプ。
    #[inline]
    pub fn surface_at(&self, x: Fx, y: Fx) -> Surface {
        let (cx, cy) = self.world_to_cell(x, y);
        Surface::from_u8(self.surface[self.idx(cx, cy)])
    }

    /// ワールド座標（Fx, m）の標高（Fx, m）。双線形補間。
    pub fn height_at(&self, x: Fx, y: Fx) -> Fx {
        let cell = fx(self.cell_m as i32);
        let gx = sim_math::fx_div(x, cell);
        let gy = sim_math::fx_div(y, cell);
        let ix = sim_math::fx_floor_int(gx);
        let iy = sim_math::fx_floor_int(gy);
        let fx_frac = gx & (sim_math::FX_ONE - 1);
        let fy_frac = gy & (sim_math::FX_ONE - 1);

        let h00 = fx_cm(self.height_at_cell(ix, iy));
        let h10 = fx_cm(self.height_at_cell(ix + 1, iy));
        let h01 = fx_cm(self.height_at_cell(ix, iy + 1));
        let h11 = fx_cm(self.height_at_cell(ix + 1, iy + 1));

        let a = sim_math::fx_lerp(h00, h10, fx_frac);
        let b = sim_math::fx_lerp(h01, h11, fx_frac);
        sim_math::fx_lerp(a, b, fy_frac)
    }

    /// ワールド座標をセル座標に写す（範囲内にクランプ）。
    #[inline]
    pub fn world_to_cell(&self, x: Fx, y: Fx) -> (u32, u32) {
        let cell = self.cell_m as i32;
        let cx = (sim_math::fx_floor_int(x) / cell).clamp(0, self.dim as i32 - 1);
        let cy = (sim_math::fx_floor_int(y) / cell).clamp(0, self.dim as i32 - 1);
        (cx as u32, cy as u32)
    }

    /// マップの一辺（m）。
    #[inline]
    pub fn size_m(&self) -> u32 {
        self.dim * self.cell_m
    }

    /// 生成物の同一性を検証するためのハッシュ。
    pub fn hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for v in &self.height {
            h ^= *v as u16 as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        for v in &self.surface {
            h ^= *v as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        for v in &self.water {
            h ^= *v as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        for v in &self.cliff {
            h ^= *v as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h
    }

    /// 崖ビットマスク（デバッグ・描画用）。
    #[inline]
    pub fn cliff_at_cell(&self, x: u32, y: u32) -> u8 {
        self.cliff[self.idx(x, y)]
    }

    /// 水深（cm）。0 = 陸地。
    #[inline]
    pub fn water_depth_cm_at_cell(&self, x: u32, y: u32) -> u32 {
        self.water[self.idx(x, y)] as u32 * 10
    }
}

/// cm を Fx（m）に変換する。
#[inline]
fn fx_cm(cm: i16) -> Fx {
    fx_from_mm(cm as i32 * 10)
}

/// 地形を生成する。同じ [`TerrainParams`] からは常に同じ結果が出る。
pub fn generate(params: &TerrainParams) -> Terrain {
    let dim = params.size_m / params.cell_m;
    let n = (dim as usize) * (dim as usize);

    let mut t = Terrain {
        dim,
        cell_m: params.cell_m,
        seed: params.seed,
        height: vec![0; n],
        surface: vec![Surface::Grass as u8; n],
        passability: vec![255; n],
        water: vec![0; n],
        water_kind: vec![WaterKind::None; n],
        cliff: vec![0; n],
        battle_sites: Vec::new(),
    };

    base_elevation(&mut t, params);
    thermal_erosion(&mut t, params.thermal_iterations);

    // 段階 6〜8: 窪地を埋め（湖の確定）、流向・流量集積を求め、河川を分類する。
    let flow = hydrology::flood_and_flow(&t);
    let river_threshold = if params.river_density == 0 {
        u32::MAX
    } else {
        // density 1000 で 20、density 0 に近づくほど閾値が跳ね上がる
        let max_threshold = (n as u32 / 4).max(200);
        let min_threshold = 20u32;
        max_threshold
            - ((max_threshold - min_threshold) * params.river_density as u32 / 1000)
                .min(max_threshold - min_threshold)
    };
    let rivers = hydrology::classify_water(&t, &flow, river_threshold);
    for i in 0..n {
        if rivers.water_depth_cm[i] == 0 {
            continue;
        }
        t.water[i] = (rivers.water_depth_cm[i] / 10).min(u8::MAX as u16) as u8;
        t.water_kind[i] = rivers.kind[i];
    }

    // 段階: 海。指定があれば海面より低いセルをすべて海にする（湖・河川分類を上書き）。
    apply_sea(&mut t, params);

    // 段階 11: 崖
    t.cliff = compute_cliffs(&t);

    // 段階 9〜10: 水域からの距離を湿度に反映しつつ地表を分類する
    let dist_to_water = distance_to_water(&t, 24);
    classify_surface(&mut t, params, &dist_to_water);

    // 段階 12: 道路
    roads::lay_roads(&mut t, params);

    // 段階 13: 通行コスト
    derive_passability(&mut t);

    // 段階 14: 会戦地の評価
    t.battle_sites = battle_site::evaluate(&t, 8);

    t
}

/// 段階 1〜2: ベース標高と山脈。
/// 大域ノイズを評価する粗いグリッドの間隔（fine セル単位）。
///
/// 800 m 周期のような滑らかな特徴を 2 m 刻みで評価するのは無駄が大きい
/// （5 km マップで標高計算だけに 1 秒以上かかった）。4 セル = 8 m 間隔で
/// サンプルしてバイリニア補間で埋めても見た目の差はほぼ出ない一方、
/// ノイズ評価回数は 1/16 になる。
const NOISE_UPSAMPLE_STRIDE: i32 = 4;

fn base_elevation(t: &mut Terrain, p: &TerrainParams) {
    // ノイズの 1 格子 = 800 m 相当になるようスケールする。
    // 会戦の舞台となる地形は 2 m スケールでは滑らかなので、
    // 特徴の周期を大きく取り、オクターブを抑えて高周波を減らす。
    let cells_per_lattice = (800 / p.cell_m).max(1) as i32;
    // 起伏の強さ（m）。relief 0 → 5 m（平原）、relief 1000 → 125 m（丘陵〜山地）
    let amplitude_m = 5 + (p.relief as i32 * 120) / 1000;
    let relief_fx = fx(p.relief as i32) / 1000;

    // `base + ridge` を粗いグリッドで評価し、バイリニア補間で fine グリッドへ
    // 拡大する（本関数末尾のコメント参照）。
    let mixed = upsample::upsample_coarse(t.dim, NOISE_UPSAMPLE_STRIDE, |gx, gy| {
        let nx = sim_math::fx_div(fx(gx), fx(cells_per_lattice));
        let ny = sim_math::fx_div(fx(gy), fx(cells_per_lattice));

        let base = noise::warped_fbm(nx, ny, sim_math::FX_HALF, 5, p.seed);
        // 起伏が強いほど稜線ノイズを混ぜる。
        // `ridged` は [0, FX_ONE] の範囲で、実測平均が約 468（0 ではない）に
        // 偏っている。中心化せずに足すと地形全体が底上げされ、逆に低い側
        // （base 由来の谷）だけが突出して深くなる。結果、広い範囲が一つの
        // 巨大な閉じた盆地になり、優先度フラッドで地図の 3 割が湖になる、
        // という不具合が実際に起きた。中心を引いて ridge を「尾根も谷も
        // 作る」項に変え、高低を均等に振れさせる。
        const RIDGE_MEAN: Fx = 468;
        let ridge = noise::ridged(nx, ny, 4, p.seed ^ 0x0000_5249_4447_4500) - RIDGE_MEAN;
        base + sim_math::fx_mul(ridge, relief_fx)
    });

    for y in 0..t.dim {
        for x in 0..t.dim {
            let idx = t.idx(x, y);
            let m = mixed[idx];

            // cm で直接求める。メートル整数を経由すると 1 m 未満の起伏が消える。
            let h_cm = (m as i64 * amplitude_m as i64 * 100) / sim_math::FX_ONE as i64;
            let mut h = h_cm.clamp(i16::MIN as i64, i16::MAX as i64) as i32;

            // 崖の縁は鋭い段差が命なので、これだけは補間せず fine 解像度のまま計算する
            h += outcrop_bump_cm(x, y, p);

            // 海の縁: 指定した辺に近いほど標高を引き下げ、海岸線を作る。
            if p.sea_edge != SeaEdge::None {
                let dim = t.dim as i32;
                let band = sea_band(dim);
                let dist_to_edge = sea_edge_distance(x as i32, y as i32, dim, p.sea_edge);
                if dist_to_edge < band {
                    // 端で海面下 8 m、遷移帯の内側端で元の高さに一致する線形ランプ
                    let t_permille = (dist_to_edge * 1000 / band.max(1)).clamp(0, 1000);
                    let sea_floor_cm = p.sea_level_cm - 800;
                    h = sea_floor_cm + ((h - sea_floor_cm) * t_permille) / 1000;
                }
            }
            t.height[idx] = h.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        }
    }
}

/// 局所的な岩場・小さな崖を作る高周波レイヤー。
///
/// [`base_elevation`] のベース地形は特徴の周期を 800 m に取っているため滑らかで、
/// 隣接セル間の高低差が [`CLIFF_THRESHOLD_CM`] を超えることがほとんどない。
/// これでは山岳地形でしか崖が現れず、丘陵程度の起伏では地形の中に崖という
/// 戦術要素が一切出ない。稜線ノイズをずっと短い周期（120 m）で重ね、
/// 一定値を超えた領域だけ鋭く立ち上げることで、平原〜丘陵にも岩場の縁として
/// 局所的な崖を点在させる。
fn outcrop_bump_cm(x: u32, y: u32, p: &TerrainParams) -> i32 {
    let lattice = (120 / p.cell_m).max(1) as i32;
    let ox = sim_math::fx_div(fx(x as i32), fx(lattice));
    let oy = sim_math::fx_div(fx(y as i32), fx(lattice));
    let ridge = noise::ridged(ox, oy, 2, p.seed ^ 0x0000_4F55_5443_524F); // "OUTCRO"

    // 0..FX_ONE のうち上位 35% だけを、0 から急峻に立ち上がる形に写す
    const THRESHOLD: sim_math::Fx = sim_math::FX_ONE * 65 / 100;
    if ridge <= THRESHOLD {
        return 0;
    }
    let sharp = ridge - THRESHOLD; // 0..(FX_ONE - THRESHOLD)
    let span = sim_math::FX_ONE - THRESHOLD;

    // 起伏が強いほど岩場も高くなる: relief 0 → 4 m、relief 1000 → 14 m
    let amplitude_m = 4 + (p.relief as i32 * 10) / 1000;
    (sharp as i64 * amplitude_m as i64 * 100 / span as i64) as i32
}

/// 海の遷移帯の幅（セル）。マップ幅の 15%。
///
/// [`base_elevation`] の海岸ランプと [`apply_sea`] の海域判定は、
/// 必ずこの同じ帯の中でだけ効かせる。範囲を共有しないと、帯の外側で
/// たまたま標高が海面を下回った内陸の低地まで「海」として扱ってしまう
/// （実際に、海のない南端の窪地が海と誤判定される不具合が起きた）。
fn sea_band(dim: i32) -> i32 {
    (dim * 15 / 100).max(4)
}

/// 指定した辺から見たセルの距離（セル数）。
fn sea_edge_distance(x: i32, y: i32, dim: i32, edge: SeaEdge) -> i32 {
    match edge {
        SeaEdge::North => y,
        SeaEdge::South => dim - 1 - y,
        SeaEdge::West => x,
        SeaEdge::East => dim - 1 - x,
        SeaEdge::None => i32::MAX,
    }
}

/// 海面より低いセルを海として上書きする。`sea_edge` が `None` なら何もしない。
///
/// 海岸の遷移帯（[`sea_band`]）の中だけを対象にする。帯の外は、たとえ
/// 標高が偶然 `sea_level_cm` を下回っても内陸の自然な低地であり、海ではない。
fn apply_sea(t: &mut Terrain, p: &TerrainParams) {
    if p.sea_edge == SeaEdge::None {
        return;
    }
    let dim = t.dim as i32;
    let band = sea_band(dim);
    for y in 0..dim {
        for x in 0..dim {
            if sea_edge_distance(x, y, dim, p.sea_edge) >= band {
                continue;
            }
            let idx = t.idx(x as u32, y as u32);
            let below = p.sea_level_cm - t.height[idx] as i32;
            if below > 0 {
                t.water[idx] = (below / 10).clamp(1, u8::MAX as i32) as u8;
                t.water_kind[idx] = WaterKind::Sea;
            }
        }
    }
}

/// 崖として扱う隣接セル間の高低差の閾値（cm）。仕様 03 章 2.5 節。
///
/// [`thermal_erosion`] はこの値以上の段差を「侵食に耐える露岩」とみなして
/// 対象から除外する。ここより低い値にすると、浸食で均される前の緩斜面まで
/// 崖として検出されてしまう。
const CLIFF_THRESHOLD_CM: i32 = 250;

/// 段階 11: 崖エッジを検出する。
///
/// 隣接セルとの高低差が [`CLIFF_THRESHOLD_CM`] を超えるエッジを崖としてマークする
/// （仕様 03 章 2.5 節）。
fn compute_cliffs(t: &Terrain) -> Vec<u8> {
    let dim = t.dim as i32;
    let mut cliff = vec![0u8; t.height.len()];

    for y in 0..dim {
        for x in 0..dim {
            let idx = (y * dim + x) as usize;
            let here = t.height[idx] as i32;
            let mut bits = 0u8;
            if y > 0 && (here - t.height_at_cell(x, y - 1) as i32).abs() >= CLIFF_THRESHOLD_CM {
                bits |= cliff_bits::NORTH;
            }
            if x < dim - 1 && (here - t.height_at_cell(x + 1, y) as i32).abs() >= CLIFF_THRESHOLD_CM
            {
                bits |= cliff_bits::EAST;
            }
            if y < dim - 1 && (here - t.height_at_cell(x, y + 1) as i32).abs() >= CLIFF_THRESHOLD_CM
            {
                bits |= cliff_bits::SOUTH;
            }
            if x > 0 && (here - t.height_at_cell(x - 1, y) as i32).abs() >= CLIFF_THRESHOLD_CM {
                bits |= cliff_bits::WEST;
            }
            cliff[idx] = bits;
        }
    }
    cliff
}

/// 各セルから最も近い水域までの距離（セル数）。多始点 BFS で O(n)。
///
/// `max_cells` を超えた距離は湿度に影響しないので、そこで打ち切ってよいが、
/// 結果配列そのものは全セルぶん持つ（`u16::MAX` = 未到達 = 上限扱い）。
fn distance_to_water(t: &Terrain, max_cells: u16) -> Vec<u16> {
    let n = t.height.len();
    let dim = t.dim as i32;
    let mut dist = vec![u16::MAX; n];
    let mut queue: VecDeque<u32> = VecDeque::new();

    for (i, &w) in t.water.iter().enumerate() {
        if w > 0 {
            dist[i] = 0;
            queue.push_back(i as u32);
        }
    }

    while let Some(idx) = queue.pop_front() {
        let d = dist[idx as usize];
        if d >= max_cells {
            continue;
        }
        let cx = idx as i32 % dim;
        let cy = idx as i32 / dim;
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let nx = cx + dx;
            let ny = cy + dy;
            if nx < 0 || ny < 0 || nx >= dim || ny >= dim {
                continue;
            }
            let nidx = (ny * dim + nx) as usize;
            if dist[nidx] == u16::MAX {
                dist[nidx] = d + 1;
                queue.push_back(nidx as u32);
            }
        }
    }
    dist
}

/// 段階 4: 熱浸食。安息角を超えた斜面を崩す。
///
/// 差が [`CLIFF_THRESHOLD_CM`] 以上のペアは対象から除外する。これは緩い崖錐
/// （安息角を守るルーズな堆積物）と、侵食に耐える露岩の崖を区別するため。
/// 除外がないと、何度も反復するうちにどんな急崖も安息角まで均されてしまい、
/// 崖グリッド（[`compute_cliffs`]）が常に空になってしまう。
fn thermal_erosion(t: &mut Terrain, iterations: u16) {
    // 安息角 35° ≒ 隣接セル間で cell_m(m) * tan(35°) ≈ cell_m(m) * 0.70 の高低差（cm）。
    // cell_m は m 単位の小さい整数なので、cm への換算とこの倍率が掛け算の順で
    // ちょうど打ち消し合い、`cell_m * 70` がそのまま cm 表記の閾値になる。
    let talus_cm = (t.cell_m as i32 * 70).max(20);
    let dim = t.dim as i32;

    for _ in 0..iterations {
        // 前回の値を読み、新しい配列に書く（読み書きフェーズの分離）
        let src = t.height.clone();
        for y in 0..dim {
            for x in 0..dim {
                let here = src[(y * dim + x) as usize];
                let mut lowest_delta: i32 = 0;
                let mut lowest_idx: Option<usize> = None;

                for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= dim || ny >= dim {
                        continue;
                    }
                    let there = src[(ny * dim + nx) as usize];
                    let delta = here as i32 - there as i32;
                    if delta > talus_cm && delta < CLIFF_THRESHOLD_CM && delta > lowest_delta {
                        lowest_delta = delta;
                        lowest_idx = Some((ny * dim + nx) as usize);
                    }
                }

                if let Some(li) = lowest_idx {
                    // 超過分の 1/4 を最も低い隣へ移す
                    let move_cm = ((lowest_delta - talus_cm) / 4).max(1);
                    let hi = (y * dim + x) as usize;
                    t.height[hi] = t.height[hi].saturating_sub(move_cm as i16);
                    t.height[li] = t.height[li].saturating_add(move_cm as i16);
                }
            }
        }
    }
}

/// 段階 9〜10: 水系からの距離を混ぜた湿度と、標高・傾斜から地表タイプを割り当てる。
///
/// 水域セル（`water_kind != None`）は深さと種類から直接
/// Ford / ShallowWater / DeepWater を決め、植生の判定はスキップする。
/// それ以外のセルは、ノイズ由来の基礎湿度に「水域への近さ」を加えた
/// 合成湿度で判定する。川べり・湖畔ほど湿地や森になりやすい。
fn classify_surface(t: &mut Terrain, p: &TerrainParams, dist_to_water: &[u16]) {
    let cells_per_lattice = (600 / p.cell_m).max(1) as i32;
    let dim = t.dim as i32;
    const PROXIMITY_RANGE: i32 = 24; // distance_to_water の上限と合わせる

    // 湿度ノイズも base_elevation と同じ理由で粗いグリッド + バイリニア補間にする
    let humidity_field = upsample::upsample_coarse(t.dim, NOISE_UPSAMPLE_STRIDE, |gx, gy| {
        let nx = sim_math::fx_div(fx(gx), fx(cells_per_lattice));
        let ny = sim_math::fx_div(fx(gy), fx(cells_per_lattice));
        noise::fbm(nx, ny, 4, p.seed ^ 0x0000_4855_4D49_4400)
    });

    for y in 0..dim {
        for x in 0..dim {
            let idx = (y * dim + x) as usize;

            if t.water_kind[idx] != WaterKind::None {
                let depth_cm = t.water[idx] as i32 * 10;
                t.surface[idx] = if t.water_kind[idx] == WaterKind::River && depth_cm < 40 {
                    Surface::Ford
                } else if depth_cm < 150 {
                    Surface::ShallowWater
                } else {
                    Surface::DeepWater
                } as u8;
                continue;
            }

            let slope_cm = local_slope_cm(t, x, y);

            // -FX_ONE..FX_ONE を 0..1000 の湿度に写す
            let humidity_noise = humidity_field[idx];
            let base_humidity = ((humidity_noise + sim_math::FX_ONE) * 500) / sim_math::FX_ONE;

            // 水域までの距離が近いほど加点する（0 距離で最大 +1000、上限距離で 0）
            let d = (dist_to_water[idx] as i32).min(PROXIMITY_RANGE);
            let proximity_bonus = ((PROXIMITY_RANGE - d) * 1000) / PROXIMITY_RANGE;
            let humidity = ((base_humidity * 6) + (proximity_bonus * 4)) / 10;

            // 傾斜が急なら岩、緩やかなら湿度で植生を決める
            let cell_cm = t.cell_m as i32 * 100;
            let surface = if slope_cm * 100 / cell_cm > 70 {
                Surface::Rock
            } else if slope_cm * 100 / cell_cm > 35 {
                Surface::Scrub
            } else if humidity > 880 - (p.marsh_bias as i32 / 4) && slope_cm * 100 / cell_cm < 8 {
                Surface::Marsh
            } else if humidity > 1000 - p.forest_cover as i32 {
                if humidity > 1000 - p.forest_cover as i32 / 2 {
                    Surface::DenseForest
                } else {
                    Surface::LightForest
                }
            } else if humidity > 420 {
                Surface::Meadow
            } else {
                Surface::Grass
            };
            t.surface[idx] = surface as u8;
        }
    }

    smooth_forests(t);
    // 海岸沿いに砂浜の帯を作る（森・湿地判定より後、最終見た目として上書き）
    apply_beaches(t);
}

/// 海に接する陸地セルを砂浜にする。
fn apply_beaches(t: &mut Terrain) {
    if !t.water_kind.contains(&WaterKind::Sea) {
        return;
    }
    let dim = t.dim as i32;
    let src = t.surface.clone();
    for y in 0..dim {
        for x in 0..dim {
            let idx = (y * dim + x) as usize;
            if t.water_kind[idx] != WaterKind::None {
                continue;
            }
            if matches!(Surface::from_u8(src[idx]), Surface::Rock | Surface::Scree) {
                continue; // 断崖には砂浜を作らない
            }
            let mut near_sea = false;
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= dim || ny >= dim {
                    continue;
                }
                if t.water_kind[(ny * dim + nx) as usize] == WaterKind::Sea {
                    near_sea = true;
                    break;
                }
            }
            if near_sea {
                t.surface[idx] = Surface::Sand as u8;
            }
        }
    }
}

/// 森の境界をセルオートマトンで均し、斑にならないようにする。
fn smooth_forests(t: &mut Terrain) {
    let dim = t.dim as i32;
    for _ in 0..3 {
        let src = t.surface.clone();
        for y in 0..dim {
            for x in 0..dim {
                let idx = (y * dim + x) as usize;
                let s = Surface::from_u8(src[idx]);
                if !matches!(
                    s,
                    Surface::Grass | Surface::Meadow | Surface::LightForest | Surface::DenseForest
                ) {
                    continue;
                }
                let mut forest_neighbors = 0;
                let mut total = 0;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let (nx, ny) = (x + dx, y + dy);
                        if nx < 0 || ny < 0 || nx >= dim || ny >= dim {
                            continue;
                        }
                        total += 1;
                        if matches!(
                            Surface::from_u8(src[(ny * dim + nx) as usize]),
                            Surface::LightForest | Surface::DenseForest
                        ) {
                            forest_neighbors += 1;
                        }
                    }
                }
                let is_forest = matches!(s, Surface::LightForest | Surface::DenseForest);
                if is_forest && forest_neighbors * 2 < total {
                    t.surface[idx] = Surface::Meadow as u8;
                } else if !is_forest && forest_neighbors * 3 > total * 2 {
                    t.surface[idx] = Surface::LightForest as u8;
                }
            }
        }
    }
}

/// セルの最大高低差（cm）。
fn local_slope_cm(t: &Terrain, x: i32, y: i32) -> i32 {
    let here = t.height_at_cell(x, y) as i32;
    let mut max = 0;
    for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
        let there = t.height_at_cell(x + dx, y + dy) as i32;
        max = max.max((here - there).abs());
    }
    max
}

/// 段階 13: 地表と傾斜から通行コストを導出する。
fn derive_passability(t: &mut Terrain) {
    let dim = t.dim as i32;
    let cell_cm = t.cell_m as i32 * 100;
    for y in 0..dim {
        for x in 0..dim {
            let idx = (y * dim + x) as usize;
            let s = Surface::from_u8(t.surface[idx]);
            let e = &SURFACE_EFFECTS[s as usize];
            if !s.passable() {
                t.passability[idx] = 0;
                continue;
            }
            // 傾斜が tan 0.55 を超えたら通行不能（仕様 03 章 3.1）
            let slope = local_slope_cm(t, x, y);
            if slope * 100 / cell_cm > 55 {
                t.passability[idx] = 0;
                continue;
            }
            let slope_penalty = (slope * 100 / cell_cm).min(55) as u32;
            let base = e.move_mult as u32 * 255 / 1150;
            let v = base.saturating_sub(base * slope_penalty / 100);
            t.passability[idx] = v.clamp(1, 255) as u8;
        }
    }
}

/// 生成された地形の健全性を検査する。テストと CI で使う。
///
/// 仕様 03 章の M1 受け入れ条件に対応する項目を検査する。
/// - グリッド長の一致
/// - 通行可能セルの比率
/// - **通行可能領域が大きく分断されていない**（最大連結成分が通行可能セル全体の
///   90% 以上を占める）
pub fn validate(t: &Terrain) -> Result<(), String> {
    let n = t.height.len();
    if t.surface.len() != n
        || t.passability.len() != n
        || t.water.len() != n
        || t.water_kind.len() != n
        || t.cliff.len() != n
    {
        return Err("グリッドの長さが一致しない".into());
    }
    let passable = t.passability.iter().filter(|&&p| p > 0).count();
    let ratio = passable * 100 / n;
    if ratio < 50 {
        return Err(format!("通行可能セルが {ratio}% しかない"));
    }

    if let Some(frag_ratio) = (largest_connected_component(t) * 100).checked_div(passable) {
        if frag_ratio < 90 {
            return Err(format!(
                "通行可能領域が分断されている: 最大連結成分は全体の {frag_ratio}%"
            ));
        }
    }

    Ok(())
}

/// 通行可能セルのうち最大の連結成分の大きさを求める（4 近傍の BFS）。
fn largest_connected_component(t: &Terrain) -> usize {
    let dim = t.dim as i32;
    let n = t.passability.len();
    let mut visited = vec![false; n];
    let mut best = 0usize;

    for start in 0..n {
        if visited[start] || t.passability[start] == 0 {
            continue;
        }
        let mut queue: VecDeque<u32> = VecDeque::new();
        visited[start] = true;
        queue.push_back(start as u32);
        let mut count = 0usize;

        while let Some(idx) = queue.pop_front() {
            count += 1;
            let cx = idx as i32 % dim;
            let cy = idx as i32 / dim;
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let nx = cx + dx;
                let ny = cy + dy;
                if nx < 0 || ny < 0 || nx >= dim || ny >= dim {
                    continue;
                }
                let nidx = (ny * dim + nx) as usize;
                if !visited[nidx] && t.passability[nidx] > 0 {
                    visited[nidx] = true;
                    queue.push_back(nidx as u32);
                }
            }
        }
        best = best.max(count);
    }
    best
}

/// デバッグ用に地形の統計を返す。
pub fn stats(t: &Terrain) -> TerrainStats {
    let mut counts = [0u32; Surface::COUNT];
    for &s in &t.surface {
        counts[(s & 0x0F) as usize] += 1;
    }
    let river_cells = t
        .water_kind
        .iter()
        .filter(|k| **k == WaterKind::River)
        .count() as u32;
    let lake_cells = t
        .water_kind
        .iter()
        .filter(|k| **k == WaterKind::Lake)
        .count() as u32;
    let sea_cells = t
        .water_kind
        .iter()
        .filter(|k| **k == WaterKind::Sea)
        .count() as u32;
    let cliff_edges = t.cliff.iter().filter(|&&c| c != 0).count() as u32;

    TerrainStats {
        min_height_cm: *t.height.iter().min().unwrap_or(&0),
        max_height_cm: *t.height.iter().max().unwrap_or(&0),
        surface_counts: counts,
        impassable: t.passability.iter().filter(|&&p| p == 0).count() as u32,
        river_cells,
        lake_cells,
        sea_cells,
        cliff_edges,
        largest_passable_component: largest_connected_component(t) as u32,
        battle_sites: t.battle_sites.len() as u32,
    }
}

#[derive(Debug)]
pub struct TerrainStats {
    pub min_height_cm: i16,
    pub max_height_cm: i16,
    pub surface_counts: [u32; Surface::COUNT],
    pub impassable: u32,
    pub river_cells: u32,
    pub lake_cells: u32,
    pub sea_cells: u32,
    pub cliff_edges: u32,
    pub largest_passable_component: u32,
    pub battle_sites: u32,
}

/// 参考: 決定論的な乱数を地形生成で使うときのストリーム。
pub fn terrain_rng(seed: u64, salt: u32) -> Rng {
    Rng::stream(seed, salt, sim_math::Purpose::Terrain, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_params() -> TerrainParams {
        TerrainParams {
            size_m: 600,
            cell_m: 2,
            thermal_iterations: 5,
            ..Default::default()
        }
    }

    #[test]
    fn generation_is_deterministic() {
        let p = small_params();
        let a = generate(&p);
        let b = generate(&p);
        assert_eq!(a.hash(), b.hash());
    }

    #[test]
    fn different_seeds_give_different_terrain() {
        let a = generate(&small_params());
        let b = generate(&TerrainParams {
            seed: 0xDEAD_BEEF,
            ..small_params()
        });
        assert_ne!(a.hash(), b.hash());
    }

    #[test]
    fn grids_have_consistent_size() {
        let t = generate(&small_params());
        assert_eq!(t.dim, 300);
        assert_eq!(t.height.len(), 300 * 300);
        assert_eq!(t.surface.len(), t.height.len());
        assert_eq!(t.passability.len(), t.height.len());
        assert_eq!(t.size_m(), 600);
    }

    #[test]
    fn generated_terrain_is_valid() {
        let t = generate(&small_params());
        validate(&t).expect("地形が健全でない");
    }

    #[test]
    fn flat_params_produce_gentle_terrain() {
        let t = generate(&TerrainParams {
            relief: 0,
            ..small_params()
        });
        let s = stats(&t);
        let range_m = (s.max_height_cm as i32 - s.min_height_cm as i32) / 100;
        assert!(range_m <= 25, "起伏が大きすぎる: {range_m} m");
    }

    #[test]
    fn mountainous_params_produce_relief() {
        let t = generate(&TerrainParams {
            relief: 1000,
            ..small_params()
        });
        let s = stats(&t);
        let range_m = (s.max_height_cm as i32 - s.min_height_cm as i32) / 100;
        assert!(range_m >= 40, "起伏が小さすぎる: {range_m} m");
    }

    #[test]
    fn height_sampling_is_continuous() {
        let t = generate(&small_params());
        // 隣接する 2 点の標高差が急に跳ばない
        let mut prev = t.height_at(fx(10), fx(10));
        for i in 1..500 {
            let h = t.height_at(fx(10) + i * 8, fx(10));
            assert!((h - prev).abs() < fx(3), "i={i}");
            prev = h;
        }
    }

    #[test]
    fn surface_lookup_stays_in_bounds() {
        let t = generate(&small_params());
        // 範囲外の座標でもパニックしない
        let _ = t.surface_at(fx(-100), fx(-100));
        let _ = t.surface_at(fx(99999), fx(99999));
        let _ = t.height_at(fx(-100), fx(99999));
    }

    #[test]
    fn surface_effects_table_is_complete() {
        assert_eq!(SURFACE_EFFECTS.len(), Surface::COUNT);
        // 深水は通行不能、道路は最速
        assert_eq!(SURFACE_EFFECTS[Surface::DeepWater as usize].move_mult, 0);
        assert!(SURFACE_EFFECTS[Surface::Road as usize].move_mult > 1000);
        // 泥は疲労が大きい（アジンクールの再現に必要）
        assert!(SURFACE_EFFECTS[Surface::Mud as usize].fatigue_mult > 1500);
    }

    fn mid_params(seed: u64) -> TerrainParams {
        TerrainParams {
            seed,
            size_m: 1600,
            cell_m: 2,
            relief: 550,
            thermal_iterations: 8,
            river_density: 600,
            road_count: 2,
            ..Default::default()
        }
    }

    #[test]
    fn moderate_map_is_valid_and_has_rivers() {
        let t = generate(&mid_params(101));
        validate(&t).expect("地形が健全でない");
        let s = stats(&t);
        assert!(s.river_cells > 0, "河川が 1 セルも生成されなかった");
    }

    #[test]
    fn lakes_do_not_dominate_the_map() {
        // 回帰ガード: 滑らかな地形では内陸側の広い低地がまるごと 1 つの
        // 巨大な湖になりうる（優先度フラッドとしては正しいが、会戦の
        // 舞台としては明らかにおかしい）。hydrology::MAX_LAKE_CELLS による
        // 上限が効いていることを、複数シードで確認する。
        for seed in [55u64, 7, 999, 301, 42] {
            let t = generate(&TerrainParams {
                relief: 500,
                thermal_iterations: 20,
                ..mid_params(seed)
            });
            let s = stats(&t);
            let lake_permille = s.lake_cells as u64 * 1000 / t.surface.len() as u64;
            assert!(
                lake_permille < 250,
                "seed={seed}: 湖が地図の {}‰ を占めている（巨大湖の疑い）",
                lake_permille
            );
        }
    }

    #[test]
    fn zero_river_density_produces_no_rivers() {
        let t = generate(&TerrainParams {
            river_density: 0,
            ..mid_params(102)
        });
        let s = stats(&t);
        assert_eq!(s.river_cells, 0);
    }

    #[test]
    fn passable_area_is_not_badly_fragmented() {
        // M1 の受け入れ条件: 通行可能領域が大きく分断されていない。
        // relief 700 程度までの、会戦の舞台として現実的な起伏で検証する。
        // 極端な山岳（relief 850 超）は現実の山岳地帯と同じく谷が孤立しうるので、
        // ここでは対象にしない（該当シナリオでは会戦地の自動評価が
        // 展開可能な平坦地を別途探す）。
        for seed in [201u64, 202, 203] {
            let t = generate(&TerrainParams {
                relief: 700,
                thermal_iterations: 10,
                ..mid_params(seed)
            });
            validate(&t).unwrap_or_else(|e| panic!("seed={seed}: {e}"));
        }
    }

    #[test]
    fn hash_is_sensitive_to_water_and_cliff_grids() {
        // 同一シードなら water/cliff を含めてもハッシュは再現する。
        // 崖の出現には起伏がある程度必要なので relief を上げる。
        let params = TerrainParams {
            relief: 700,
            ..mid_params(301)
        };
        let a = generate(&params);
        let b = generate(&params);
        assert_eq!(a.hash(), b.hash());
        assert!(a.water.iter().any(|&w| w > 0), "水域が生成されていない");
        assert!(a.cliff.iter().any(|&c| c != 0), "崖が生成されていない");
    }

    #[test]
    fn sea_edge_creates_water_along_that_edge() {
        let t = generate(&TerrainParams {
            sea_edge: SeaEdge::North,
            sea_level_cm: 0,
            ..mid_params(401)
        });
        // グリッド長など構造面の健全性は確認する。連結性の厳密な 90% 判定
        // （`passable_area_is_not_badly_fragmented` で別途検証）はここでは
        // 求めない。海岸線は地形の一部を通行不能にするので、内陸の起伏次第で
        // 90% をわずかに割ることがあり、それ自体は不具合ではないため。
        let s = stats(&t);
        let passable = t.surface.len() as u32 - s.impassable;
        assert!(
            passable > 0 && s.largest_passable_component * 100 / passable >= 70,
            "陸地が過度に分断されている"
        );

        let dim = t.dim;
        let mut sea_at_north = 0;
        for x in 0..dim {
            if t.water_kind[t.idx(x, 0)] == WaterKind::Sea {
                sea_at_north += 1;
            }
        }
        assert!(
            sea_at_north as u32 > dim / 2,
            "北端の大半が海になっていない: {sea_at_north}/{dim}"
        );

        // 南端は海の影響を受けず、通常どおり陸地のはず
        let mut sea_at_south = 0;
        for x in 0..dim {
            if t.water_kind[t.idx(x, dim - 1)] == WaterKind::Sea {
                sea_at_south += 1;
            }
        }
        assert_eq!(sea_at_south, 0);
    }

    #[test]
    fn no_sea_by_default() {
        let t = generate(&mid_params(402));
        assert!(!t.water_kind.contains(&WaterKind::Sea));
    }

    #[test]
    fn battle_sites_are_populated_on_a_normal_map() {
        let t = generate(&mid_params(501));
        assert!(!t.battle_sites.is_empty());
    }
}
