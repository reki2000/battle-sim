//! シナリオによる地形の整形（stamping）。
//!
//! 手続き生成された地形は「それらしい野原」までしか作れない。史実の会戦を
//! 再現するには、その上に**その会戦固有の地勢**——森の縁、雨で泥濘化した耕地、
//! 緩い登り——を焼き込む必要がある。ここはそのための最小の道具立てで、
//! シナリオが宣言的に並べた [`Stamp`] を会戦開始前に一度だけ適用する。
//!
//! 仕様 03 章 4 節「地形の動的変更」が工兵と戦闘によるランタイムの書き換えを
//! 扱うのに対し、こちらは**生成直後の 1 回だけ**走る。書き換えるグリッドは
//! 同じ（`surface` / `height`）で、適用後に派生グリッド（崖・通行コスト）を
//! [`crate::recompute_derived`] で整合させる。
//!
//! 浮動小数点は使わない。座標は m 単位の整数、標高は cm 単位の整数で扱う。

use crate::{Surface, Terrain};

/// [`Stamp::HeightRamp`] が標高を変える軸。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RampAxis {
    X,
    Y,
}

/// 地形へ焼き込む 1 つの地勢。座標はすべてワールド座標の m。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stamp {
    /// 軸並行の矩形を 1 種類の地表で塗る（耕地、泥濘の帯など）。
    SurfaceRect {
        surface: Surface,
        x_m: i32,
        y_m: i32,
        w_m: i32,
        h_m: i32,
    },
    /// 線分 `a`-`b` から `half_width_m` 以内を塗る帯。斜めに走る森の縁を
    /// 表すために使う（矩形では会戦場が狭まっていく形を作れない）。
    SurfaceBelt {
        surface: Surface,
        ax_m: i32,
        ay_m: i32,
        bx_m: i32,
        by_m: i32,
        half_width_m: i32,
    },
    /// `axis` 方向に沿って標高へ緩い勾配を足す。`from_m` 以前は `from_cm`、
    /// `to_m` 以降は `to_cm` で一定なので、どこにも段差ができない
    /// （矩形に限定すると境界が崖になってしまう）。
    HeightRamp {
        axis: RampAxis,
        from_m: i32,
        to_m: i32,
        from_cm: i32,
        to_cm: i32,
    },
}

/// スタンプを順に適用し、派生グリッドを整合させる。
///
/// 水域（`water > 0`）のセルは地表を塗り替えない。水深グリッドと地表が食い違うと
/// 描画も水系ロジックも壊れるため、川や湖を潰したいシナリオは地形パラメータ側
/// （`river_density` など）で消すこと。
pub fn apply(t: &mut Terrain, stamps: &[Stamp]) {
    if stamps.is_empty() {
        return;
    }
    for stamp in stamps {
        apply_one(t, *stamp);
    }
    crate::recompute_derived(t);
}

fn apply_one(t: &mut Terrain, stamp: Stamp) {
    match stamp {
        Stamp::SurfaceRect {
            surface,
            x_m,
            y_m,
            w_m,
            h_m,
        } => {
            for_each_cell(t, |x, y| {
                (x >= x_m && x < x_m + w_m && y >= y_m && y < y_m + h_m).then_some(surface)
            });
        }
        Stamp::SurfaceBelt {
            surface,
            ax_m,
            ay_m,
            bx_m,
            by_m,
            half_width_m,
        } => {
            let limit_sq = (half_width_m as i64) * (half_width_m as i64);
            for_each_cell(t, |x, y| {
                (dist_sq_to_segment(x, y, ax_m, ay_m, bx_m, by_m) <= limit_sq).then_some(surface)
            });
        }
        Stamp::HeightRamp {
            axis,
            from_m,
            to_m,
            from_cm,
            to_cm,
        } => {
            let dim = t.dim;
            let cell_m = t.cell_m as i32;
            for cy in 0..dim {
                for cx in 0..dim {
                    let (px, py) = cell_center_m(cx, cy, cell_m);
                    let p = match axis {
                        RampAxis::X => px,
                        RampAxis::Y => py,
                    };
                    let delta = ramp_value_cm(p, from_m, to_m, from_cm, to_cm);
                    let idx = t.idx(cx, cy);
                    t.height[idx] = (t.height[idx] as i32 + delta)
                        .clamp(i16::MIN as i32, i16::MAX as i32)
                        as i16;
                }
            }
        }
    }
}

/// 全セルを走査し、`pick` が地表を返したセルだけを塗り替える。
fn for_each_cell(t: &mut Terrain, pick: impl Fn(i32, i32) -> Option<Surface>) {
    let dim = t.dim;
    let cell_m = t.cell_m as i32;
    for cy in 0..dim {
        for cx in 0..dim {
            let (px, py) = cell_center_m(cx, cy, cell_m);
            let Some(surface) = pick(px, py) else {
                continue;
            };
            let idx = t.idx(cx, cy);
            if t.water[idx] > 0 {
                continue;
            }
            t.set_surface(idx, surface);
        }
    }
}

/// セル中心のワールド座標（m）。
#[inline]
fn cell_center_m(cx: u32, cy: u32, cell_m: i32) -> (i32, i32) {
    (
        cx as i32 * cell_m + cell_m / 2,
        cy as i32 * cell_m + cell_m / 2,
    )
}

/// `from_m`..`to_m` を `from_cm`..`to_cm` で線形補間し、範囲外は端の値で一定。
fn ramp_value_cm(p: i32, from_m: i32, to_m: i32, from_cm: i32, to_cm: i32) -> i32 {
    let (lo, hi, lo_cm, hi_cm) = if from_m <= to_m {
        (from_m, to_m, from_cm, to_cm)
    } else {
        (to_m, from_m, to_cm, from_cm)
    };
    if p <= lo || hi == lo {
        return lo_cm;
    }
    if p >= hi {
        return hi_cm;
    }
    lo_cm + ((hi_cm - lo_cm) as i64 * (p - lo) as i64 / (hi - lo) as i64) as i32
}

