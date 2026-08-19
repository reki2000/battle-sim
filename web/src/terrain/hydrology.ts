/**
 * 水系の生成。仕様 `docs/spec/03-terrain.md` 2.3 節（段階 6〜8）。
 *
 * 河川は生成しない（見た目が不自然なため廃止済み）。ここでやるのは
 * 湖の確定だけだが、そのために 2 段階を踏む。
 *
 * 1. **優先度フラッド**（bucket 版）: 境界セルから内側に向かって処理し、
 *    窪地を「埋めた高さ」に引き上げる。同時に各セルの流出先（`flowTo`）を
 *    記録する。処理順そのものがトポロジカル順になる。
 * 2. **湖の分類**: 埋めた窪地（`filledCm` が元の標高より高いセル）を
 *    湖にする。
 *
 * 木構造（各セルは高々 1 つの `flowTo` を持つ）は境界セルを根とする森なので、
 * すべてのセルは必ず境界（地図端）に到達する。この排水の保証は湖の形が
 * 有限に確定するために必要で、河川を作らなくなった後も変わらず要る。
 *
 * 流量集積（`accumulation`）は河川生成をやめた今は使っていないが、
 * 優先度フラッドの副産物として計算コストがほぼゼロなので残してある
 * （呼び出し側の互換性・将来の再利用のため）。
 *
 * 標高は cm の整数で扱う。優先度フラッドは高さをバケット添字にするので、
 * ここを浮動小数にすると計算量が跳ね上がる。
 */

import { WaterKind } from "./types";

/** 流出先を持たない（＝境界に到達した）ことを示す番兵値。 */
export const SINK = -1;

export interface FlowField {
  /** 窪地を埋めた高さ（cm）。湖面の水深計算に使う */
  filledCm: Int32Array;
  /** 各セルの流出先 index。境界に到達済みなら `SINK` */
  flowTo: Int32Array;
  /** 優先度フラッドが処理した順（境界に近い順） */
  visitOrder: Uint32Array;
  /** 各セルの流量集積（降水量 1 単位/セルからの積算） */
  accumulation: Uint32Array;
}

/**
 * 優先度フラッド（bucket 方式）で窪地を埋め、流向・流量集積まで求める。
 *
 * 高さは整数 cm で範囲が小さい（実運用で概ね ±5000 cm）ため、
 * 二分ヒープではなく高さをそのままバケット添字にする方式が
 * O(n + range) で高速かつ実装が単純になる。
 */
export function floodAndFlow(dim: number, heightCm: Int16Array): FlowField {
  const n = dim * dim;

  let minH = heightCm.length > 0 ? heightCm[0]! : 0;
  let maxH = minH;
  for (let i = 1; i < n; i++) {
    const h = heightCm[i]!;
    if (h < minH) minH = h;
    if (h > maxH) maxH = h;
  }
  const range = Math.max(1, maxH - minH + 1);

  const filledCm = new Int32Array(n);
  for (let i = 0; i < n; i++) filledCm[i] = heightCm[i]!;
  const flowTo = new Int32Array(n).fill(SINK);
  const visited = new Uint8Array(n);
  const buckets: number[][] = Array.from({ length: range }, () => []);

  // 境界セルを種にする。地図の端が全体の排水口（＝海または地図外）。
  const seed = (i: number): void => {
    visited[i] = 1;
    buckets[heightCm[i]! - minH]!.push(i);
  };
  for (let x = 0; x < dim; x++) {
    seed(x);
    seed((dim - 1) * dim + x);
  }
  for (let y = 1; y < dim - 1; y++) {
    seed(y * dim);
    seed(y * dim + dim - 1);
  }

  const visitOrder = new Uint32Array(n);
  let orderLen = 0;
  let level = 0;
  let remaining = n;

  while (remaining > 0) {
    while (level < range && buckets[level]!.length === 0) level++;
    // すべてのセルが孤立していない限り起こらない（防御的に打ち切る）
    if (level >= range) break;

    const idx = buckets[level]!.pop()!;
    visitOrder[orderLen++] = idx;
    remaining--;

    const curFill = minH + level;
    const cx = idx % dim;
    const cy = (idx / dim) | 0;

    for (let d = 0; d < 4; d++) {
      const nx = cx + (d === 0 ? 1 : d === 1 ? -1 : 0);
      const ny = cy + (d === 2 ? 1 : d === 3 ? -1 : 0);
      if (nx < 0 || ny < 0 || nx >= dim || ny >= dim) continue;
      const nidx = ny * dim + nx;
      if (visited[nidx]) continue;
      visited[nidx] = 1;
      flowTo[nidx] = idx;

      const nh = heightCm[nidx]!;
      const fillH = Math.max(nh, curFill);
      filledCm[nidx] = fillH;
      buckets[Math.min(range - 1, fillH - minH)]!.push(nidx);
    }
  }

  // 流量集積: 処理順の逆順が上流→下流のトポロジカル順になる
  // （優先度フラッドは境界から遠い＝高いセルほど後で処理するため）。
  const accumulation = new Uint32Array(n).fill(1); // 各セル 1 単位の降水量から開始
  for (let k = orderLen - 1; k >= 0; k--) {
    const idx = visitOrder[k]!;
    const down = flowTo[idx]!;
    if (down !== SINK) {
      accumulation[down] = Math.min(0xffffffff, accumulation[down]! + accumulation[idx]!);
    }
  }

  return { filledCm, flowTo, visitOrder: visitOrder.subarray(0, orderLen), accumulation };
}

