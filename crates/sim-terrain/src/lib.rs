//! 地形。仕様は `docs/spec/03-terrain.md`。
//!
//! **このクレートは地形を生成しない。** 生成器は `web/src/terrain` にあり、
//! 多スケールのノイズ・水系・被覆の判定を f64 で行う。ここが受け取るのは
//! その結果の整数グリッドだけで、シミュレーションはその整数だけを見て動く。
//! こうしておくと、生成側の丸めが環境で 1 cm ずれても、シミュレーションの
//! 決定論には一切伝播しない（仕様 02 章 2.5 節）。
//!
//! グリッドは**直交した軸**に分かれている。一つの列挙に地質・植生・水・
//! 人工物を詰め込むと、「砂地の疎林」のような組み合わせが表現できず、
//! 工兵の伐採（植生だけ消す）が地質まで書き換えてしまう。軸を分けたことで、
//! それぞれが独立に読み書きできる。
//!
//! | 軸 | グリッド | 内容 |
//! |---|---|---|
//! | 標高 | `height` | cm（`i16`） |
//! | 地質 | `ground` | [`Ground`] |
//! | 植生 | `vegetation` | [`Vegetation`] |
//! | 水 | `water` / `water_kind` | 水深 cm（`u16`）と種別 |
//! | 人工物 | `overlay` | [`Overlay`] |
//! | 湿度 | `moisture` | 0..255。泥濘化と植生が読む |
//! | 導出 | `cliff` / `slope` / `passability` | 読み込み時に計算する |

pub mod effects;
pub mod fixture;

pub use effects::{
    base_passability, compose_effect, passability_with_slope, water_effect, work_mult_permille,
    TerrainEffect, GROUND_EFFECTS, NEUTRAL, OVERLAY_EFFECTS, VEGETATION_EFFECTS,
};

use sim_math::{fx, fx_from_mm, Fx};
use std::collections::VecDeque;

/// セルの地質・地表素材。植生とは独立。
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Ground {
    /// 裸地・締まった土
    #[default]
    Soil = 0,
    /// 草地の土壌
    Grass = 1,
    Sand = 2,
    Rock = 3,
    Mud = 4,
    /// 河床・湖底。水深は `water` グリッドが持つ
    RiverBed = 5,
    SeaBed = 6,
    /// 崖錐。熱浸食が崖の下に積む不安定な礫
    Scree = 7,
    /// 湿地の土壌
    Marsh = 8,
}

impl Ground {
    pub const COUNT: usize = 9;

    #[inline]
    pub fn from_u8(v: u8) -> Ground {
        match v {
            1 => Ground::Grass,
            2 => Ground::Sand,
            3 => Ground::Rock,
            4 => Ground::Mud,
            5 => Ground::RiverBed,
            6 => Ground::SeaBed,
            7 => Ground::Scree,
            8 => Ground::Marsh,
            _ => Ground::Soil,
        }
    }
}

/// セルの植生。地質とは独立。
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Vegetation {
    #[default]
    None = 0,
    /// 短い草。遮蔽にならない
    ShortGrass = 1,
    /// 疎林
    Sparse = 2,
    /// 林
    Woodland = 3,
    /// 密林
    Dense = 4,
    /// 灌木
    Scrub = 5,
    /// 丈の高い草地
    Meadow = 6,
    /// 耕地
    Farmland = 7,
}

impl Vegetation {
    pub const COUNT: usize = 8;

    #[inline]
    pub fn from_u8(v: u8) -> Vegetation {
        match v {
            1 => Vegetation::ShortGrass,
            2 => Vegetation::Sparse,
            3 => Vegetation::Woodland,
            4 => Vegetation::Dense,
            5 => Vegetation::Scrub,
            6 => Vegetation::Meadow,
            7 => Vegetation::Farmland,
            _ => Vegetation::None,
        }
    }

