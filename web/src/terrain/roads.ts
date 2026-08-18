/**
 * 道路の敷設。仕様 `docs/spec/03-terrain.md` 2 節（段階 12）。
 *
 * フルの地形グリッド（5 km マップで最大 2500² セル）上で直接 Dijkstra を
 * 回すと重いので、粗いグリッド（既定 8 m/セル相当）に落として経路を求め、
 * 結果をラスタライズしてから元のグリッドへ焼き戻す。拠点間は勾配コストに
 * 従うので、平地を通り、川は狭い場所を選んで渡る傾向が自然に出る。
 * 水域との交点は自動的に橋になる。
 *
 * 道路は `overlay` グリッドに書く。地質と植生には触らない——路面の下の
 * 土は土のままで、工兵が道を掘り返せば元の地質が戻る。
 */

import { basePassability } from "./effects";
import { Overlay } from "./types";
import type { GenerationResult } from "./generate";
import { Domain, Stream, derive32 } from "./rng";

/** 粗いグリッド 1 セルあたりの元グリッドのセル数。 */
const COARSEN = 4;
/** 道路の幅（片側、fine セル単位）。1 なら中心 ±1 セル = 3 セル幅。 */
const ROAD_HALF_WIDTH = 1;

/** 道路を敷設し、`overlay` に `ROAD` / `BRIDGE` を焼き込む。 */
export function layRoads(t: GenerationResult, roadCount: number): void {
  if (roadCount <= 0) return;
  const dim = t.dim;
  if (dim < COARSEN * 4) return; // マップが小さすぎて粗視化する意味がない
  const coarseDim = Math.max(2, Math.floor(dim / COARSEN));

  const cost = buildCoarseCost(t, coarseDim);
  const rng = new Stream(derive32(t.seed, Domain.ROADS));
  const last = coarseDim - 1;

  for (let i = 0; i < roadCount; i++) {
    const jitterA = rng.range(0, coarseDim);
    const jitterB = rng.range(0, coarseDim);
    // 偶数本目は西→東、奇数本目は北→南。地図の対辺同士を結ぶ
    const a = i % 2 === 0 ? jitterA * coarseDim : jitterA;
    const b = last * coarseDim + jitterB;
    const path = shortestPath(cost, coarseDim, a, b);
    if (path) rasterizePath(t, path, coarseDim);
  }
}

/** 粗いグリッド 1 セルの通行コスト。値が大きいほど通りたくない。 */
function buildCoarseCost(t: GenerationResult, coarseDim: number): Uint32Array {
  const cost = new Uint32Array(coarseDim * coarseDim);
  const fineDim = t.dim;
  for (let cy = 0; cy < coarseDim; cy++) {
    for (let cx = 0; cx < coarseDim; cx++) {
      const [fx, fy] = coarseToFineCenter(cx, cy, fineDim);
      const i = fy * fineDim + fx;
      const depth = t.water[i]!;

      // 歩きやすいほどコストを下げる。通行不能セルは事実上の壁
      const pass = basePassability(t.ground[i]!, t.vegetation[i]!, depth, Overlay.NONE);
      let c = pass === 0 ? 100_000 : Math.max(50, 1200 - Math.floor((pass * 1150) / 255));

      if (depth > 0) {
        // 水域は渡橋コストとして重く見積もる。深いほど橋が長く高くつく
        c += 500 + depth * 3;
      }

      // 粗いセル内の標高差を大まかな勾配コストにする
      const hHere = t.height[i]!;
      const hEast = t.height[fy * fineDim + Math.min(fineDim - 1, fx + COARSEN)]!;
      const hSouth = t.height[Math.min(fineDim - 1, fy + COARSEN) * fineDim + fx]!;
      c += Math.floor(Math.max(Math.abs(hHere - hEast), Math.abs(hHere - hSouth)) / 4);

      cost[cy * coarseDim + cx] = Math.max(1, c);
    }
  }
  return cost;
}

function coarseToFineCenter(cx: number, cy: number, fineDim: number): [number, number] {
  return [
    Math.min(fineDim - 1, cx * COARSEN + (COARSEN >> 1)),
    Math.min(fineDim - 1, cy * COARSEN + (COARSEN >> 1)),
  ];
}

/**
 * 粗いグリッド上の Dijkstra 探索。
 *
 * 優先度キューのタイブレークにノード index を使うので、コストが同じでも
 * 走査順に依らず同じ経路が選ばれる。
 */
