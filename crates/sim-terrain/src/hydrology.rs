//! 水系の生成。
//!
//! 仕様 `docs/spec/03-terrain.md` 2.3 節（段階 6〜8）。
//!
//! 3 段階からなる。
//!
//! 1. **優先度フラッド**（bucket 版）: 境界セルから内側に向かって処理し、
//!    窪地を「埋めた高さ」に引き上げる。同時に各セルの流出先（`flow_to`）を
//!    記録する。これにより後段の D8 判定（平坦な湖面での方向の曖昧さ）を
//!    避けられる。処理順そのものがトポロジカル順になるため、流量集積も
//!    追加のソートなしに求まる。
//! 2. **流量集積**: 優先度フラッドの処理順を逆にたどるだけで、
//!    上流から下流への合算が O(n) で求まる。
//! 3. **河川化**: 流量が閾値を超えたセルを水域にする。
//!
//! 木構造（各セルは高々 1 つの `flow_to` を持つ）は境界セルを根とする森なので、
//! すべてのセルは必ず境界（地図端）に到達する。これは仕様の受け入れ条件
//! 「河川が海または地図外に到達している」を構造的に保証する。

use crate::Terrain;
use std::collections::VecDeque;

/// 流出先を持たない（＝境界に到達した）ことを示す番兵値。
pub const SINK: i32 = -1;

/// 優先度フラッドの結果。
pub struct FlowField {
    /// 窪地を埋めた高さ（cm）。D8 判定や湖面の水深計算に使う。
    pub filled_cm: Vec<i32>,
    /// 各セルの流出先 index。境界に到達済みなら [`SINK`]。
    pub flow_to: Vec<i32>,
    /// 優先度フラッドが処理した順（境界に近い順）。
    /// これを逆にたどると上流から下流への正しい処理順になる。
    pub visit_order: Vec<u32>,
    /// 各セルの流量集積（相対値。降水量 1 単位/セルからの積算）。
    pub accumulation: Vec<u32>,
}

/// 優先度フラッド（bucket 方式）で窪地を埋め、流向・流量集積まで求める。
///
/// 高さは整数 cm で範囲が小さい（実運用で概ね ±5000 cm）ため、
/// 二分ヒープではなく高さをそのままバケット添字にする方式が
/// O(n + range) で高速かつ実装が単純になる。
///
/// バケットからの取り出しは `Vec::pop`（LIFO）で、押し込み順はラスタ走査に
/// 由来するため、結果は入力に対して完全に決定論的。
pub fn flood_and_flow(t: &Terrain) -> FlowField {
    let dim = t.dim as i32;
    let n = (dim * dim) as usize;

    let min_h = *t.height.iter().min().unwrap_or(&0) as i32;
    let max_h = *t.height.iter().max().unwrap_or(&0) as i32;
    let range = (max_h - min_h + 1).max(1) as usize;

    let mut filled_cm: Vec<i32> = t.height.iter().map(|&h| h as i32).collect();
    let mut flow_to: Vec<i32> = vec![SINK; n];
    let mut visited = vec![false; n];
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); range];

    // 境界セルを種にする。地図の端が全体の排水口（＝海または地図外）。
    // 境界は O(dim) で列挙できるので、O(dim²) の全セル走査はしない。
    let mut seed = |idx: usize| {
        visited[idx] = true;
        let b = (t.height[idx] as i32 - min_h) as usize;
        buckets[b].push(idx as u32);
    };
    for x in 0..dim {
        seed(x as usize);
        seed(((dim - 1) * dim + x) as usize);
    }
    for y in 1..dim - 1 {
        seed((y * dim) as usize);
        seed((y * dim + dim - 1) as usize);
    }

    let mut visit_order = Vec::with_capacity(n);
    let mut level = 0usize;
    let mut remaining = n;

    while remaining > 0 {
        while level < buckets.len() && buckets[level].is_empty() {
            level += 1;
        }
        if level >= buckets.len() {
            // すべてのセルが孤立していない限り起こらない（防御的に打ち切る）
            break;
        }
        let idx = buckets[level].pop().unwrap() as usize;
        visit_order.push(idx as u32);
        remaining -= 1;

        let cur_fill = min_h + level as i32;
        let cx = idx as i32 % dim;
        let cy = idx as i32 / dim;

        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let nx = cx + dx;
            let ny = cy + dy;
            if nx < 0 || ny < 0 || nx >= dim || ny >= dim {
                continue;
            }
            let nidx = (ny * dim + nx) as usize;
            if visited[nidx] {
                continue;
            }
            visited[nidx] = true;
            flow_to[nidx] = idx as i32;

            let nh = t.height[nidx] as i32;
            let fill_h = nh.max(cur_fill);
            filled_cm[nidx] = fill_h;
            let nb = ((fill_h - min_h) as usize).min(buckets.len() - 1);
            buckets[nb].push(nidx as u32);
        }
    }

    // 流量集積: 処理順の逆順が上流→下流のトポロジカル順になる
    // （優先度フラッドは境界から遠い＝高いセルほど後で処理するため）。
    let mut accumulation = vec![1u32; n]; // 各セル 1 単位の降水量から開始
    for &idx in visit_order.iter().rev() {
        let idx = idx as usize;
        let downstream = flow_to[idx];
        if downstream != SINK {
            accumulation[downstream as usize] =
                accumulation[downstream as usize].saturating_add(accumulation[idx]);
        }
    }

    FlowField {
        filled_cm,
        flow_to,
        visit_order,
        accumulation,
    }
}

