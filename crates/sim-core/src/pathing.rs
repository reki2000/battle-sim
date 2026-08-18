//! Unit 単位の粗い経路探索（M2）。
//!
//! 地形の `passability` を間引いた粗いグリッド上で A* を実行し、部隊の目的地までの
//! ウェイポイント列を返す。粗いグリッドが「局所フローフィールド」の役割を兼ねる：
//! 各部隊は経路上の次のウェイポイントだけを目指せばよく、障害物の回避は
//! 経路そのものにあらかじめ織り込まれている。
//!
//! 決定論を保つため優先度キューのタイブレークにセル index を使う
//! （`sim-terrain::roads::shortest_path` と同じ手法）。

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use sim_math::{fx, Fx, Vec2Fx, FX_SHIFT};
use sim_terrain::Terrain;

/// Fx をメートル単位の整数に変換する（切り捨て）。
#[inline]
fn fx_to_m(v: Fx) -> i32 {
    v >> FX_SHIFT
}

/// 粗いセルの一辺（m）。地形の `cell_m` より大きく、隘路を見失わない程度に小さい。
pub const COARSE_CELL_M: i32 = 16;

/// 探索が長くなりすぎたときに打ち切るノード数の上限。
const MAX_EXPANSIONS: usize = 200_000;

/// 経路の到達判定に使う許容半径。
pub fn arrival_radius() -> Fx {
    fx(COARSE_CELL_M / 2)
}

fn coarse_dim(terrain: &Terrain) -> i32 {
    terrain.size_m().div_ceil(COARSE_CELL_M as u32).max(1) as i32
}

fn to_coarse(terrain: &Terrain, p: Vec2Fx) -> (i32, i32) {
    let dim = coarse_dim(terrain);
    let limit = terrain.size_m() as i32 - 1;
    let x = fx_to_m(p.x).clamp(0, limit) / COARSE_CELL_M;
    let y = fx_to_m(p.y).clamp(0, limit) / COARSE_CELL_M;
    (x.clamp(0, dim - 1), y.clamp(0, dim - 1))
}

fn coarse_center(cx: i32, cy: i32) -> Vec2Fx {
    Vec2Fx::new(
        fx(cx * COARSE_CELL_M + COARSE_CELL_M / 2),
        fx(cy * COARSE_CELL_M + COARSE_CELL_M / 2),
    )
}

/// 粗いセルを通過するコスト。通行不能なら `None`。
///
/// 中心 1 点だけのサンプルだと、粗いセルよりずっと狭い隘路や、逆に細い障害物を
/// 取りこぼす／過大評価することがあるため、セル内を 3×3 の点でサンプルし、
/// 過半数（5 点以上）が通行可能なら「通れる」とみなす多数決にする。
/// コストは通行可能な点の中で最も速い地表を採用する
/// （粗いグリッドの中でどこを通るかは fine な移動・衝突フェーズが決める）。
fn cell_cost(terrain: &Terrain, cx: i32, cy: i32) -> Option<u32> {
    let base_x = cx * COARSE_CELL_M;
    let base_y = cy * COARSE_CELL_M;
    let mut passable = 0u32;
    let mut best_mult = 0u32;
    for sx in [1, COARSE_CELL_M / 2, COARSE_CELL_M - 2] {
        for sy in [1, COARSE_CELL_M / 2, COARSE_CELL_M - 2] {
            let world = Vec2Fx::new(fx(base_x + sx), fx(base_y + sy));
            let (fcx, fcy) = terrain.world_to_cell(world.x, world.y);
            let idx = terrain.idx(fcx, fcy);
            if terrain.passability[idx] == 0 {
                continue;
            }
            passable += 1;
            let mult = terrain.effect_at_index(idx).move_mult as u32;
            best_mult = best_mult.max(mult);
        }
    }
    if passable < 5 {
        return None;
    }
    Some((1_000_000 / best_mult.max(1)).max(50))
}

/// 経路探索の許容判定に使う、地表として理論上ありうる最小コスト
/// （ヒューリスティックの admissible 性を保つための下限）。
const MIN_STEP_COST: u32 = 700;
const DIAGONAL_FACTOR: u32 = 1414; // sqrt(2) * 1000

fn heuristic(a: (i32, i32), b: (i32, i32)) -> u32 {
    let dx = (a.0 - b.0).unsigned_abs();
    let dy = (a.1 - b.1).unsigned_abs();
    let (small, large) = if dx < dy { (dx, dy) } else { (dy, dx) };
    // octile distance（対角移動を考慮した下界）
    (small * DIAGONAL_FACTOR + (large - small) * 1000) / 1000 * MIN_STEP_COST / 1000
}

const NEIGHBORS: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