    /// 伐採の対象になる林か（工兵の射界確保、仕様 07 章 3 節）。
    #[inline]
    pub fn is_forest(self) -> bool {
        matches!(self, Vegetation::Woodland | Vegetation::Dense)
    }
}

/// 人工物のオーバーレイ。地質・植生の上に重なり、効果を上書きする。
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Overlay {
    #[default]
    None = 0,
    Road = 1,
    Bridge = 2,
}

impl Overlay {
    pub const COUNT: usize = 3;

    #[inline]
    pub fn from_u8(v: u8) -> Overlay {
        match v {
            1 => Overlay::Road,
            2 => Overlay::Bridge,
            _ => Overlay::None,
        }
    }
}

/// 水域の種類。水深と対になるメタデータ。
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum WaterKind {
    #[default]
    None = 0,
    Lake = 1,
    River = 2,
    Sea = 3,
}

impl WaterKind {
    #[inline]
    pub fn from_u8(v: u8) -> WaterKind {
        match v {
            1 => WaterKind::Lake,
            2 => WaterKind::River,
            3 => WaterKind::Sea,
            _ => WaterKind::None,
        }
    }
}

/// 崖のビット。隣接セルとの高低差が閾値を超えるとそのビットが立つ。
///
/// 値そのものは方位を表す（描画で崖面のクアッドをどちら向きに立てるかに使う）。
pub mod cliff_bits {
    pub const NORTH: u8 = 1 << 0; // y - 1 側
    pub const EAST: u8 = 1 << 1; // x + 1 側
    pub const SOUTH: u8 = 1 << 2; // y + 1 側
    pub const WEST: u8 = 1 << 3; // x - 1 側
}

/// 崖として扱う隣接セル間の高低差（cm）。仕様 03 章 2.5 節。
pub const CLIFF_THRESHOLD_CM: i32 = 250;

/// 渡渉可能な最大水深（cm）。これ以下の河川セルが渡渉点になる。
pub const FORD_MAX_DEPTH_CM: u16 = 40;
/// 浅瀬の上限水深（cm）。これを超えると徒歩でも渡れない。
pub const SHALLOW_MAX_DEPTH_CM: u16 = 150;

/// 会戦地の候補。生成器（`web/src/terrain/battle-site.ts`）が評価した結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BattleSiteCandidate {
    pub x_m: i32,
    pub y_m: i32,
    pub score: i32,
    /// 展開可能面積の比率（1000 分率）
    pub passable_permille: u16,
    /// 地形の非対称性（1000 分率、大きいほど南北で地形差がある）
    pub asymmetry_permille: u16,
    /// 視界の開け具合（1000 分率）
    pub openness_permille: u16,
    /// ボトルネック（渡渉点・橋・浅瀬）のサンプル数
    pub bottleneck_count: u16,
}

/// 生成器から受け取る素のグリッド一式。
///
/// 導出グリッド（傾斜・通行コスト）は含まない——[`Terrain::from_grids`] が
/// 効果テーブルから計算する。実行時の判定は Rust 側のテーブルを使うため、
/// 生成器が計算した通行コストをそのまま信用してはいけない。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerrainGrids {
    pub dim: u32,
    pub cell_m: u32,
    pub seed: u64,
    pub height: Vec<i16>,
    pub ground: Vec<u8>,
    pub vegetation: Vec<u8>,
    pub overlay: Vec<u8>,
    /// 水深（cm）。0 = 陸地
    pub water: Vec<u16>,
    pub water_kind: Vec<u8>,
    pub moisture: Vec<u8>,
    pub battle_sites: Vec<BattleSiteCandidate>,
}

