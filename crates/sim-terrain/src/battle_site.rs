//! 会戦地の自動評価。
//!
//! 仕様 `docs/spec/03-terrain.md` 2.6 節（段階 14）。
//!
//! 「ここで会戦をやるとおもしろい」場所をスコアリングで探す。評価するのは
//! 展開可能面積・地形の非対称性・ボトルネックの存在・視界の開け具合の 4 つで、
//! これらの合成として上位候補を返す。実際の初期配置決定はシナリオ側
//! （M3 以降）の仕事で、ここでは候補地点とその評価内訳を提供するだけ。

use crate::{Surface, Terrain, SURFACE_EFFECTS};

/// 会戦地候補。
#[derive(Clone, Copy, Debug)]
pub struct BattleSiteCandidate {
    pub x_m: i32,
    pub y_m: i32,
    pub score: i32,
    /// 展開可能面積の比率（1000 分率）
    pub passable_permille: u16,
    /// 地形の非対称性（1000 分率、大きいほど南北で地形差がある）
    pub asymmetry_permille: u16,
    /// 視界の開け具合（1000 分率。0 = 完全に閉じている、1000 = 完全に開けている）
    pub openness_permille: u16,
    /// ボトルネック（渡渉点・橋・浅瀬）のサンプル数
    pub bottleneck_count: u16,
}

const RADIUS_M: i32 = 250;
const CANDIDATE_STRIDE_M: i32 = 300;
const SAMPLE_STEP_M: i32 = 16;
/// 非対称性を 1000 分率に正規化するときの基準（cm）。これ以上の高低差で頭打ち。
const ASYMMETRY_CAP_CM: i32 = 600;
/// 開放度が最も好ましいとみなす値（1000 分率）。開けすぎず閉じすぎず。
const OPENNESS_TARGET: i32 = 650;

/// 会戦地の候補を評価し、スコア降順で上位 `top_n` 件を返す。
///
/// マップが小さすぎて半径ぶんの窓が取れない場合は空を返す。
pub fn evaluate(t: &Terrain, top_n: usize) -> Vec<BattleSiteCandidate> {
    let size_m = t.size_m() as i32;
    let radius = RADIUS_M.min(size_m / 3).max(t.cell_m as i32 * 8);
    if size_m < radius * 2 + CANDIDATE_STRIDE_M {
        return Vec::new();
    }

    let stride = CANDIDATE_STRIDE_M.max(t.cell_m as i32 * 4);
    let step = SAMPLE_STEP_M.max(t.cell_m as i32);

    let mut candidates = Vec::new();

    let mut cy = radius;
    while cy <= size_m - radius {
        let mut cx = radius;
        while cx <= size_m - radius {
            if let Some(c) = evaluate_one(t, cx, cy, radius, step) {
                candidates.push(c);
            }
            cx += stride;
        }
        cy += stride;
    }

    candidates.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(a.x_m.cmp(&b.x_m))
            .then(a.y_m.cmp(&b.y_m))
    });
    candidates.truncate(top_n);
    candidates
}

fn evaluate_one(
    t: &Terrain,
    cx_m: i32,
    cy_m: i32,
    radius_m: i32,
    step_m: i32,
) -> Option<BattleSiteCandidate> {
    let cell_m = t.cell_m as i32;
    let mut total = 0i32;
    let mut passable = 0i32;
    let mut cover_sum: i64 = 0;
    let mut north_height_sum: i64 = 0;
    let mut north_count = 0i32;
    let mut south_height_sum: i64 = 0;
    let mut south_count = 0i32;
    let mut bottleneck = 0i32;

    let mut dy = -radius_m;
    while dy <= radius_m {
        let mut dx = -radius_m;
        while dx <= radius_m {
            if dx * dx + dy * dy > radius_m * radius_m {
                dx += step_m;
                continue;
            }
            let wx = cx_m + dx;
            let wy = cy_m + dy;
            let gx = (wx / cell_m).clamp(0, t.dim as i32 - 1) as u32;
            let gy = (wy / cell_m).clamp(0, t.dim as i32 - 1) as u32;
            let idx = t.idx(gx, gy);

            total += 1;
            let surface = Surface::from_u8(t.surface[idx]);
            let eff = &SURFACE_EFFECTS[surface as usize];
            if t.passability[idx] > 0 {
                passable += 1;
            }
            cover_sum += eff.cover as i64;

            if matches!(
                surface,
                Surface::Ford | Surface::Bridge | Surface::ShallowWater
            ) {
                bottleneck += 1;
            }

            let h = t.height[idx] as i64;
            if dy < 0 {
                north_height_sum += h;
                north_count += 1;
            } else {
                south_height_sum += h;
                south_count += 1;
            }

            dx += step_m;
        }
        dy += step_m;
    }

    if total == 0 || north_count == 0 || south_count == 0 {
        return None;
    }

    let passable_permille = (passable * 1000 / total).clamp(0, 1000) as u16;
    let avg_cover = (cover_sum / total as i64) as i32;
    let openness_permille = (1000 - avg_cover).clamp(0, 1000) as u16;

    let avg_north = north_height_sum / north_count as i64;
    let avg_south = south_height_sum / south_count as i64;
    let height_diff_cm = (avg_north - avg_south).unsigned_abs() as i32;
    let asymmetry_permille =
        ((height_diff_cm.min(ASYMMETRY_CAP_CM)) * 1000 / ASYMMETRY_CAP_CM) as u16;

    let openness_score = 1000 - (openness_permille as i32 - OPENNESS_TARGET).abs();
    let score = passable_permille as i32 * 3
        + openness_score * 2
        + asymmetry_permille as i32
        + (bottleneck.min(20)) * 50;

    Some(BattleSiteCandidate {
        x_m: cx_m,
        y_m: cy_m,
        score,
        passable_permille,
        asymmetry_permille,
        openness_permille,
        bottleneck_count: bottleneck.min(u16::MAX as i32) as u16,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{generate, TerrainParams};

    fn mid_params(seed: u64) -> TerrainParams {
        TerrainParams {
            seed,
            size_m: 2000,
            cell_m: 2,
            relief: 500,
            thermal_iterations: 6,
            ..Default::default()
        }
    }

    #[test]
    fn evaluation_is_deterministic() {
        let t = generate(&mid_params(21));
        let a = evaluate(&t, 5);
        let b = evaluate(&t, 5);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.x_m, y.x_m);
            assert_eq!(x.y_m, y.y_m);
            assert_eq!(x.score, y.score);
        }
    }

    #[test]
    fn candidates_are_sorted_descending() {
        let t = generate(&mid_params(22));
        let sites = evaluate(&t, 10);
        for w in sites.windows(2) {
            assert!(w[0].score >= w[1].score);
        }
    }

    #[test]
    fn candidates_stay_within_the_map() {
        let t = generate(&mid_params(23));
        let size = t.size_m() as i32;
        for s in evaluate(&t, 10) {
            assert!((0..=size).contains(&s.x_m));
            assert!((0..=size).contains(&s.y_m));
        }
    }

    #[test]
    fn tiny_map_yields_no_candidates() {
        let t = generate(&TerrainParams {
            size_m: 200,
            cell_m: 2,
            thermal_iterations: 2,
            ..mid_params(24)
        });
        assert!(evaluate(&t, 5).is_empty());
    }

    #[test]
    fn best_candidate_has_reasonable_passability() {
        let t = generate(&mid_params(25));
        let sites = evaluate(&t, 3);
        assert!(!sites.is_empty());
        assert!(
            sites[0].passable_permille > 300,
            "最上位候補の通行可能率が低すぎる: {}",
            sites[0].passable_permille
        );
    }
}
