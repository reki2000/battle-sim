//! 道路の敷設。
//!
//! 仕様 `docs/spec/03-terrain.md` 2.6 節・6 節（段階 12）。
//!
//! フルの地形グリッド（5 km マップで最大 2500² セル）上で直接 Dijkstra を
//! 回すと重いので、粗いグリッド（既定 8 m/セル相当）に落として経路を求め、
//! 結果をラスタライズしてから元のグリッドへ焼き戻す。拠点間は勾配コストに
//! 従うので、平地を通り、川は流量の小さい場所（狭い場所）を選んで渡る
//! 傾向が自然に出る。交点は自動的に橋になる。

use crate::{Surface, Terrain, TerrainParams, SURFACE_EFFECTS};
use sim_math::{Purpose, Rng};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// 粗いグリッド 1 セルあたりの元グリッドのセル数。
const COARSEN: i32 = 4;
/// 道路の幅（片側、fine セル単位）。1 なら中心 ±1 セル = 3 セル幅。
const ROAD_HALF_WIDTH: i32 = 1;

/// 道路を敷設し、`t.surface` に `Road` / `Bridge` を焼き込む。
pub fn lay_roads(t: &mut Terrain, p: &TerrainParams) {
    if p.road_count == 0 {
        return;
    }
    let dim = t.dim as i32;
    if dim < COARSEN * 4 {
        return; // マップが小さすぎて粗視化する意味がない
    }
    let coarse_dim = (dim / COARSEN).max(2);

    let coarse_cost = build_coarse_cost(t, coarse_dim);
    let endpoints = pick_endpoints(t, p, coarse_dim);

    for (a, b) in endpoints {
        if let Some(path) = shortest_path(&coarse_cost, coarse_dim, a, b) {
            rasterize_path(t, &path, coarse_dim);
        }
    }
}

/// 粗いグリッド 1 セルの通行コスト。値が大きいほど通りたくない。
fn build_coarse_cost(t: &Terrain, coarse_dim: i32) -> Vec<u32> {
    let mut cost = vec![0u32; (coarse_dim * coarse_dim) as usize];
    for cy in 0..coarse_dim {
        for cx in 0..coarse_dim {
            let (fx, fy) = coarse_to_fine_center(cx, cy, t.dim as i32);
            let idx = t.idx(fx as u32, fy as u32);
            let surface = Surface::from_u8(t.surface[idx]);
            let eff = &SURFACE_EFFECTS[surface as usize];

            // move_mult が高い（歩きやすい）ほどコストを下げる
            let mut c: i32 = 1200 - eff.move_mult as i32;
            c = c.max(50);

            let depth_cm = t.water[idx] as i32 * 10;
            if depth_cm > 0 {
                // 水域は渡橋コストとして重く見積もる。深いほど橋が長く高くつく
                c += 500 + depth_cm * 3;
            }

            // 粗いセル内の標高差を大まかな勾配コストにする
            let h_here = t.height_at_cell(fx, fy) as i32;
            let h_east = t.height_at_cell(fx + COARSEN, fy) as i32;
            let h_south = t.height_at_cell(fx, fy + COARSEN) as i32;
            let slope = (h_here - h_east).abs().max((h_here - h_south).abs());
            c += slope / 4;

            cost[(cy * coarse_dim + cx) as usize] = c.max(1) as u32;
        }
    }
    cost
}

#[inline]
fn coarse_to_fine_center(cx: i32, cy: i32, fine_dim: i32) -> (i32, i32) {
    let fx = (cx * COARSEN + COARSEN / 2).min(fine_dim - 1);
    let fy = (cy * COARSEN + COARSEN / 2).min(fine_dim - 1);
    (fx, fy)
}

/// 道路の始点・終点ペアを決める。地図の対辺同士を、辺に沿ってずらしながら結ぶ。
fn pick_endpoints(t: &Terrain, p: &TerrainParams, coarse_dim: i32) -> Vec<(i32, i32)> {
    let mut rng = Rng::stream(p.seed, 0, Purpose::Terrain, 0xC0AD);
    let mut pairs = Vec::new();
    let last = coarse_dim - 1;

    for i in 0..p.road_count {
        let jitter_a = rng.range(0, coarse_dim);
        let jitter_b = rng.range(0, coarse_dim);
        let (a, b) = if i % 2 == 0 {
            // 西 → 東
            (jitter_a * coarse_dim, last * coarse_dim + jitter_b)
        } else {
            // 北 → 南
            (jitter_a, last * coarse_dim + jitter_b)
        };
        pairs.push((a, b));
    }

    let _ = t;
    pairs
}