/// 地形。各グリッドは行優先（`y * dim + x`）。
pub struct Terrain {
    pub dim: u32,
    pub cell_m: u32,
    pub seed: u64,
    /// 標高（cm）
    pub height: Vec<i16>,
    /// 地質（[`Ground`] の discriminant）
    pub ground: Vec<u8>,
    /// 植生（[`Vegetation`] の discriminant）
    pub vegetation: Vec<u8>,
    /// 人工物（[`Overlay`] の discriminant）
    pub overlay: Vec<u8>,
    /// 水深（cm）。0 = 陸地
    pub water: Vec<u16>,
    /// 水域の種類（[`WaterKind`] の discriminant）
    pub water_kind: Vec<u8>,
    /// 湿度（0..255）。泥濘化と植生が読む
    pub moisture: Vec<u8>,
    /// 崖エッジ（[`cliff_bits`] のビットマスク）。標高からの導出値
    pub cliff: Vec<u8>,
    /// 隣接 4 セルとの最大高低差（cm）。標高からの導出値
    pub slope: Vec<u16>,
    /// 通行コスト。0 = 不通、255 = 最速。効果テーブルからの導出値
    pub passability: Vec<u8>,
    /// 会戦に適した場所の候補（評価順、上位のみ）
    pub battle_sites: Vec<BattleSiteCandidate>,
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
    /// 生成器のグリッドから地形を組み立て、導出グリッドを計算する。
    ///
    /// 長さの合わない・空のグリッドは、その軸の既定値で埋める。ワーカーから
    /// 届く配列は外部入力なので、ここで一度だけ整えてから中に入れる。
    pub fn from_grids(g: TerrainGrids) -> Terrain {
        let dim = g.dim.max(1);
        let n = (dim as usize) * (dim as usize);

        fn fit<T: Clone + Default>(mut v: Vec<T>, n: usize) -> Vec<T> {
            v.resize(n, T::default());
            v
        }

        let mut t = Terrain {
            dim,
            cell_m: g.cell_m.max(1),
            seed: g.seed,
            height: fit(g.height, n),
            ground: fit(g.ground, n),
            vegetation: fit(g.vegetation, n),
            overlay: fit(g.overlay, n),
            water: fit(g.water, n),
            water_kind: fit(g.water_kind, n),
            moisture: fit(g.moisture, n),
            cliff: vec![0; n],
            slope: vec![0; n],
            passability: vec![0; n],
            battle_sites: g.battle_sites,
        };
        t.recompute_derived();
        t
    }

    /// 一様な平原を作る。
    ///
    /// **これは地形生成ではない。** 生成器は `web/src/terrain` にあり、
    /// ここは定数で埋めるだけ。地形そのものが主題ではないテスト（衝突解決、
    /// 隊列、白兵戦）と、固定地形を読めなかったときの退避先に使う。
    /// 起伏・水・森のある地形が要るテストは `fixture::load_scenario` を使うこと。
    pub fn flat(dim: u32, cell_m: u32, seed: u64) -> Terrain {
        let n = (dim.max(1) as usize).pow(2);
        Terrain::from_grids(TerrainGrids {
            dim,
            cell_m,
            seed,
            height: vec![0; n],
            ground: vec![Ground::Grass as u8; n],
            vegetation: vec![Vegetation::ShortGrass as u8; n],
            overlay: vec![0; n],
            water: vec![0; n],
            water_kind: vec![0; n],
            moisture: vec![110; n],
            battle_sites: Vec::new(),
        })
    }

    /// 一様な傾斜地を作る。標高は `x` と `y` に比例して直線的に上がる。
    ///
    /// [`Terrain::flat`] と同じくこれは地形生成ではない。**完全な水平面は
    /// 現実の会戦場には無い**——地形が主題ではないテストでも、水平面は
    /// 兵の前後関係や向きの判断で当たり前に生じる差を消してしまうので、
    /// わずかな傾きを既定にしてある（実際、水平面では突撃命令が
    /// 前進を始めないことが確認されている）。
    pub fn sloped(dim: u32, cell_m: u32, seed: u64, rise_cm_per_cell: i32) -> Terrain {
        let dim = dim.max(1);
        let n = (dim as usize).pow(2);
        let mut height = vec![0i16; n];
        for y in 0..dim as i32 {
            for x in 0..dim as i32 {
                let h = (x + y) * rise_cm_per_cell;
                height[(y * dim as i32 + x) as usize] =
                    h.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            }
        }
        Terrain::from_grids(TerrainGrids {
            dim,
            cell_m,
            seed,
            height,
            ground: vec![Ground::Grass as u8; n],
            vegetation: vec![Vegetation::ShortGrass as u8; n],
            overlay: vec![0; n],
            water: vec![0; n],
            water_kind: vec![0; n],
            moisture: vec![110; n],
            battle_sites: Vec::new(),
        })
    }

