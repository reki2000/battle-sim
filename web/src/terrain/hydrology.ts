/**
 * 水系の生成。仕様 `docs/spec/03-terrain.md` 2.3 節（段階 6〜8）。
 *
 * 3 段階からなる。
 *
 * 1. **優先度フラッド**（bucket 版）: 境界セルから内側に向かって処理し、
 *    窪地を「埋めた高さ」に引き上げる。同時に各セルの流出先（`flowTo`）を
 *    記録する。これにより後段の D8 判定（平坦な湖面での方向の曖昧さ）を
 *    避けられる。処理順そのものがトポロジカル順になるため、流量集積も
 *    追加のソートなしに求まる。
 * 2. **流量集積**: 優先度フラッドの処理順を逆にたどるだけで、
 *    上流から下流への合算が O(n) で求まる。
 * 3. **河川・湖の分類**: 流量が閾値を超えたセルを河川に、埋めた窪地を湖にする。
 *
 * 木構造（各セルは高々 1 つの `flowTo` を持つ）は境界セルを根とする森なので、
 * すべてのセルは必ず境界（地図端）に到達する。これが仕様の受け入れ条件
 * 「河川が海または地図外に到達している」を構造的に保証する。
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
  riverCells: number;
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
const LAKE_MIN_DEPTH_CM = 30;

/**
 * 湖として残す最小のセル数。
 *
 * 深さで切ってもなお、1〜2 セルの点のような窪みが多数残る。面積の下限を
 * 置くことで、地図に散る「水たまり」を消し、湖と呼べる大きさのものだけを
 * 残す。8 セル = 32 m²。
 */
const MIN_LAKE_CELLS = 8;

/**
 * 流量集積から河川と湖を分類する。
 *
 * - 湖: `filledCm` が元の標高より高い（＝優先度フラッドが窪地を埋めた）セル。
 * - 河川: 湖でないセルのうち、流量が閾値を超えたもの。水深は流量の
 *   平方根に比例させる（仕様 03 章 2.3 節）。
 */
export function classifyWater(
  dim: number,
  heightCm: Int16Array,
  flow: FlowField,
  riverThreshold: number,
): WaterClassification {
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
  let riverCells = 0;
  let lakeCells = 0;

  for (let i = 0; i < n; i++) {
    if (isTrueLake[i]) {
      depthCm[i] = Math.min(65535, lakeDepthCm[i]!);
      kind[i] = WaterKind.LAKE;
      lakeCells++;
      continue;
    }
    const acc = flow.accumulation[i]!;
    if (riverThreshold > 0 && acc >= riverThreshold) {
      // 深さは流量の平方根に比例
      const d = 20 + Math.floor(Math.sqrt(Math.floor(acc / Math.max(1, riverThreshold)))) * 15;
      depthCm[i] = Math.min(800, d);
      kind[i] = WaterKind.RIVER;
      riverCells++;
    }
  }

  return { depthCm, kind, riverCells, lakeCells };
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
