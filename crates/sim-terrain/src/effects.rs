//! 地形が兵士に与える効果。仕様 `docs/spec/03-terrain.md` 3 節。
//!
//! **効果値の正本は `web/src/terrain/effects.ts`** で、ここはその写しである。
//! 地形の生成は JS 側に移ったので、生成時に道路が選ぶ経路と、シミュレーション
//! 中の移動速度は、同じ数字から出ていなければならない。食い違うと「地形は
//! 見た目どおりなのに兵が動かない」という形で表面化する。
//! `tools/check_terrain_effects.mjs` が両ファイルを突き合わせて CI で落とす。
//!
//! 効果は 4 つの軸から**合成**する。統合された地表 ID を引くのではなく、
//! 地質・植生・水・人工物のそれぞれが独立に倍率を持ち、掛け合わせる。
//! これにより「砂地の疎林」のような、テーブルに書いていない組み合わせも
//! 自動的に意味のある値になる。

use crate::{Ground, Overlay, Vegetation, FORD_MAX_DEPTH_CM, SHALLOW_MAX_DEPTH_CM};

/// 地形の効果。すべて 1000 分率の倍率。1000 = 等倍。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainEffect {
    /// 移動速度倍率
    pub move_mult: u16,
    /// 隊列維持のしやすさ
    pub cohesion_mult: u16,
    /// 視界遮蔽率（0..1000）。合成では軸の最大値を採る
    pub cover: u16,
    /// 防御倍率
    pub defense_mult: u16,
    /// 疲労消費倍率
    pub fatigue_mult: u16,
    /// 騎兵の可否（0 = 進入不可、1000 = 制限なし）
    pub cavalry_mult: u16,
}

const fn eff(
    move_mult: u16,
    cohesion_mult: u16,
    cover: u16,
    defense_mult: u16,
    fatigue_mult: u16,
    cavalry_mult: u16,
) -> TerrainEffect {
    TerrainEffect {
        move_mult,
        cohesion_mult,
        cover,
        defense_mult,
        fatigue_mult,
        cavalry_mult,
    }
}

/// 効果を持たない中立値。合成の単位元。
pub const NEUTRAL: TerrainEffect = eff(1000, 1000, 0, 1000, 1000, 1000);

/// 地質の効果。[`Ground`] の並びと 1 対 1。
pub static GROUND_EFFECTS: [TerrainEffect; Ground::COUNT] = [
    eff(1000, 1000, 0, 1000, 1000, 1000), // Soil
    eff(1000, 1000, 0, 1000, 1000, 1000), // Grass
    eff(850, 850, 0, 1000, 1250, 700),    // Sand
    eff(700, 600, 100, 1100, 1400, 500),  // Rock
    eff(500, 600, 0, 900, 1900, 400),     // Mud
    eff(1000, 1000, 0, 1000, 1000, 1000), // RiverBed（水そのものの penalty は水軸が持つ）
    eff(1000, 1000, 0, 1000, 1000, 1000), // SeaBed
    eff(600, 450, 0, 1000, 1500, 0),      // Scree
    eff(350, 400, 200, 800, 2200, 0),     // Marsh
];

/// 植生の効果。[`Vegetation`] の並びと 1 対 1。
pub static VEGETATION_EFFECTS: [TerrainEffect; Vegetation::COUNT] = [
    eff(1000, 1000, 0, 1000, 1000, 1000), // None
    eff(1000, 1000, 0, 1000, 1000, 1000), // ShortGrass
    eff(850, 800, 300, 1050, 1150, 500),  // Sparse（疎林。騎兵は通れるが速度が落ちる）
    eff(650, 500, 600, 1100, 1300, 0),    // Woodland
    eff(400, 250, 900, 1200, 1600, 0),    // Dense
    eff(800, 750, 250, 1000, 1200, 700),  // Scrub
    eff(900, 900, 150, 1000, 1100, 1000), // Meadow
    eff(950, 950, 50, 1000, 1050, 1000),  // Farmland
];

/// 人工物の効果。**合成ではなく上書き**する。
///
/// 道は森を切り開いて敷くものなので、路面の上に林の減速が残るのはおかしい。
/// 橋も同じで、下の水深は移動に効かない。
pub static OVERLAY_EFFECTS: [TerrainEffect; Overlay::COUNT] = [
    NEUTRAL,                             // None（使われない）
    eff(1150, 1100, 0, 1000, 900, 1000), // Road
    eff(1000, 1000, 0, 900, 1000, 1000), // Bridge
];