/// 河川・湖の分類結果。
pub struct RiverClassification {
    /// 水深（cm）。0 なら陸地。
    pub water_depth_cm: Vec<u16>,
    /// 各セルの水域種別。`crate::WaterKind::Sea` はここでは使わない
    /// （海は生成パイプラインの後段、`apply_sea` で上書きする）。
    pub kind: Vec<crate::WaterKind>,
    /// 河川セルの数（デバッグ・テスト用）。
    pub river_cells: u32,
    /// 湖セルの数（`filled_cm > height` かつ [`MAX_LAKE_CELLS`] の上限内）。
    pub lake_cells: u32,
}

/// 1 つの湖として残す最大セル数。
///
/// 滑らかな地形では、地図の縁より内陸側が広く低い一つの塊になり、それが
/// そのまま巨大な単一の盆地として境界に接するまで一切分割されない、という
/// ことが実際に起こりうる（優先度フラッドは物理的に正しく動作しているが、
/// 結果としてマップの 3 割が一枚の湖になる、というのはゲームの地形として
/// 望ましくない）。本物の水力浸食（谷を刻んで出口を作る）を実装していない
/// 現状の代替として、閾値を超える盆地は最も深い部分だけを湖として残し、
/// 残りは陸地に戻す（たいてい低湿地として `classify_surface` が湿地・森に
/// 分類する）。
const MAX_LAKE_CELLS: usize = 6000;

/// 流量集積から河川と湖を分類する。
///
/// - 湖: `filled_cm` が元の標高より高い（＝優先度フラッドが窪地を埋めた）セル。
///   深さは埋めた分そのもの。ただし [`MAX_LAKE_CELLS`] を超える盆地は
///   最も深い部分だけを残す。
/// - 河川: 湖でないセルのうち、流量が閾値を超えたもの。
///   川幅・水深は流量の平方根に比例させる（仕様 03 章 2.3 節）。
pub fn classify_water(t: &Terrain, flow: &FlowField, river_threshold: u32) -> RiverClassification {
    let n = t.height.len();
    let dim = t.dim as i32;
    let mut lake_depth_cm = vec![0i32; n];

    for (i, slot) in lake_depth_cm.iter_mut().enumerate() {
        let d = flow.filled_cm[i] - t.height[i] as i32;
        // 数 cm の誤差は湖として数えない（ノイズ由来の一時セル凹凸を避ける）
        if d > 15 {
            *slot = d;
        }
    }

    let is_lake_raw: Vec<bool> = lake_depth_cm.iter().map(|&d| d > 0).collect();
    let mut is_true_lake = vec![false; n];
    let mut visited = vec![false; n];

    for start in 0..n {
        if visited[start] || !is_lake_raw[start] {
            continue;
        }
        // この盆地（連結成分）を BFS で集める
        let mut component: Vec<u32> = Vec::new();
        let mut queue: VecDeque<u32> = VecDeque::new();
        visited[start] = true;
        queue.push_back(start as u32);
        while let Some(idx) = queue.pop_front() {
            component.push(idx);
            let cx = idx as i32 % dim;
            let cy = idx as i32 / dim;
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let nx = cx + dx;
                let ny = cy + dy;
                if nx < 0 || ny < 0 || nx >= dim || ny >= dim {
                    continue;
                }
                let nidx = (ny * dim + nx) as usize;
                if !visited[nidx] && is_lake_raw[nidx] {
                    visited[nidx] = true;
                    queue.push_back(nidx as u32);
                }
            }
        }

        if component.len() <= MAX_LAKE_CELLS {
            for &idx in &component {
                is_true_lake[idx as usize] = true;
            }
            continue;
        }

        // 大きすぎる盆地は、最も深い MAX_LAKE_CELLS 件だけを湖として残す。
        // 深さの同点はセル index で決定的にタイブレークする。
        component.sort_by(|&a, &b| {
            lake_depth_cm[b as usize]
                .cmp(&lake_depth_cm[a as usize])
                .then(a.cmp(&b))
        });
        for &idx in component.iter().take(MAX_LAKE_CELLS) {
            is_true_lake[idx as usize] = true;
        }
    }

    let mut depth = vec![0u16; n];
    let mut kind = vec![crate::WaterKind::None; n];
    let mut river_cells = 0u32;
    let mut lake_cells = 0u32;

    for i in 0..n {
        if is_true_lake[i] {
            depth[i] = lake_depth_cm[i].min(u16::MAX as i32) as u16;
            kind[i] = crate::WaterKind::Lake;
            lake_cells += 1;
            continue;
        }

        let flow_here = flow.accumulation[i];
        if flow_here >= river_threshold {
            // 深さは流量の平方根に比例（sqrt はテーブルなしで整数実装）
            let d_cm = 20 + isqrt_u32(flow_here / river_threshold.max(1)) * 15;
            depth[i] = d_cm.min(800) as u16;
            kind[i] = crate::WaterKind::River;
            river_cells += 1;
        }
    }

    RiverClassification {
        water_depth_cm: depth,
        kind,
        river_cells,
        lake_cells,
    }
}

