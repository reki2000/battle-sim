//! 地形生成。
//!
//! 仕様は `docs/spec/03-terrain.md`。
//!
//! **M0 の実装範囲**: グリッド構造、地表タイプの定義、地表効果のテーブル、
//! ノイズによるベース標高と傾斜からの地表分類、熱浸食。
//! 水系（D8 フロー、河川）・道路・会戦地の評価はまだ実装していない（M1 で追加する）。

pub mod noise;

use sim_math::{fx, fx_from_mm, Fx, Rng};

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
    /// 熱浸食の反復回数
    pub thermal_iterations: u16,
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
            thermal_iterations: 30,
        }
    }
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
        h
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
    };

    base_elevation(&mut t, params);
    thermal_erosion(&mut t, params.thermal_iterations);
    classify_surface(&mut t, params);
    derive_passability(&mut t);
    t
}

/// 段階 1〜2: ベース標高と山脈。
fn base_elevation(t: &mut Terrain, p: &TerrainParams) {
    // ノイズの 1 格子 = 800 m 相当になるようスケールする。
    // 会戦の舞台となる地形は 2 m スケールでは滑らかなので、
    // 特徴の周期を大きく取り、オクターブを抑えて高周波を減らす。
    let cells_per_lattice = (800 / p.cell_m).max(1) as i32;
    // 起伏の強さ（m）。relief 0 → 5 m（平原）、relief 1000 → 125 m（丘陵〜山地）
    let amplitude_m = 5 + (p.relief as i32 * 120) / 1000;

    for y in 0..t.dim {
        for x in 0..t.dim {
            let nx = sim_math::fx_div(fx(x as i32), fx(cells_per_lattice));
            let ny = sim_math::fx_div(fx(y as i32), fx(cells_per_lattice));

            let base = noise::warped_fbm(nx, ny, sim_math::FX_HALF, 5, p.seed);
            // 起伏が強いほど稜線ノイズを混ぜる
            let ridge = noise::ridged(nx, ny, 4, p.seed ^ 0x0000_5249_4447_4500);
            let mixed = base + sim_math::fx_mul(ridge, fx(p.relief as i32) / 1000);

            // cm で直接求める。メートル整数を経由すると 1 m 未満の起伏が消える。
            let h_cm = (mixed as i64 * amplitude_m as i64 * 100) / sim_math::FX_ONE as i64;
            let idx = t.idx(x, y);
            t.height[idx] = h_cm.clamp(i16::MIN as i64, i16::MAX as i64) as i16;
        }
    }
}

/// 段階 4: 熱浸食。安息角を超えた斜面を崩す。
fn thermal_erosion(t: &mut Terrain, iterations: u16) {
    // 安息角 35° ≒ 隣接セル間で cell_m * tan(35°) = cell_m * 0.70 の高低差
    let talus_cm = ((t.cell_m as i32 * 70) / 100 * 100).max(20) as i16;
    let dim = t.dim as i32;

    for _ in 0..iterations {
        // 前回の値を読み、新しい配列に書く（読み書きフェーズの分離）
        let src = t.height.clone();
        for y in 0..dim {
            for x in 0..dim {
                let here = src[(y * dim + x) as usize];
                let mut total_excess: i32 = 0;
                let mut lowest_delta: i32 = 0;
                let mut lowest_idx: Option<usize> = None;

                for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= dim || ny >= dim {
                        continue;
                    }
                    let there = src[(ny * dim + nx) as usize];
                    let delta = here as i32 - there as i32;
                    if delta > talus_cm as i32 {
                        total_excess += delta;
                        if delta > lowest_delta {
                            lowest_delta = delta;
                            lowest_idx = Some((ny * dim + nx) as usize);
                        }
                    }
                }

                if let Some(li) = lowest_idx {
                    // 超過分の 1/4 を最も低い隣へ移す
                    let move_cm = ((lowest_delta - talus_cm as i32) / 4).max(1);
                    let hi = (y * dim + x) as usize;
                    t.height[hi] = t.height[hi].saturating_sub(move_cm as i16);
                    t.height[li] = t.height[li].saturating_add(move_cm as i16);
                }
                let _ = total_excess;
            }
        }
    }
}

/// 段階 10: 標高・傾斜・擬似的な湿度から地表タイプを割り当てる。
///
/// M1 で水系を実装したら、湿度は河川と湖からの距離で置き換える。
fn classify_surface(t: &mut Terrain, p: &TerrainParams) {
    let cells_per_lattice = (600 / p.cell_m).max(1) as i32;
    let dim = t.dim as i32;

    for y in 0..dim {
        for x in 0..dim {
            let idx = (y * dim + x) as usize;
            let slope_cm = local_slope_cm(t, x, y);

            let nx = sim_math::fx_div(fx(x), fx(cells_per_lattice));
            let ny = sim_math::fx_div(fx(y), fx(cells_per_lattice));
            // -FX_ONE..FX_ONE を 0..1000 の湿度に写す
            let humidity_noise = noise::fbm(nx, ny, 4, p.seed ^ 0x0000_4855_4D49_4400);
            let humidity =
                (((humidity_noise + sim_math::FX_ONE) as i32 * 500) / sim_math::FX_ONE) as i32;

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
pub fn validate(t: &Terrain) -> Result<(), String> {
    let n = t.height.len();
    if t.surface.len() != n || t.passability.len() != n {
        return Err("グリッドの長さが一致しない".into());
    }
    let passable = t.passability.iter().filter(|&&p| p > 0).count();
    let ratio = passable * 100 / n;
    if ratio < 50 {
        return Err(format!("通行可能セルが {ratio}% しかない"));
    }
    Ok(())
}

/// デバッグ用に地形の統計を返す。
pub fn stats(t: &Terrain) -> TerrainStats {
    let mut counts = [0u32; Surface::COUNT];
    for &s in &t.surface {
        counts[(s & 0x0F) as usize] += 1;
    }
    TerrainStats {
        min_height_cm: *t.height.iter().min().unwrap_or(&0),
        max_height_cm: *t.height.iter().max().unwrap_or(&0),
        surface_counts: counts,
        impassable: t.passability.iter().filter(|&&p| p == 0).count() as u32,
    }
}

#[derive(Debug)]
pub struct TerrainStats {
    pub min_height_cm: i16,
    pub max_height_cm: i16,
    pub surface_counts: [u32; Surface::COUNT],
    pub impassable: u32,
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
}