/// 水深の効果。深さで 3 段に分かれる。
///
/// 浅瀬の防御 600 が意味するもの: 川を渡っている最中の部隊は隊列が崩れ、
/// 防御が 4 割落ちる。渡渉中に襲われると壊滅する。中世会戦の定番だった
/// 「渡渉点を押さえる」戦術が、この 1 行から出る。
#[inline]
pub fn water_effect(depth_cm: u16) -> TerrainEffect {
    if depth_cm == 0 {
        NEUTRAL
    } else if depth_cm <= FORD_MAX_DEPTH_CM {
        eff(550, 400, 0, 700, 1500, 900)
    } else if depth_cm <= SHALLOW_MAX_DEPTH_CM {
        eff(400, 300, 0, 600, 1800, 300)
    } else {
        eff(0, 0, 0, 1000, 1000, 0)
    }
}

#[inline]
fn mul3(a: u16, b: u16, c: u16) -> u16 {
    ((a as u32 * b as u32 * c as u32) / 1_000_000).min(u16::MAX as u32) as u16
}

/// 4 軸から効果を合成する。
///
/// 倍率は掛け算、遮蔽だけは最大値を採る（林の中の泥地は、泥のぶん遅くなるが
/// 遮蔽は林のぶんだけあり、足し合わさりはしない）。
pub fn compose_effect(
    ground: Ground,
    vegetation: Vegetation,
    water_depth_cm: u16,
    overlay: Overlay,
) -> TerrainEffect {
    if overlay != Overlay::None {
        return OVERLAY_EFFECTS[overlay as usize];
    }
    let g = &GROUND_EFFECTS[ground as usize];
    let v = &VEGETATION_EFFECTS[vegetation as usize];
    let w = water_effect(water_depth_cm);
    TerrainEffect {
        move_mult: mul3(g.move_mult, v.move_mult, w.move_mult),
        cohesion_mult: mul3(g.cohesion_mult, v.cohesion_mult, w.cohesion_mult),
        cover: g.cover.max(v.cover).max(w.cover),
        defense_mult: mul3(g.defense_mult, v.defense_mult, w.defense_mult),
        fatigue_mult: mul3(g.fatigue_mult, v.fatigue_mult, w.fatigue_mult),
        cavalry_mult: mul3(g.cavalry_mult, v.cavalry_mult, w.cavalry_mult),
    }
}

/// 傾斜を無視した通行コストの基準値（0 = 不通、255 = 最速）。
///
/// 工兵の地形改修（架橋・伐採、仕様 07 章 3 節）が、全セルの再走査なしに
/// 周辺と整合した通行コストを付けられるようにするための版。
#[inline]
pub fn base_passability(
    ground: Ground,
    vegetation: Vegetation,
    water_depth_cm: u16,
    overlay: Overlay,
) -> u8 {
    let e = compose_effect(ground, vegetation, water_depth_cm, overlay);
    if e.move_mult == 0 {
        return 0;
    }
    ((e.move_mult as u32 * 255 / 1150).clamp(1, 255)) as u8
}

/// 傾斜のペナルティを織り込んだ通行コスト。
///
/// `slope_cm` は隣接 4 セルとの最大高低差（cm）、`cell_m` はセルの一辺（m）。
/// tan 0.55 を超える斜面は通行不能（仕様 03 章 3.1 節）。
pub fn passability_with_slope(
    ground: Ground,
    vegetation: Vegetation,
    water_depth_cm: u16,
    overlay: Overlay,
    slope_cm: u16,
    cell_m: u32,
) -> u8 {
    let base = base_passability(ground, vegetation, water_depth_cm, overlay);
    if base == 0 {
        return 0;
    }
    let slope_percent = slope_cm as u32 * 100 / (cell_m * 100);
    if slope_percent > 55 {
        return 0;
    }
    let base = base as u32;
    ((base - base * slope_percent.min(55) / 100).clamp(1, 255)) as u8
}