/// 粗いグリッド上の Dijkstra 探索。決定論を保つため、優先度キューのタイブレークに
/// ノード index を使う（コストが同じでも走査順に依らず同じ経路が選ばれる）。
fn shortest_path(cost: &[u32], coarse_dim: i32, start: i32, goal: i32) -> Option<Vec<i32>> {
    let n = (coarse_dim * coarse_dim) as usize;
    if start < 0 || goal < 0 || start as usize >= n || goal as usize >= n {
        return None;
    }

    let mut dist = vec![u32::MAX; n];
    let mut prev = vec![-1i32; n];
    let mut heap: BinaryHeap<Reverse<(u32, i32)>> = BinaryHeap::new();

    dist[start as usize] = 0;
    heap.push(Reverse((0, start)));

    while let Some(Reverse((d, idx))) = heap.pop() {
        if idx == goal {
            break;
        }
        if d > dist[idx as usize] {
            continue;
        }
        let cx = idx % coarse_dim;
        let cy = idx / coarse_dim;
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let nx = cx + dx;
            let ny = cy + dy;
            if nx < 0 || ny < 0 || nx >= coarse_dim || ny >= coarse_dim {
                continue;
            }
            let nidx = ny * coarse_dim + nx;
            let nd = d + cost[nidx as usize];
            if nd < dist[nidx as usize] {
                dist[nidx as usize] = nd;
                prev[nidx as usize] = idx;
                heap.push(Reverse((nd, nidx)));
            }
        }
    }

    if dist[goal as usize] == u32::MAX {
        return None;
    }

    let mut path = vec![goal];
    let mut cur = goal;
    while cur != start {
        cur = prev[cur as usize];
        if cur < 0 {
            return None;
        }
        path.push(cur);
    }
    path.reverse();
    Some(path)
}

/// 粗い経路を fine グリッドへラスタライズし、`Road` / `Bridge` を焼く。
fn rasterize_path(t: &mut Terrain, coarse_path: &[i32], coarse_dim: i32) {
    let fine_dim = t.dim as i32;
    let mut fine_points = Vec::with_capacity(coarse_path.len());
    for &c in coarse_path {
        let cx = c % coarse_dim;
        let cy = c / coarse_dim;
        fine_points.push(coarse_to_fine_center(cx, cy, fine_dim));
    }

    for w in fine_points.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        for (lx, ly) in bresenham(x0, y0, x1, y1) {
            stamp_road(t, lx, ly, fine_dim);
        }
    }
    if let Some(&(x, y)) = fine_points.first() {
        stamp_road(t, x, y, fine_dim);
    }
}

/// 1 点の周囲 `ROAD_HALF_WIDTH` セルを道路（水域なら橋）にする。
fn stamp_road(t: &mut Terrain, cx: i32, cy: i32, fine_dim: i32) {
    for dy in -ROAD_HALF_WIDTH..=ROAD_HALF_WIDTH {
        for dx in -ROAD_HALF_WIDTH..=ROAD_HALF_WIDTH {
            let x = cx + dx;
            let y = cy + dy;
            if x < 0 || y < 0 || x >= fine_dim || y >= fine_dim {
                continue;
            }
            let idx = t.idx(x as u32, y as u32);
            t.surface[idx] = if t.water[idx] > 0 {
                Surface::Bridge as u8
            } else {
                Surface::Road as u8
            };
        }
    }
}

/// 整数 Bresenham 直線。始点・終点を含む。
fn bresenham(x0: i32, y0: i32, x1: i32, y1: i32) -> Vec<(i32, i32)> {
    let mut points = Vec::new();
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let (mut x, mut y) = (x0, y0);

    loop {
        points.push((x, y));
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{generate, TerrainParams};

    fn params(seed: u64, road_count: u16) -> TerrainParams {
        TerrainParams {
            seed,
            size_m: 800,
            cell_m: 2,
            relief: 400,
            thermal_iterations: 5,
            road_count,
            ..Default::default()
        }
    }

    #[test]
    fn roads_are_deterministic() {
        let a = generate(&params(11, 2));
        let b = generate(&params(11, 2));
        assert_eq!(a.hash(), b.hash());
    }

    #[test]
    fn zero_roads_means_no_road_surface() {
        let t = generate(&params(12, 0));
        assert!(!t.surface.contains(&(Surface::Road as u8)));
    }

    #[test]
    fn requested_roads_produce_road_or_bridge_tiles() {
        let t = generate(&params(13, 2));
        let road_tiles = t
            .surface
            .iter()
            .filter(|&&s| s == Surface::Road as u8 || s == Surface::Bridge as u8)
            .count();
        assert!(road_tiles > 0, "道路が 1 タイルも生成されていない");
    }

    #[test]
    fn bresenham_includes_both_endpoints() {
        let pts = bresenham(0, 0, 5, 3);
        assert_eq!(pts[0], (0, 0));
        assert_eq!(*pts.last().unwrap(), (5, 3));
    }

    #[test]
    fn shortest_path_finds_a_route_on_uniform_cost() {
        let dim = 5;
        let cost = vec![10u32; (dim * dim) as usize];
        let path = shortest_path(&cost, dim, 0, dim * dim - 1).expect("経路が見つからない");
        assert_eq!(path.first().copied(), Some(0));
        assert_eq!(path.last().copied(), Some(dim * dim - 1));
    }

    #[test]
    fn shortest_path_avoids_expensive_cells() {
        // 中央の列 (x=3) に、1 か所だけ隙間 (y=0) を残した壁を作る 7x7 グリッド。
        // 中段 (y=3) から出発して反対側へ抜けるには、壁のない y=0 まで
        // 迂回するはずで、壁のセルは一切通らない。
        let dim = 7;
        let mut cost = vec![10u32; (dim * dim) as usize];
        for y in 1..dim {
            cost[(y * dim + 3) as usize] = 1_000_000;
        }
        let start = 3 * dim; // (x=0, y=3)
        let goal = 3 * dim + dim - 1; // (x=6, y=3)
        let path = shortest_path(&cost, dim, start, goal).unwrap();
        let crosses_wall = path.iter().any(|&idx| idx % dim == 3 && idx / dim != 0);
        assert!(!crosses_wall, "壁を突っ切っている: {path:?}");
        // 隙間 (x=3, y=0) は通っているはず
        assert!(path.contains(&3));
    }
}