/// 点から線分までの距離の 2 乗（m²）。
fn dist_sq_to_segment(px: i32, py: i32, ax: i32, ay: i32, bx: i32, by: i32) -> i64 {
    let abx = (bx - ax) as i64;
    let aby = (by - ay) as i64;
    let apx = (px - ax) as i64;
    let apy = (py - ay) as i64;
    let len_sq = abx * abx + aby * aby;
    if len_sq == 0 {
        return apx * apx + apy * apy;
    }
    // 射影係数を [0, 1] に収める。除算を最後まで遅らせて精度を保つ。
    let dot = (apx * abx + apy * aby).clamp(0, len_sq);
    // 最近点 = a + ab * dot / len_sq。分母を掛けたまま差を取る。
    let cx = apx * len_sq - abx * dot;
    let cy = apy * len_sq - aby * dot;
    let num = cx * cx + cy * cy;
    // num / len_sq² が距離の 2 乗。桁溢れを避けるため 2 段階で割る。
    (num / len_sq) / len_sq
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{generate, SeaEdge, TerrainParams};

    fn flat_params() -> TerrainParams {
        TerrainParams {
            seed: 99,
            size_m: 400,
            cell_m: 2,
            relief: 0,
            forest_cover: 0,
            marsh_bias: 0,
            thermal_iterations: 2,
            river_density: 0,
            road_count: 0,
            sea_edge: SeaEdge::None,
            sea_level_cm: 0,
        }
    }

    #[test]
    fn surface_rect_paints_only_inside() {
        let mut t = generate(&flat_params());
        apply(
            &mut t,
            &[Stamp::SurfaceRect {
                surface: Surface::Mud,
                x_m: 100,
                y_m: 100,
                w_m: 100,
                h_m: 100,
            }],
        );
        let inside = t.idx(75, 75); // (150 m, 150 m)
        let outside = t.idx(25, 25); // (50 m, 50 m)
        assert_eq!(Surface::from_u8(t.surface[inside]), Surface::Mud);
        assert_ne!(Surface::from_u8(t.surface[outside]), Surface::Mud);
        // 地表を変えたら通行コストも追従する（傾斜ぶんの減衰が乗るので
        // 基準値そのものではなく、通行可能かつ元の地表より歩きにくいことを見る）
        assert!(t.passability[inside] > 0);
        assert!(t.passability[inside] < t.passability[outside]);
    }

    #[test]
    fn surface_belt_follows_a_slanted_line() {
        let mut t = generate(&flat_params());
        apply(
            &mut t,
            &[Stamp::SurfaceBelt {
                surface: Surface::DenseForest,
                ax_m: 100,
                ay_m: 0,
                bx_m: 200,
                by_m: 400,
                half_width_m: 20,
            }],
        );
        // 線分は y とともに東へ寄っていく（y=100 で x=125、y=300 で x=175）。
        // その各点は塗られ、そこから 60 m 離れた点は塗られない。水域は
        // 塗り替えの対象外なので判定から外す。
        for (cy, cx_on, cx_off) in [(50u32, 62u32, 32u32), (150, 87, 57)] {
            let on = t.idx(cx_on, cy);
            let off = t.idx(cx_off, cy);
            if t.water[on] == 0 {
                assert_eq!(Surface::from_u8(t.surface[on]), Surface::DenseForest);
            }
            assert_ne!(Surface::from_u8(t.surface[off]), Surface::DenseForest);
        }
    }

    #[test]
    fn height_ramp_tilts_without_creating_a_step() {
        let mut t = generate(&flat_params());
        let before_low = t.height[t.idx(100, 10)];
        let before_high = t.height[t.idx(100, 190)];
        apply(
            &mut t,
            &[Stamp::HeightRamp {
                axis: RampAxis::Y,
                from_m: 0,
                to_m: 400,
                from_cm: 0,
                to_cm: 400,
            }],
        );
        let after_low = t.height[t.idx(100, 10)];
        let after_high = t.height[t.idx(100, 190)];
        // 南（y 小）はほぼそのまま、北（y 大）は約 4 m 持ち上がる
        assert!((after_low - before_low).abs() <= 30);
        assert!((after_high - before_high) >= 350);

        // 隣接セル間の段差は 1 セル（2 m）あたり数 cm に収まる = 崖はできない
        for y in 1..t.dim {
            let a = t.height[t.idx(100, y - 1)] as i32;
            let b = t.height[t.idx(100, y)] as i32;
            assert!((a - b).abs() < 100, "y={y} で段差 {} cm", (a - b).abs());
        }
    }

    #[test]
    fn ramp_value_is_clamped_outside_the_span() {
        assert_eq!(ramp_value_cm(-50, 0, 100, 0, 200), 0);
        assert_eq!(ramp_value_cm(50, 0, 100, 0, 200), 100);
        assert_eq!(ramp_value_cm(500, 0, 100, 0, 200), 200);
        // 逆向きに書いても同じ勾配になる
        assert_eq!(ramp_value_cm(50, 100, 0, 200, 0), 100);
    }

    #[test]
    fn distance_to_segment_matches_known_values() {
        // 線分 (0,0)-(100,0) から (50, 30) までは 30 m
        assert_eq!(dist_sq_to_segment(50, 30, 0, 0, 100, 0), 900);
        // 端点の外側は端点までの距離
        assert_eq!(dist_sq_to_segment(-30, 40, 0, 0, 100, 0), 2500);
    }
}