    #[inline]
    pub fn idx(&self, x: u32, y: u32) -> usize {
        (y * self.dim + x) as usize
    }

    /// マップの一辺（m）。
    #[inline]
    pub fn size_m(&self) -> u32 {
        self.dim * self.cell_m
    }

    /// セル座標の標高（cm）。範囲外はクランプ。
    #[inline]
    pub fn height_at_cell(&self, x: i32, y: i32) -> i16 {
        let d = self.dim as i32 - 1;
        let cx = x.clamp(0, d) as u32;
        let cy = y.clamp(0, d) as u32;
        self.height[self.idx(cx, cy)]
    }

    /// ワールド座標をセル座標に写す（範囲内にクランプ）。
    #[inline]
    pub fn world_to_cell(&self, x: Fx, y: Fx) -> (u32, u32) {
        let cell = self.cell_m as i32;
        let cx = (sim_math::fx_floor_int(x) / cell).clamp(0, self.dim as i32 - 1);
        let cy = (sim_math::fx_floor_int(y) / cell).clamp(0, self.dim as i32 - 1);
        (cx as u32, cy as u32)
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

    #[inline]
    pub fn ground_at_index(&self, idx: usize) -> Ground {
        Ground::from_u8(self.ground[idx])
    }

    #[inline]
    pub fn vegetation_at_index(&self, idx: usize) -> Vegetation {
        Vegetation::from_u8(self.vegetation[idx])
    }

    #[inline]
    pub fn overlay_at_index(&self, idx: usize) -> Overlay {
        Overlay::from_u8(self.overlay[idx])
    }

    #[inline]
    pub fn water_kind_at_index(&self, idx: usize) -> WaterKind {
        WaterKind::from_u8(self.water_kind[idx])
    }

    /// セルの合成済み地形効果。移動・隊列・遮蔽・防御・疲労・騎兵をまとめて返す。
    #[inline]
    pub fn effect_at_index(&self, idx: usize) -> TerrainEffect {
        compose_effect(
            self.ground_at_index(idx),
            self.vegetation_at_index(idx),
            self.water[idx],
            self.overlay_at_index(idx),
        )
    }

    /// ワールド座標（Fx, m）の地形効果。
    #[inline]
    pub fn effect_at(&self, x: Fx, y: Fx) -> TerrainEffect {
        let (cx, cy) = self.world_to_cell(x, y);
        self.effect_at_index(self.idx(cx, cy))
    }

    /// 水深（cm）。0 = 陸地。
    #[inline]
    pub fn water_depth_cm_at_cell(&self, x: u32, y: u32) -> u16 {
        self.water[self.idx(x, y)]
    }

    /// 徒渉できない深さか。
    #[inline]
    pub fn is_deep_water(&self, idx: usize) -> bool {
        self.water[idx] > SHALLOW_MAX_DEPTH_CM
    }

    /// 渡渉点か（河川で、徒歩なら渡れる深さ）。
    #[inline]
    pub fn is_ford(&self, idx: usize) -> bool {
        self.water[idx] > 0
            && self.water[idx] <= FORD_MAX_DEPTH_CM
            && self.water_kind_at_index(idx) == WaterKind::River
    }

    /// 崖ビットマスク（デバッグ・描画用）。
    #[inline]
    pub fn cliff_at_cell(&self, x: u32, y: u32) -> u8 {
        self.cliff[self.idx(x, y)]
    }

    /// 1 セルの地質を書き換え、通行コストを整合させる。変更前の値を返す。
    pub fn set_ground(&mut self, idx: usize, ground: Ground) -> Ground {
        let prev = self.ground_at_index(idx);
        self.ground[idx] = ground as u8;
        self.refresh_passability(idx);
        prev
    }

    /// 1 セルの植生を書き換え、通行コストを整合させる。変更前の値を返す。
    ///
    /// 工兵の伐採（射界の確保、仕様 07 章 3 節）はこれだけを呼ぶ——
    /// 木を切っても下の土は変わらない。
    pub fn set_vegetation(&mut self, idx: usize, vegetation: Vegetation) -> Vegetation {
        let prev = self.vegetation_at_index(idx);
        self.vegetation[idx] = vegetation as u8;
        self.refresh_passability(idx);
        prev
    }

    /// 1 セルの人工物を書き換え、通行コストを整合させる。変更前の値を返す。
    ///
    /// 架橋・破壊工作（橋を落とす）がこれを使う。
    pub fn set_overlay(&mut self, idx: usize, overlay: Overlay) -> Overlay {
        let prev = self.overlay_at_index(idx);
        self.overlay[idx] = overlay as u8;
        self.refresh_passability(idx);
        prev
    }

    /// 1 セルの水深と種別を書き換え、通行コストを整合させる。
    pub fn set_water(&mut self, idx: usize, depth_cm: u16, kind: WaterKind) {
        self.water[idx] = depth_cm;
        self.water_kind[idx] = kind as u8;
        self.refresh_passability(idx);
    }

    /// 1 セルの通行コストを、いまの属性と傾斜から取り直す。
    #[inline]
    fn refresh_passability(&mut self, idx: usize) {
        self.passability[idx] = passability_with_slope(
            self.ground_at_index(idx),
            self.vegetation_at_index(idx),
            self.water[idx],
            self.overlay_at_index(idx),
            self.slope[idx],
            self.cell_m,
        );
    }

    /// 標高を書き換えたあとに、崖・傾斜・通行コストを取り直す。
    ///
    /// 工兵の掘削・土塁（仕様 07 章 3 節）のように標高を触ったときは、
    /// セル 1 つの更新では周囲の傾斜が合わなくなるので全体を取り直す。
    pub fn recompute_derived(&mut self) {
        self.compute_cliffs();
        self.compute_slope();
        for idx in 0..self.height.len() {
            self.refresh_passability(idx);
        }
    }

    fn compute_cliffs(&mut self) {
        let dim = self.dim as i32;
        for y in 0..dim {
            for x in 0..dim {
                let idx = (y * dim + x) as usize;
                let here = self.height[idx] as i32;
                let mut bits = 0u8;
                if y > 0
                    && (here - self.height_at_cell(x, y - 1) as i32).abs() >= CLIFF_THRESHOLD_CM
                {
                    bits |= cliff_bits::NORTH;
                }
                if x < dim - 1
                    && (here - self.height_at_cell(x + 1, y) as i32).abs() >= CLIFF_THRESHOLD_CM
                {
                    bits |= cliff_bits::EAST;
                }
                if y < dim - 1
                    && (here - self.height_at_cell(x, y + 1) as i32).abs() >= CLIFF_THRESHOLD_CM
                {
                    bits |= cliff_bits::SOUTH;
                }
                if x > 0
                    && (here - self.height_at_cell(x - 1, y) as i32).abs() >= CLIFF_THRESHOLD_CM
                {
                    bits |= cliff_bits::WEST;
                }
                self.cliff[idx] = bits;
            }
        }
    }

    fn compute_slope(&mut self) {
        let dim = self.dim as i32;
        for y in 0..dim {
            for x in 0..dim {
                let idx = (y * dim + x) as usize;
                let here = self.height[idx] as i32;
                let mut max = 0i32;
                for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                    let there = self.height_at_cell(x + dx, y + dy) as i32;
                    max = max.max((here - there).abs());
                }
                self.slope[idx] = max.min(u16::MAX as i32) as u16;
            }
        }
    }