/// 掘削・整地の作業速度倍率（1000 分率）。仕様 07 章 3 節。
pub fn work_mult_permille(ground: Ground, vegetation: Vegetation, depth_cm: u16) -> i32 {
    if depth_cm > SHALLOW_MAX_DEPTH_CM {
        return 0;
    }
    match (ground, vegetation) {
        (Ground::Rock | Ground::Scree, _) => 400,
        (_, Vegetation::Dense) => 700,
        (_, Vegetation::Woodland | Vegetation::Scrub) => 850,
        (Ground::Mud | Ground::Marsh, _) => 1_300,
        (Ground::Sand, _) => 900,
        _ if depth_cm > 0 => 900,
        _ => 1_000,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4軸化した時点で固定した代表16ケースの効果値が、現在の合成から
    /// **そのまま再現される**ことを確かめる。ここが崩れると、同じ地形で
    /// 兵の動きだけが変わってしまう。
    #[test]
    fn composition_matches_the_reference_surface_cases() {
        let cases: &[(&str, Ground, Vegetation, u16, Overlay, TerrainEffect)] = &[
            (
                "Grass",
                Ground::Grass,
                Vegetation::ShortGrass,
                0,
                Overlay::None,
                eff(1000, 1000, 0, 1000, 1000, 1000),
            ),
            (
                "Meadow",
                Ground::Grass,
                Vegetation::Meadow,
                0,
                Overlay::None,
                eff(900, 900, 150, 1000, 1100, 1000),
            ),
            (
                "Farmland",
                Ground::Soil,
                Vegetation::Farmland,
                0,
                Overlay::None,
                eff(950, 950, 50, 1000, 1050, 1000),
            ),
            (
                "Scrub",
                Ground::Soil,
                Vegetation::Scrub,
                0,
                Overlay::None,
                eff(800, 750, 250, 1000, 1200, 700),
            ),
            (
                "LightForest",
                Ground::Soil,
                Vegetation::Woodland,
                0,
                Overlay::None,
                eff(650, 500, 600, 1100, 1300, 0),
            ),
            (
                "DenseForest",
                Ground::Soil,
                Vegetation::Dense,
                0,
                Overlay::None,
                eff(400, 250, 900, 1200, 1600, 0),
            ),
            (
                "Rock",
                Ground::Rock,
                Vegetation::None,
                0,
                Overlay::None,
                eff(700, 600, 100, 1100, 1400, 500),
            ),
            (
                "Scree",
                Ground::Scree,
                Vegetation::None,
                0,
                Overlay::None,
                eff(600, 450, 0, 1000, 1500, 0),
            ),
            (
                "Sand",
                Ground::Sand,
                Vegetation::None,
                0,
                Overlay::None,
                eff(850, 850, 0, 1000, 1250, 700),
            ),
            (
                "Mud",
                Ground::Mud,
                Vegetation::None,
                0,
                Overlay::None,
                eff(500, 600, 0, 900, 1900, 400),
            ),
            (
                "Marsh",
                Ground::Marsh,
                Vegetation::None,
                0,
                Overlay::None,
                eff(350, 400, 200, 800, 2200, 0),
            ),
            (
                "Ford",
                Ground::RiverBed,
                Vegetation::None,
                30,
                Overlay::None,
                eff(550, 400, 0, 700, 1500, 900),
            ),
            (
                "ShallowWater",
                Ground::RiverBed,
                Vegetation::None,
                100,
                Overlay::None,
                eff(400, 300, 0, 600, 1800, 300),
            ),
            (
                "DeepWater",
                Ground::SeaBed,
                Vegetation::None,
                400,
                Overlay::None,
                eff(0, 0, 0, 1000, 1000, 0),
            ),
            (
                "Road",
                Ground::Soil,
                Vegetation::None,
                0,
                Overlay::Road,
                eff(1150, 1100, 0, 1000, 900, 1000),
            ),
            (
                "Bridge",
                Ground::RiverBed,
                Vegetation::None,
                200,
                Overlay::Bridge,
                eff(1000, 1000, 0, 900, 1000, 1000),
            ),
        ];
        for (name, g, v, depth, o, expected) in cases {
            assert_eq!(
                compose_effect(*g, *v, *depth, *o),
                *expected,
                "{name} の効果が基準値と違う"
            );
        }
    }

    #[test]
    fn unlisted_combinations_compose_sensibly() {
        // 砂地の疎林。旧 `Surface` には無かった組み合わせ
        let e = compose_effect(Ground::Sand, Vegetation::Sparse, 0, Overlay::None);
        assert_eq!(e.move_mult, (850u32 * 850 / 1000) as u16);
        // 遮蔽は最大値。砂（0）と疎林（300）なら 300
        assert_eq!(e.cover, 300);
        // 騎兵は掛け算で両方のペナルティを受ける
        assert_eq!(e.cavalry_mult, (700u32 * 500 / 1000) as u16);
    }

    #[test]
    fn overlay_overrides_everything_under_it() {
        // 密林に敷いた道は、道として通れる
        let road = compose_effect(Ground::Mud, Vegetation::Dense, 0, Overlay::Road);
        assert_eq!(road, OVERLAY_EFFECTS[Overlay::Road as usize]);
        // 深い水に架けた橋も同じ
        let bridge = compose_effect(Ground::SeaBed, Vegetation::None, 500, Overlay::Bridge);
        assert_eq!(bridge.move_mult, 1000);
    }

    #[test]
    fn deep_water_is_impassable_regardless_of_slope() {
        let p = passability_with_slope(
            Ground::SeaBed,
            Vegetation::None,
            SHALLOW_MAX_DEPTH_CM + 1,
            Overlay::None,
            0,
            2,
        );
        assert_eq!(p, 0);
    }

    #[test]
    fn steep_slope_blocks_movement() {
        // 2 m セルで 120 cm の段差 = 60% > tan 0.55
        let p = passability_with_slope(
            Ground::Grass,
            Vegetation::ShortGrass,
            0,
            Overlay::None,
            120,
            2,
        );
        assert_eq!(p, 0);
    }
}