/// `from` から `to` までの粗い経路をワールド座標のウェイポイント列で返す。
/// 到達不能、あるいは探索が上限に達した場合は `None`。
pub fn find_path(terrain: &Terrain, from: Vec2Fx, to: Vec2Fx) -> Option<Vec<Vec2Fx>> {
    let dim = coarse_dim(terrain);
    let n = (dim * dim) as usize;
    let start = to_coarse(terrain, from);
    let goal = to_coarse(terrain, to);
    let start_idx = (start.1 * dim + start.0) as usize;
    let goal_idx = (goal.1 * dim + goal.0) as usize;

    if start_idx == goal_idx {
        return Some(vec![to]);
    }
    // 出発点・目的地そのものが通行不能セルにあっても探索は続ける
    // （兵士は密集や崖際にいることがあり、直近セルから抜けられる必要がある）。

    let mut g = vec![u32::MAX; n];
    let mut prev = vec![-1i32; n];
    let mut heap: BinaryHeap<Reverse<(u32, i32)>> = BinaryHeap::new();
    g[start_idx] = 0;
    heap.push(Reverse((heuristic(start, goal), start_idx as i32)));

    let mut expansions = 0usize;
    while let Some(Reverse((_, idx))) = heap.pop() {
        if idx as usize == goal_idx {
            break;
        }
        expansions += 1;
        if expansions > MAX_EXPANSIONS {
            return None;
        }
        let d = g[idx as usize];
        let cx = idx % dim;
        let cy = idx / dim;
        for &(dx, dy) in &NEIGHBORS {
            let nx = cx + dx;
            let ny = cy + dy;
            if nx < 0 || ny < 0 || nx >= dim || ny >= dim {
                continue;
            }
            // 対角移動は角を斜めに切り抜けさせない（両側の直交セルも通行可能なこと）
            if dx != 0
                && dy != 0
                && (cell_cost(terrain, cx + dx, cy).is_none()
                    || cell_cost(terrain, cx, cy + dy).is_none())
            {
                continue;
            }
            let Some(cost) = cell_cost(terrain, nx, ny) else {
                continue;
            };
            let step = if dx != 0 && dy != 0 {
                cost * DIAGONAL_FACTOR / 1000
            } else {
                cost
            };
            let nidx = (ny * dim + nx) as usize;
            let nd = d.saturating_add(step);
            if nd < g[nidx] {
                g[nidx] = nd;
                prev[nidx] = idx;
                let f = nd.saturating_add(heuristic((nx, ny), goal));
                heap.push(Reverse((f, nidx as i32)));
            }
        }
    }

    if g[goal_idx] == u32::MAX {
        return None;
    }

    let mut cells = vec![goal_idx as i32];
    let mut cur = goal_idx as i32;
    while cur != start_idx as i32 {
        cur = prev[cur as usize];
        if cur < 0 {
            return None;
        }
        cells.push(cur);
    }
    cells.reverse();

    Some(simplify(&cells, dim, to))
}

/// 直線でつながる区間を間引き、最後のウェイポイントは正確な目的地に差し替える。
fn simplify(cells: &[i32], dim: i32, exact_goal: Vec2Fx) -> Vec<Vec2Fx> {
    let mut points: Vec<(i32, i32)> = cells.iter().map(|&c| (c % dim, c / dim)).collect();
    let mut waypoints = Vec::new();
    let mut i = 0;
    while i < points.len() {
        let start = points[i];
        let mut j = i + 1;
        if j < points.len() {
            let dir = (
                (points[j].0 - start.0).signum(),
                (points[j].1 - start.1).signum(),
            );
            while j + 1 < points.len() {
                let next_dir = (
                    (points[j + 1].0 - points[j].0).signum(),
                    (points[j + 1].1 - points[j].1).signum(),
                );
                if next_dir != dir {
                    break;
                }
                j += 1;
            }
        }
        waypoints.push(coarse_center(
            points[j.min(points.len() - 1)].0,
            points[j.min(points.len() - 1)].1,
        ));
        i = j + 1;
        if i >= points.len() {
            break;
        }
    }
    points.clear();
    if let Some(last) = waypoints.last_mut() {
        *last = exact_goal;
    } else {
        waypoints.push(exact_goal);
    }
    waypoints
}

#[cfg(test)]
mod tests {
    use super::*;
    fn flat_terrain(size_m: u32) -> Terrain {
        Terrain::flat(size_m / 4, 4, 42)
    }

    #[test]
    fn straight_line_on_open_ground_reaches_the_goal() {
        let t = flat_terrain(400);
        let path = find_path(
            &t,
            Vec2Fx::new(fx(20), fx(20)),
            Vec2Fx::new(fx(300), fx(300)),
        )
        .expect("経路が見つかるはず");
        let last = *path.last().unwrap();
        assert!(sim_math::dist(last, Vec2Fx::new(fx(300), fx(300))) < fx(1));
    }

    #[test]
    fn path_is_deterministic() {
        let t = flat_terrain(500);
        let a = find_path(
            &t,
            Vec2Fx::new(fx(10), fx(10)),
            Vec2Fx::new(fx(450), fx(400)),
        );
        let b = find_path(
            &t,
            Vec2Fx::new(fx(10), fx(10)),
            Vec2Fx::new(fx(450), fx(400)),
        );
        assert_eq!(a, b);
    }

    #[test]
    fn same_cell_returns_direct_goal() {
        let t = flat_terrain(200);
        let path = find_path(&t, Vec2Fx::new(fx(10), fx(10)), Vec2Fx::new(fx(12), fx(11))).unwrap();
        assert_eq!(path, vec![Vec2Fx::new(fx(12), fx(11))]);
    }

    #[test]
    fn detours_around_a_wall_that_blocks_a_whole_coarse_cell() {
        // 粗いセルは 16 m 四方なので、壁はその幅を丸ごと塞ぐ厚みが要る
        // （1 fine セルだけの薄い壁は 3x3 サンプルのどれかが素通りしてしまう）。
        let mut t = flat_terrain(400);
        let gap_start = t.dim.saturating_sub(12);
        for cy in 0..t.dim {
            if cy >= gap_start {
                continue;
            }
            for wx in (t.dim / 2)..(t.dim / 2 + 8) {
                let idx = t.idx(wx, cy);
                t.passability[idx] = 0;
            }
        }
        let start = Vec2Fx::new(fx(50), fx(200));
        let goal = Vec2Fx::new(fx(350), fx(200));
        let path = find_path(&t, start, goal).expect("隙間があるので経路が見つかるはず");
        assert!(path.len() > 1, "壁があるのに経路が直線 1 本になっている");
    }
}