    /// 生成物の同一性を検証するためのハッシュ。
    pub fn hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |v: u64| {
            h ^= v;
            h = h.wrapping_mul(0x0100_0000_01b3);
        };
        for v in &self.height {
            mix(*v as u16 as u64);
        }
        for v in &self.ground {
            mix(*v as u64);
        }
        for v in &self.vegetation {
            mix(*v as u64);
        }
        for v in &self.overlay {
            mix(*v as u64);
        }
        for v in &self.water {
            mix(*v as u64);
        }
        for v in &self.cliff {
            mix(*v as u64);
        }
        h
    }
}

/// cm を Fx（m）に変換する。
#[inline]
fn fx_cm(cm: i16) -> Fx {
    fx_from_mm(cm as i32 * 10)
}

/// 地形の健全性を検査する。テストと CI で使う。
///
/// - グリッド長の一致
/// - 通行可能セルの比率
/// - **通行可能領域が大きく分断されていない**（最大連結成分が通行可能セル全体の
///   90% 以上を占める）
pub fn validate(t: &Terrain) -> Result<(), String> {
    let n = t.height.len();
    if t.ground.len() != n
        || t.vegetation.len() != n
        || t.overlay.len() != n
        || t.passability.len() != n
        || t.water.len() != n
        || t.water_kind.len() != n
        || t.cliff.len() != n
        || t.slope.len() != n
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

/// デバッグ用の地形統計。
#[derive(Clone, Debug)]
pub struct TerrainStats {
    pub cells: usize,
    pub ground_counts: [u32; Ground::COUNT],
    pub vegetation_counts: [u32; Vegetation::COUNT],
    pub water_cells: u32,
    pub river_cells: u32,
    pub lake_cells: u32,
    pub sea_cells: u32,
    pub road_cells: u32,
    pub bridge_cells: u32,
    pub cliff_cells: u32,
    pub passable_cells: u32,
    pub min_height_cm: i16,
    pub max_height_cm: i16,
}

pub fn stats(t: &Terrain) -> TerrainStats {
    let mut s = TerrainStats {
        cells: t.height.len(),
        ground_counts: [0; Ground::COUNT],
        vegetation_counts: [0; Vegetation::COUNT],
        water_cells: 0,
        river_cells: 0,
        lake_cells: 0,
        sea_cells: 0,
        road_cells: 0,
        bridge_cells: 0,
        cliff_cells: 0,
        passable_cells: 0,
        min_height_cm: t.height.iter().copied().min().unwrap_or(0),
        max_height_cm: t.height.iter().copied().max().unwrap_or(0),
    };
    for idx in 0..t.height.len() {
        s.ground_counts[t.ground_at_index(idx) as usize] += 1;
        s.vegetation_counts[t.vegetation_at_index(idx) as usize] += 1;
        if t.water[idx] > 0 {
            s.water_cells += 1;
        }
        match t.water_kind_at_index(idx) {
            WaterKind::River => s.river_cells += 1,
            WaterKind::Lake => s.lake_cells += 1,
            WaterKind::Sea => s.sea_cells += 1,
            WaterKind::None => {}
        }
        match t.overlay_at_index(idx) {
            Overlay::Road => s.road_cells += 1,
            Overlay::Bridge => s.bridge_cells += 1,
            Overlay::None => {}
        }
        if t.cliff[idx] != 0 {
            s.cliff_cells += 1;
        }
        if t.passability[idx] > 0 {
            s.passable_cells += 1;
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一様な平原。属性を触るテストの土台。
    fn flat(dim: u32) -> Terrain {
        let n = (dim as usize) * (dim as usize);
        Terrain::from_grids(TerrainGrids {
            dim,
            cell_m: 2,
            seed: 7,
            height: vec![800; n],
            ground: vec![Ground::Grass as u8; n],
            vegetation: vec![Vegetation::ShortGrass as u8; n],
            overlay: vec![0; n],
            water: vec![0; n],
            water_kind: vec![0; n],
            moisture: vec![120; n],
            battle_sites: Vec::new(),
        })
    }

    #[test]
    fn from_grids_derives_passability_and_slope() {
        let t = flat(16);
        assert!(t.slope.iter().all(|&s| s == 0));
        assert!(t.passability.iter().all(|&p| p > 0));
        assert!(validate(&t).is_ok());
    }

    #[test]
    fn short_grids_are_padded_to_the_grid_size() {
        // ワーカーから届く配列は外部入力なので、足りなくても落ちてはいけない
        let t = Terrain::from_grids(TerrainGrids {
            dim: 4,
            cell_m: 2,
            height: vec![100; 3],
            ..Default::default()
        });
        assert_eq!(t.height.len(), 16);
        assert_eq!(t.ground.len(), 16);
        assert_eq!(t.height[3], 0);
    }

    #[test]
    fn felling_trees_keeps_the_ground_underneath() {
        let mut t = flat(8);
        let idx = t.idx(3, 3);
        t.set_ground(idx, Ground::Mud);
        t.set_vegetation(idx, Vegetation::Dense);
        let before = t.passability[idx];

        let prev = t.set_vegetation(idx, Vegetation::None);
        assert_eq!(prev, Vegetation::Dense);
        // 土は泥のまま。木を切っても地質は変わらない
        assert_eq!(t.ground_at_index(idx), Ground::Mud);
        assert!(t.passability[idx] > before, "伐採で通りやすくなるはず");
    }

    #[test]
    fn a_bridge_makes_deep_water_passable_and_removing_it_undoes_that() {
        let mut t = flat(8);
        let idx = t.idx(2, 2);
        t.set_water(idx, 400, WaterKind::River);
        assert_eq!(t.passability[idx], 0, "深い水は通れない");

        t.set_overlay(idx, Overlay::Bridge);
        assert!(t.passability[idx] > 0, "橋を架ければ通れる");

        t.set_overlay(idx, Overlay::None);
        assert_eq!(t.passability[idx], 0, "橋を落とせば元に戻る");
    }

    #[test]
    fn cliffs_are_detected_from_height_differences() {
        let dim = 8;
        let n = (dim * dim) as usize;
        let mut height = vec![0i16; n];
        // x >= 4 を 3 m 持ち上げる。x=3 と x=4 の間が崖になる
        for y in 0..dim {
            for x in 4..dim {
                height[(y * dim + x) as usize] = 300;
            }
        }
        let t = Terrain::from_grids(TerrainGrids {
            dim,
            cell_m: 2,
            height,
            ..Default::default()
        });
        assert_ne!(t.cliff_at_cell(3, 2) & cliff_bits::EAST, 0);
        assert_ne!(t.cliff_at_cell(4, 2) & cliff_bits::WEST, 0);
        assert_eq!(t.cliff_at_cell(1, 2), 0);
    }

    #[test]
    fn ford_is_a_shallow_river_not_a_shallow_lake() {
        let mut t = flat(8);
        let river = t.idx(1, 1);
        let lake = t.idx(2, 1);
        t.set_water(river, 30, WaterKind::River);
        t.set_water(lake, 30, WaterKind::Lake);
        assert!(t.is_ford(river));
        assert!(!t.is_ford(lake));
    }

    #[test]
    fn stats_counts_every_axis() {
        let mut t = flat(8);
        let idx = t.idx(0, 0);
        t.set_overlay(idx, Overlay::Road);
        t.set_water(t.idx(1, 0), 200, WaterKind::Sea);
        let s = stats(&t);
        assert_eq!(s.road_cells, 1);
        assert_eq!(s.sea_cells, 1);
        assert_eq!(s.water_cells, 1);
        // 道路も水も植生の軸には触らない
        assert_eq!(s.vegetation_counts[Vegetation::ShortGrass as usize], 64);
    }
}