/// u32 の整数平方根（Newton 法）。sim-math の isqrt64 は i64 向けなので専用に持つ。
fn isqrt_u32(n: u32) -> u32 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// すべてのセルが境界（地図端）まで到達できるか検証する。
///
/// `flow_to` の連鎖を最大 `dim*2` 回たどって `SINK` に到達しなければ
/// サイクルまたは未到達とみなす。優先度フラッドの木構造上は起こらないはずだが、
/// 実装ミスの検出用に置いておく。
///
/// 上限はセル総数 + 1（木構造なので最長経路でもこれを超えない。地図の一辺に
/// 比例する値では、細長く曲がりくねった排水路の経路長を過小評価してしまう）。
pub fn all_cells_drain_to_border(flow: &FlowField, _dim: u32) -> bool {
    let max_steps = flow.flow_to.len() + 1;
    for start in 0..flow.flow_to.len() {
        let mut cur = start as i32;
        let mut steps = 0;
        while cur != SINK {
            steps += 1;
            if steps > max_steps {
                return false;
            }
            cur = flow.flow_to[cur as usize];
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{generate, TerrainParams};

    fn small(seed: u64, relief: u16) -> Terrain {
        generate(&TerrainParams {
            seed,
            size_m: 400,
            cell_m: 2,
            relief,
            thermal_iterations: 5,
            ..Default::default()
        })
    }

    #[test]
    fn every_cell_drains_to_the_border() {
        let t = small(1, 700);
        let flow = flood_and_flow(&t);
        assert!(all_cells_drain_to_border(&flow, t.dim));
    }

    #[test]
    fn border_cells_are_their_own_sink() {
        let t = small(2, 500);
        let flow = flood_and_flow(&t);
        let dim = t.dim as i32;
        for x in 0..dim {
            assert_eq!(flow.flow_to[x as usize], SINK);
            assert_eq!(flow.flow_to[((dim - 1) * dim + x) as usize], SINK);
        }
    }

    #[test]
    fn filled_height_never_decreases_original() {
        let t = small(3, 800);
        let flow = flood_and_flow(&t);
        for i in 0..t.height.len() {
            assert!(flow.filled_cm[i] >= t.height[i] as i32);
        }
    }

    #[test]
    fn accumulation_is_monotonic_along_flow_paths() {
        // 下流のセルの流量は、少なくとも自分自身の 1 単位以上になる
        let t = small(4, 600);
        let flow = flood_and_flow(&t);
        for i in 0..t.height.len() {
            assert!(flow.accumulation[i] >= 1);
            if flow.flow_to[i] != SINK {
                let downstream = flow.flow_to[i] as usize;
                assert!(
                    flow.accumulation[downstream] >= flow.accumulation[i],
                    "下流の流量が上流を下回っている: i={i}"
                );
            }
        }
    }

    #[test]
    fn total_accumulation_at_sinks_covers_the_grid() {
        // すべてのセルの降水（1 単位）は、最終的にどこかの境界セルに集まる。
        // 境界セルの流量合計は少なくともセル総数以上になる
        // （境界セル自身の 1 単位 + 内部から流れ込む分）。
        let t = small(5, 400);
        let flow = flood_and_flow(&t);
        let dim = t.dim as i32;
        let mut border_sum: u64 = 0;
        for y in 0..dim {
            for x in 0..dim {
                if x == 0 || y == 0 || x == dim - 1 || y == dim - 1 {
                    border_sum += flow.accumulation[(y * dim + x) as usize] as u64;
                }
            }
        }
        assert!(border_sum >= t.height.len() as u64);
    }

    #[test]
    fn river_classification_is_deterministic() {
        let t = small(6, 700);
        let flow = flood_and_flow(&t);
        let a = classify_water(&t, &flow, 200);
        let b = classify_water(&t, &flow, 200);
        assert_eq!(a.water_depth_cm, b.water_depth_cm);
    }

    #[test]
    fn lower_threshold_yields_more_or_equal_river_cells() {
        let t = small(7, 700);
        let flow = flood_and_flow(&t);
        let strict = classify_water(&t, &flow, 500);
        let loose = classify_water(&t, &flow, 50);
        assert!(loose.river_cells >= strict.river_cells);
    }

    #[test]
    fn isqrt_matches_known_values() {
        assert_eq!(isqrt_u32(0), 0);
        assert_eq!(isqrt_u32(1), 1);
        assert_eq!(isqrt_u32(4), 2);
        assert_eq!(isqrt_u32(15), 3);
        assert_eq!(isqrt_u32(16), 4);
        assert_eq!(isqrt_u32(1_000_000), 1000);
    }
}