export function shortestPath(
  cost: Uint32Array,
  coarseDim: number,
  start: number,
  goal: number,
): number[] | null {
  const n = coarseDim * coarseDim;
  if (start < 0 || goal < 0 || start >= n || goal >= n) return null;

  const dist = new Uint32Array(n).fill(0xffffffff);
  const prev = new Int32Array(n).fill(-1);
  const heap = new MinHeap();

  dist[start] = 0;
  heap.push(0, start);

  while (heap.size > 0) {
    const [d, idx] = heap.pop()!;
    if (idx === goal) break;
    if (d > dist[idx]!) continue;
    const cx = idx % coarseDim;
    const cy = (idx / coarseDim) | 0;
    for (let k = 0; k < 4; k++) {
      const nx = cx + (k === 0 ? 1 : k === 1 ? -1 : 0);
      const ny = cy + (k === 2 ? 1 : k === 3 ? -1 : 0);
      if (nx < 0 || ny < 0 || nx >= coarseDim || ny >= coarseDim) continue;
      const nidx = ny * coarseDim + nx;
      const nd = d + cost[nidx]!;
      if (nd < dist[nidx]!) {
        dist[nidx] = nd;
        prev[nidx] = idx;
        heap.push(nd, nidx);
      }
    }
  }

  if (dist[goal] === 0xffffffff) return null;
  const path = [goal];
  let cur = goal;
  while (cur !== start) {
    cur = prev[cur]!;
    if (cur < 0) return null;
    path.push(cur);
  }
  path.reverse();
  return path;
}

/** コストとノード index の 2 段キーで並ぶ最小ヒープ。 */
class MinHeap {
  private readonly keys: number[] = [];
  private readonly vals: number[] = [];

  get size(): number {
    return this.keys.length;
  }

  private less(i: number, j: number): boolean {
    return this.keys[i]! < this.keys[j]! || (this.keys[i] === this.keys[j] && this.vals[i]! < this.vals[j]!);
  }

  private swap(i: number, j: number): void {
    [this.keys[i], this.keys[j]] = [this.keys[j]!, this.keys[i]!];
    [this.vals[i], this.vals[j]] = [this.vals[j]!, this.vals[i]!];
  }

  push(key: number, val: number): void {
    this.keys.push(key);
    this.vals.push(val);
    let i = this.keys.length - 1;
    while (i > 0) {
      const parent = (i - 1) >> 1;
      if (!this.less(i, parent)) break;
      this.swap(i, parent);
      i = parent;
    }
  }

  pop(): [number, number] | null {
    if (this.keys.length === 0) return null;
    const top: [number, number] = [this.keys[0]!, this.vals[0]!];
    const lastKey = this.keys.pop()!;
    const lastVal = this.vals.pop()!;
    if (this.keys.length > 0) {
      this.keys[0] = lastKey;
      this.vals[0] = lastVal;
      let i = 0;
      for (;;) {
        const l = i * 2 + 1;
        const r = l + 1;
        let best = i;
        if (l < this.keys.length && this.less(l, best)) best = l;
        if (r < this.keys.length && this.less(r, best)) best = r;
        if (best === i) break;
        this.swap(i, best);
        i = best;
      }
    }
    return top;
  }
}

/** 粗い経路を fine グリッドへラスタライズし、道路・橋を焼く。 */
function rasterizePath(t: GenerationResult, coarsePath: number[], coarseDim: number): void {
  const fineDim = t.dim;
  const points = coarsePath.map((c) =>
    coarseToFineCenter(c % coarseDim, (c / coarseDim) | 0, fineDim),
  );
  for (let i = 0; i + 1 < points.length; i++) {
    for (const [lx, ly] of bresenham(points[i]![0], points[i]![1], points[i + 1]![0], points[i + 1]![1])) {
      stampRoad(t, lx, ly);
    }
  }
  if (points.length > 0) stampRoad(t, points[0]![0], points[0]![1]);
}

/** 1 点の周囲 `ROAD_HALF_WIDTH` セルを道路（水域なら橋）にする。 */
function stampRoad(t: GenerationResult, cx: number, cy: number): void {
  for (let dy = -ROAD_HALF_WIDTH; dy <= ROAD_HALF_WIDTH; dy++) {
    for (let dx = -ROAD_HALF_WIDTH; dx <= ROAD_HALF_WIDTH; dx++) {
      const x = cx + dx;
      const y = cy + dy;
      if (x < 0 || y < 0 || x >= t.dim || y >= t.dim) continue;
      const i = y * t.dim + x;
      t.overlay[i] = t.water[i]! > 0 ? Overlay.BRIDGE : Overlay.ROAD;
    }
  }
}

/** 整数 Bresenham 直線。始点・終点を含む。 */
export function bresenham(x0: number, y0: number, x1: number, y1: number): Array<[number, number]> {
  const points: Array<[number, number]> = [];
  const dx = Math.abs(x1 - x0);
  const dy = -Math.abs(y1 - y0);
  const sx = x0 < x1 ? 1 : -1;
  const sy = y0 < y1 ? 1 : -1;
  let err = dx + dy;
  let x = x0;
  let y = y0;
  for (;;) {
    points.push([x, y]);
    if (x === x1 && y === y1) break;
    const e2 = 2 * err;
    if (e2 >= dy) {
      err += dy;
      x += sx;
    }
    if (e2 <= dx) {
      err += dx;
      y += sy;
    }
  }
  return points;
}