export interface WaterClassification {
  /** 水深（cm）。0 なら陸地 */
  depthCm: Uint16Array;
  kind: Uint8Array;
  lakeCells: number;
}

/**
 * 1 つの湖として残す最大セル数。
 *
 * 滑らかな地形では、地図の縁より内陸側が広く低い一つの塊になり、それが
 * そのまま巨大な単一の盆地として境界に接するまで一切分割されない、という
 * ことが実際に起こりうる（優先度フラッドは物理的に正しく動作しているが、
 * 結果としてマップの 3 割が一枚の湖になるのはゲームの地形として困る）。
 * 本物の水力浸食（谷を刻んで出口を作る）を実装していない現状の代替として、
 * 閾値を超える盆地は最も深い部分だけを湖として残し、残りは陸地に戻す。
 */
const MAX_LAKE_CELLS = 6000;

/**
 * 湖として数えない浅さ（cm）。
 *
 * 標高ノイズは 2 m セルの尺度で数十 cm の凹凸を必ず作る。そこに溜まった
 * 数 cm の水を湖として扱うと、平坦なシナリオの地図が水玉模様になり、
 * 展開地の 2 割が騎兵の通れない面になってしまう。窪みが水を湛えるには
 * 相応の深さが要る、として切る。
 */
const LAKE_MIN_DEPTH_CM = 50;

/**
 * 湖として残す最小のセル数。
 *
 * 深さで切ってもなお、数セルの点のような窪みが多数残る。面積の下限を
 * 置くことで、地図に散る「水たまり」を消し、湖と呼べる大きさのものだけを
 * 残す。50 セル = 200 m²（一辺 14 m 程度）。
 */
const MIN_LAKE_CELLS = 50;

/**
 * 埋めた窪地（`filledCm` が元の標高より高いセル）を湖に分類する。
 *
 * 河川は生成しない（不自然な見た目のため廃止済み）。
 */
export function classifyWater(dim: number, heightCm: Int16Array, flow: FlowField): WaterClassification {
  const n = dim * dim;
  const lakeDepthCm = new Int32Array(n);
  for (let i = 0; i < n; i++) {
    const d = flow.filledCm[i]! - heightCm[i]!;
    if (d > LAKE_MIN_DEPTH_CM) lakeDepthCm[i] = d;
  }

  const isTrueLake = new Uint8Array(n);
  const visited = new Uint8Array(n);
  const queue = new Int32Array(n);

  for (let start = 0; start < n; start++) {
    if (visited[start] || lakeDepthCm[start] === 0) continue;
    // この盆地（連結成分）を BFS で集める
    const component: number[] = [];
    let qh = 0;
    let qt = 0;
    visited[start] = 1;
    queue[qt++] = start;
    while (qh < qt) {
      const idx = queue[qh++]!;
      component.push(idx);
      const cx = idx % dim;
      const cy = (idx / dim) | 0;
      for (let d = 0; d < 4; d++) {
        const nx = cx + (d === 0 ? 1 : d === 1 ? -1 : 0);
        const ny = cy + (d === 2 ? 1 : d === 3 ? -1 : 0);
        if (nx < 0 || ny < 0 || nx >= dim || ny >= dim) continue;
        const nidx = ny * dim + nx;
        if (!visited[nidx] && lakeDepthCm[nidx] !== 0) {
          visited[nidx] = 1;
          queue[qt++] = nidx;
        }
      }
    }

    if (component.length < MIN_LAKE_CELLS) continue;

    if (component.length <= MAX_LAKE_CELLS) {
      for (const idx of component) isTrueLake[idx] = 1;
      continue;
    }

    // 大きすぎる盆地は、最も深い MAX_LAKE_CELLS 件だけを湖として残す。
    // 深さの同点はセル index で決定的にタイブレークする。
    component.sort((a, b) => lakeDepthCm[b]! - lakeDepthCm[a]! || a - b);
    for (let k = 0; k < MAX_LAKE_CELLS; k++) isTrueLake[component[k]!] = 1;
  }

  const depthCm = new Uint16Array(n);
  const kind = new Uint8Array(n);
  let lakeCells = 0;

  for (let i = 0; i < n; i++) {
    if (isTrueLake[i]) {
      depthCm[i] = Math.min(65535, lakeDepthCm[i]!);
      kind[i] = WaterKind.LAKE;
      lakeCells++;
    }
  }

  return { depthCm, kind, lakeCells };
}

/** すべてのセルが境界（地図端）まで到達できるか検証する。 */
export function allCellsDrainToBorder(flow: FlowField): boolean {
  const maxSteps = flow.flowTo.length + 1;
  for (let start = 0; start < flow.flowTo.length; start++) {
    let cur = start;
    let steps = 0;
    while (cur !== SINK) {
      cur = flow.flowTo[cur]!;
      steps++;
      if (steps > maxSteps) return false;
    }
  }
  return true;
}
