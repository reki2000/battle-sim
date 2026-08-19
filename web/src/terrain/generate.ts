/**
 * 地形生成の本体。仕様 `docs/spec/03-terrain.md` 2 節。
 *
 * ```
 * Seed + TerrainParams
 *   ├─ 1. ベース標高      多スケールの value noise（波長 700/320/120/48/18 m）
 *   ├─ 2. 露岩            短い波長の稜線ノイズを鋭く立ち上げて局所的な崖を作る
 *   ├─ 3. 点標高制約      Wendland 核による残差投影
 *   ├─ 4. 海岸ランプ      指定した辺へ向けて標高を海面下まで落とす
 *   ├─ 5. 熱浸食          安息角を超える斜面を崩す
 *   ├─ 6. 窪地の処理      優先度フラッドで湖を確定、残りは排水
 *   ├─ 7. 流向          D8 フロー方向（境界までの排水を保証する）
 *   ├─ 8. 海              海面より低い海岸帯のセルを海にする
 *   ├─ 9. 湿度            湖・海からの距離とノイズから湿度場を作る
 *   ├─10. 地質・植生      標高・傾斜・湿度から 2 軸を独立に割り当てる
 *   └─11. 崖              隣接セル間の高度差が閾値超のエッジをマークする
 * ```
 *
 * 生成は f64 で行う。決定論はここでは保証されず、**出力の整数グリッドを
 * シード付きで固定して配布する**ことで担保する（`serialize.ts`）。
 * シミュレーション本体（Rust）は整数演算だけで動き、生成器には依存しない。
 */

import {
  CLIFF_THRESHOLD_CM,
  CliffBits,
  Ground,
  Vegetation,
  WaterKind,
  type SeaEdge,
  type TerrainGrids,
  type TerrainParams,
} from "./types";
import { Domain, NoiseField, clamp, derive32, parseSeed, wendland } from "./rng";
import { classifyWater, floodAndFlow } from "./hydrology";

/** 湿度に効く水域までの最大距離（セル）。これを超えると加点しない。 */
const MOISTURE_RANGE_CELLS = 24;

/** 標高の格納範囲（cm）。i16 に収まる必要がある。 */
const HEIGHT_MIN_CM = -32768;
const HEIGHT_MAX_CM = 32767;

/** 生成の中間結果。道路・整形・会戦地評価が続けて読む。 */
export interface GenerationResult extends Omit<TerrainGrids, "battleSites"> {
  /** 隣接 4 セルとの最大高低差（cm）。通行コストと地表判定が使う */
  slopeCm: Uint16Array;
  /** 流量集積（デバッグ用。河川は生成しないため描画では使っていない） */
  flow: Uint32Array;
}

export const DEFAULT_PARAMS: TerrainParams = {
  seed: 0x5eed_1234_abcd_0001n,
  sizeM: 5000,
  cellM: 2,
  relief: 450,
  baseHeightM: 8,
  thermalIterations: 6,
  roadCount: 2,
  seaEdge: "none",
  seaLevelCm: 0,
  forestCover: 350,
  marshBias: 150,
  sandBias: 200,
};

export function generateFields(paramsIn: Partial<TerrainParams>): GenerationResult {
  const p: TerrainParams = { ...DEFAULT_PARAMS, ...paramsIn };
  const cellM = Math.max(1, Math.floor(p.cellM));
  const dim = Math.max(8, Math.floor(p.sizeM / cellM));
  const n = dim * dim;
  const seed = parseSeed(p.seed);
  const extentM = dim * cellM;

  const height = baseElevation(dim, cellM, extentM, seed, p);
  applyConstraints(height, dim, cellM, p);
  applySeaRamp(height, dim, p);
  thermalErosion(height, dim, cellM, p.thermalIterations);

  const flowField = floodAndFlow(dim, height);
  const water = classifyWater(dim, height, flowField);
  const depth = water.depthCm;
  const kind = water.kind;
  applySea(height, depth, kind, dim, p);

  const distToWater = distanceToWater(dim, depth, MOISTURE_RANGE_CELLS);
  const slopeCm = computeSlope(height, dim);
  const moisture = computeMoisture(dim, cellM, extentM, seed, distToWater, slopeCm, p);

  const { ground, vegetation } = classifyCover(
    dim,
    cellM,
    extentM,
    seed,
    height,
    depth,
    kind,
    slopeCm,
    moisture,
    distToWater,
    p,
  );
  smoothForests(vegetation, ground, depth, dim);
  applyBeaches(ground, vegetation, kind, dim, p.sandBias);
  applyScree(ground, vegetation, height, depth, dim);

  return {
    dim,
    cellM,
    seed,
    height,
    ground,
    vegetation,
    overlay: new Uint8Array(n),
    water: depth,
    waterKind: kind,
    moisture,
    cliff: computeCliffs(height, dim),
    slopeCm,
    flow: flowField.accumulation,
  };
}

/**
 * 段階 1〜2: ベース標高と露岩。
 *
 * 波長は m 単位で決めるので、マップの大きさを変えても丘の大きさは変わらない。
 * 重みは長い波長ほど大きく、合計が 1 になるよう配分してある。
 */
function baseElevation(
  dim: number,
  cellM: number,
  extentM: number,
  seed: bigint,
  p: TerrainParams,
): Int16Array {
  const hs = derive32(seed, Domain.HEIGHT);
  const fields: Array<[NoiseField, number]> = [
    [new NoiseField(hs, extentM, extentM, 700, 0), 0.22],
    [new NoiseField(hs, extentM, extentM, 320, 1), 0.42],
    [new NoiseField(hs, extentM, extentM, 120, 2), 0.2],
    [new NoiseField(hs, extentM, extentM, 48, 3), 0.1],
    [new NoiseField(hs, extentM, extentM, 18, 4), 0.06],
  ];
  // 起伏の強さ（m）。relief 0 → 5 m（平原）、relief 1000 → 125 m（丘陵〜山地）
  const reliefM = 5 + (p.relief * 120) / 1000;
  const outcrop = new NoiseField(derive32(seed, Domain.HEIGHT, 9), extentM, extentM, 120, 5);
  // 露岩の高さ: relief 0 → 4 m、relief 1000 → 14 m
  const outcropM = 4 + (p.relief * 10) / 1000;

  const height = new Int16Array(dim * dim);
  for (let z = 0; z < dim; z++) {
    const zm = (z + 0.5) * cellM;
    for (let x = 0; x < dim; x++) {
      const xm = (x + 0.5) * cellM;
      let f = 0;
      for (const [field, weight] of fields) f += field.sample(xm, zm) * weight;
      const cm = (p.baseHeightM + f * reliefM) * 100 + outcropBumpCm(outcrop, xm, zm, outcropM);
      height[z * dim + x] = clampHeight(Math.round(cm));
    }
  }
  return height;
}

/**
 * 局所的な岩場・小さな崖を作る高周波レイヤー。
 *
 * ベース地形は最短でも波長 18 m と滑らかで、隣接セル間の高低差が
 * 崖の閾値（2.5 m）を超えることがほとんどない。これでは山岳地形でしか
 * 崖が現れず、丘陵程度の起伏では崖という戦術要素が一切出ない。
 * 稜線ノイズ（`1 - |noise|`）の上位だけを鋭く立ち上げることで、
 * 平原〜丘陵にも岩場の縁として局所的な崖を点在させる。
 */
function outcropBumpCm(field: NoiseField, xm: number, zm: number, amplitudeM: number): number {
  const ridge = 1 - Math.abs(field.sample(xm, zm));
  const threshold = 0.65;
  if (ridge <= threshold) return 0;
  return ((ridge - threshold) / (1 - threshold)) * amplitudeM * 100;
}

function clampHeight(cm: number): number {
  return Math.max(HEIGHT_MIN_CM, Math.min(HEIGHT_MAX_CM, cm));
}

/**
 * 段階 3: 点標高制約。
 *
 * 残差の投影を 6 回繰り返すことで、指定地点に最も近いセルが要求標高へ
 * 収束しつつ、補正は `radiusM` の縁で滑らかに 0 へ消える。
 */
function applyConstraints(
  height: Int16Array,
  dim: number,
  cellM: number,
  p: TerrainParams,
): void {
  const constraints = p.constraints ?? [];
  if (constraints.length === 0) return;

  for (let pass = 0; pass < 6; pass++) {
    for (const c of constraints) {
      const cx = clamp(Math.floor(c.xM / cellM), 0, dim - 1);
      const cz = clamp(Math.floor(c.zM / cellM), 0, dim - 1);
      const target = Math.round(c.heightM * 100);
      const residual = target - height[cz * dim + cx]!;
      if (Math.abs(residual) <= 1) continue;

      const radius = Math.max(cellM, c.radiusM);
      const cells = Math.ceil(radius / cellM);
      const minZ = Math.max(0, cz - cells);
      const maxZ = Math.min(dim - 1, cz + cells);
      const minX = Math.max(0, cx - cells);
      const maxX = Math.min(dim - 1, cx + cells);
      for (let z = minZ; z <= maxZ; z++) {
        const dz = (z + 0.5) * cellM - c.zM;
        for (let x = minX; x <= maxX; x++) {
          const dx = (x + 0.5) * cellM - c.xM;
          // `Math.hypot` は精度の保護が実装依存で、エンジンが違うと結果が
          // 1 ulp ずれうる。フィクスチャ（`data/terrain/*.bin`）は CI で
          // 生成し直して突き合わせるので、ここは IEEE で厳密に定義された
          // `sqrt` だけで書く。
          const q = Math.sqrt(dx * dx + dz * dz) / radius;
          if (q >= 1) continue;
          const i = z * dim + x;
          height[i] = clampHeight(height[i]! + Math.round(residual * wendland(q)));
        }
      }
    }
  }
}

/** 海岸の遷移帯の幅（セル）。マップ幅の 15%。 */
function seaBand(dim: number): number {
  return Math.max(4, Math.floor((dim * 15) / 100));
}

/** 指定した辺から見たセルの距離（セル数）。 */
function seaEdgeDistance(x: number, y: number, dim: number, edge: SeaEdge): number {
  switch (edge) {
    case "north":
      return y;
    case "south":
      return dim - 1 - y;
    case "west":
      return x;
    case "east":
      return dim - 1 - x;
    default:
      return Number.MAX_SAFE_INTEGER;
  }
}

/**
 * 段階 4: 指定した辺に近いほど標高を引き下げ、海岸線を作る。
 *
 * 海岸ランプと段階 9 の海域判定は、必ずこの同じ帯の中でだけ効かせる。
 * 範囲を共有しないと、帯の外側でたまたま標高が海面を下回った内陸の低地まで
 * 「海」として扱ってしまう。
 */
function applySeaRamp(height: Int16Array, dim: number, p: TerrainParams): void {
  if (p.seaEdge === "none") return;
  const band = seaBand(dim);
  const seaFloorCm = p.seaLevelCm - 800;
  for (let y = 0; y < dim; y++) {
    for (let x = 0; x < dim; x++) {
      const dist = seaEdgeDistance(x, y, dim, p.seaEdge);
      if (dist >= band) continue;
      const i = y * dim + x;
      // 端で海面下 8 m、遷移帯の内側端で元の高さに一致する線形ランプ
      const t = clamp(dist / band, 0, 1);
      height[i] = clampHeight(Math.round(seaFloorCm + (height[i]! - seaFloorCm) * t));
    }
  }
}

/**
 * 段階 5: 熱浸食。安息角を超えた斜面を崩す。
 *
 * 差が `CLIFF_THRESHOLD_CM` 以上のペアは対象から除外する。これは緩い崖錐
 * （安息角を守るルーズな堆積物）と、侵食に耐える露岩の崖を区別するため。
 * 除外がないと、反復するうちにどんな急崖も安息角まで均されてしまい、
 * 崖グリッドが常に空になる。
 */
function thermalErosion(height: Int16Array, dim: number, cellM: number, iterations: number): void {
  // 安息角 35° ≒ 隣接セル間で cell_m(m) * tan(35°) ≈ cell_m(m) * 0.70 の高低差（cm）
  const talusCm = Math.max(20, cellM * 70);
  for (let it = 0; it < iterations; it++) {
    const src = Int16Array.from(height);
    for (let y = 0; y < dim; y++) {
      for (let x = 0; x < dim; x++) {
        const here = src[y * dim + x]!;
        let lowestDelta = 0;
        let lowestIdx = -1;
        for (let d = 0; d < 4; d++) {
          const nx = x + (d === 0 ? 1 : d === 1 ? -1 : 0);
          const ny = y + (d === 2 ? 1 : d === 3 ? -1 : 0);
          if (nx < 0 || ny < 0 || nx >= dim || ny >= dim) continue;
          const delta = here - src[ny * dim + nx]!;
          if (delta > talusCm && delta < CLIFF_THRESHOLD_CM && delta > lowestDelta) {
            lowestDelta = delta;
            lowestIdx = ny * dim + nx;
          }
        }
        if (lowestIdx >= 0) {
          // 超過分の 1/4 を最も低い隣へ移す
          const moveCm = Math.max(1, Math.floor((lowestDelta - talusCm) / 4));
          const hi = y * dim + x;
          height[hi] = clampHeight(height[hi]! - moveCm);
          height[lowestIdx] = clampHeight(height[lowestIdx]! + moveCm);
        }
      }
    }
  }
}

/** 段階 9: 海面より低い海岸帯のセルを海にする。 */
function applySea(
  height: Int16Array,
  depth: Uint16Array,
  kind: Uint8Array,
  dim: number,
  p: TerrainParams,
): void {
  if (p.seaEdge === "none") return;
  const band = seaBand(dim);
  for (let y = 0; y < dim; y++) {
    for (let x = 0; x < dim; x++) {
      if (seaEdgeDistance(x, y, dim, p.seaEdge) >= band) continue;
      const i = y * dim + x;
      const below = p.seaLevelCm - height[i]!;
      if (below > 0) {
        depth[i] = Math.min(65535, below);
        kind[i] = WaterKind.SEA;
      }
    }
  }
}

/** 各セルから最も近い水域までの距離（セル数）。多始点 BFS で O(n)。 */
function distanceToWater(dim: number, depth: Uint16Array, maxCells: number): Uint16Array {
  const n = dim * dim;
  const dist = new Uint16Array(n).fill(65535);
  const queue = new Int32Array(n);
  let qh = 0;
  let qt = 0;
  for (let i = 0; i < n; i++) {
    if (depth[i]! > 0) {
      dist[i] = 0;
      queue[qt++] = i;
    }
  }
  while (qh < qt) {
    const idx = queue[qh++]!;
    const d = dist[idx]!;
    if (d >= maxCells) continue;
    const cx = idx % dim;
    const cy = (idx / dim) | 0;
    for (let k = 0; k < 4; k++) {
      const nx = cx + (k === 0 ? 1 : k === 1 ? -1 : 0);
      const ny = cy + (k === 2 ? 1 : k === 3 ? -1 : 0);
      if (nx < 0 || ny < 0 || nx >= dim || ny >= dim) continue;
      const nidx = ny * dim + nx;
      if (dist[nidx] === 65535) {
        dist[nidx] = d + 1;
        queue[qt++] = nidx;
      }
    }
  }
  return dist;
}

/** 隣接 4 セルとの最大高低差（cm）。 */
function computeSlope(height: Int16Array, dim: number): Uint16Array {
  const slope = new Uint16Array(dim * dim);
  for (let y = 0; y < dim; y++) {
    for (let x = 0; x < dim; x++) {
      const i = y * dim + x;
      const here = height[i]!;
      let max = 0;
      for (let k = 0; k < 4; k++) {
        const nx = clamp(x + (k === 0 ? 1 : k === 1 ? -1 : 0), 0, dim - 1);
        const ny = clamp(y + (k === 2 ? 1 : k === 3 ? -1 : 0), 0, dim - 1);
        const d = Math.abs(here - height[ny * dim + nx]!);
        if (d > max) max = d;
      }
      slope[i] = Math.min(65535, max);
    }
  }
  return slope;
}

/**
 * 段階 10: 湿度。水域への近さ・ノイズ・傾斜から合成する。
 *
 * 配分（ノイズ 6 : 水域への近さ 4）は移行前の実装から引き継いでいる。
 * ここを変えると湿地・森の量が一斉に動くので、被覆率を調整したいときは
 * `marshBias` / `forestCover` の側で行うこと。
 *
 * 川べり・湖畔ほど高くなり、急斜面ほど水が抜けて下がる。泥濘化（天候）と
 * 植生判定の両方がこの 1 枚を読む。
 */
function computeMoisture(
  dim: number,
  cellM: number,
  extentM: number,
  seed: bigint,
  distToWater: Uint16Array,
  slopeCm: Uint16Array,
  p: TerrainParams,
): Uint8Array {
  const key = derive32(seed, Domain.MOISTURE);
  // 3 オクターブ。重みの合計が 1 なので、標本は -1..1 に収まる
  const octaves: Array<[NoiseField, number]> = [
    [new NoiseField(key, extentM, extentM, 600, 0), 0.6],
    [new NoiseField(key, extentM, extentM, 150, 1), 0.28],
    [new NoiseField(key, extentM, extentM, 40, 2), 0.12],
  ];
  const moisture = new Uint8Array(dim * dim);
  const marshBias = p.marshBias / 1000;
  const cellCm = cellM * 100;

  for (let z = 0; z < dim; z++) {
    const zm = (z + 0.5) * cellM;
    for (let x = 0; x < dim; x++) {
      const i = z * dim + x;
      const xm = (x + 0.5) * cellM;
      let noise = 0;
      for (const [field, weight] of octaves) noise += field.sample(xm, zm) * weight;
      const base = (noise + 1) / 2;
      const d = Math.min(distToWater[i]!, MOISTURE_RANGE_CELLS);
      const proximity = (MOISTURE_RANGE_CELLS - d) / MOISTURE_RANGE_CELLS;
      const slope = slopeCm[i]! / cellCm;
      const wet = clamp(base * 0.6 + proximity * 0.4 + marshBias * 0.15 - slope * 0.25, 0, 1);
      moisture[i] = Math.round(wet * 255);
    }
  }
  return moisture;
}

/**
 * 段階 11: 地質と植生を独立に割り当てる。
 *
 * 水域セルは河床・海底を置き、植生は持たない（水深グリッドが水そのものを
 * 表すので、地質を「水」にはしない）。陸地は傾斜と湿度から地質を決め、
 * その上に植生を独立に載せる。「砂地の疎林」のような組み合わせが自然に出る。
 */
function classifyCover(
  dim: number,
  cellM: number,
  extentM: number,
  seed: bigint,
  height: Int16Array,
  depth: Uint16Array,
  kind: Uint8Array,
  slopeCm: Uint16Array,
  moisture: Uint8Array,
  distToWater: Uint16Array,
  p: TerrainParams,
): { ground: Uint8Array; vegetation: Uint8Array } {
  const n = dim * dim;
  const ground = new Uint8Array(n);
  const vegetation = new Uint8Array(n);
  const vegA = new NoiseField(derive32(seed, Domain.VEGETATION), extentM, extentM, 320, 0);
  const vegB = new NoiseField(derive32(seed, Domain.VEGETATION, 1), extentM, extentM, 54, 1);
  const cellCm = cellM * 100;

  // 被覆率のバイアス。移行前の実装と同じ意味で、`forestCover` は「森になる
  // 湿度の下限」を押し下げる。300 なら湿度 0.70 超が林、0.85 超が密林。
  const forestThreshold = clamp(1 - p.forestCover / 1000, 0.05, 0.98);
  const denseThreshold = clamp(1 - p.forestCover / 2000, forestThreshold + 0.02, 0.99);
  const sparseThreshold = Math.max(0.3, forestThreshold - 0.08);
  const marshThreshold = 0.88 - p.marshBias / 4000;
  const sandDistCells = Math.round(clamp(1 + (p.sandBias / 1000) * 4, 0, 6));

  for (let z = 0; z < dim; z++) {
    const zm = (z + 0.5) * cellM;
    for (let x = 0; x < dim; x++) {
      const i = z * dim + x;
      const xm = (x + 0.5) * cellM;

      if (depth[i]! > 0) {
        // 河川は生成しないので、ここに来る淡水はすべて湖。`Ground.RIVER_BED`
        // という名前のまま流用している（`WaterKind.RIVER` ごと削っていない）
        ground[i] = kind[i] === WaterKind.SEA ? Ground.SEA_BED : Ground.RIVER_BED;
        vegetation[i] = Vegetation.NONE;
        continue;
      }

      const slope = slopeCm[i]! / cellCm;
      const wet = moisture[i]! / 255;

      // ---- 地質 ----
      // 泥はここでは作らない。生成時の泥濘は移行前の実装にも無く、
      // 泥はシナリオの整形・天候・踏み荒らしの結果として後から現れる。
      if (slope > 0.7) {
        ground[i] = Ground.ROCK;
        vegetation[i] = Vegetation.NONE;
        continue;
      }
      if (wet > marshThreshold && slope < 0.08) {
        ground[i] = Ground.MARSH;
        vegetation[i] = Vegetation.NONE;
        continue;
      }
      if (distToWater[i]! <= sandDistCells && wet < 0.62 && slope < 0.4) {
        ground[i] = Ground.SAND;
        vegetation[i] = Vegetation.NONE;
        continue;
      }
      ground[i] = wet > 0.3 ? Ground.GRASS : Ground.SOIL;

      // ---- 植生（地質とは独立の軸）----
      // 湿度に植生ノイズを少し混ぜ、同じ湿度でも林の縁が直線にならないようにする
      const noise = vegA.sample(xm, zm) * 0.1 + vegB.sample(xm, zm) * 0.05;
      const score = wet + noise - slope * 0.4;
      if (slope > 0.35) {
        vegetation[i] = Vegetation.SCRUB;
      } else if (score > denseThreshold) {
        vegetation[i] = Vegetation.DENSE;
      } else if (score > forestThreshold) {
        vegetation[i] = Vegetation.WOODLAND;
      } else if (score > sparseThreshold) {
        vegetation[i] = Vegetation.SPARSE;
      } else if (score > 0.42) {
        vegetation[i] = Vegetation.MEADOW;
      } else {
        vegetation[i] = Vegetation.SHORT_GRASS;
      }
      void height;
    }
  }
  return { ground, vegetation };
}

/** 森の境界をセルオートマトンで均し、斑にならないようにする。 */
function smoothForests(
  vegetation: Uint8Array,
  ground: Uint8Array,
  depth: Uint16Array,
  dim: number,
): void {
  const mutable = (i: number): boolean =>
    depth[i] === 0 &&
    ground[i] !== Ground.ROCK &&
    ground[i] !== Ground.MARSH &&
    ground[i] !== Ground.SAND;
  const isForest = (v: number): boolean => v === Vegetation.WOODLAND || v === Vegetation.DENSE;

  for (let pass = 0; pass < 3; pass++) {
    const src = Uint8Array.from(vegetation);
    for (let y = 0; y < dim; y++) {
      for (let x = 0; x < dim; x++) {
        const i = y * dim + x;
        if (!mutable(i)) continue;
        const v = src[i]!;
        if (v !== Vegetation.SHORT_GRASS && v !== Vegetation.MEADOW && !isForest(v)) continue;

        let forestNeighbors = 0;
        let total = 0;
        for (let dy = -1; dy <= 1; dy++) {
          for (let dx = -1; dx <= 1; dx++) {
            if (dx === 0 && dy === 0) continue;
            const nx = x + dx;
            const ny = y + dy;
            if (nx < 0 || ny < 0 || nx >= dim || ny >= dim) continue;
            total++;
            if (isForest(src[ny * dim + nx]!)) forestNeighbors++;
          }
        }
        if (isForest(v) && forestNeighbors * 2 < total) {
          vegetation[i] = Vegetation.MEADOW;
        } else if (!isForest(v) && forestNeighbors * 3 > total * 2) {
          vegetation[i] = Vegetation.WOODLAND;
        }
      }
    }
  }
}

/** 海に接する陸地セルを砂浜にする。 */
function applyBeaches(
  ground: Uint8Array,
  vegetation: Uint8Array,
  kind: Uint8Array,
  dim: number,
  sandBias: number,
): void {
  if (!kind.includes(WaterKind.SEA)) return;
  const width = Math.max(1, Math.round(1 + (sandBias / 1000) * 3));
  let frontier: number[] = [];
  const isBeachable = (i: number): boolean =>
    kind[i] === WaterKind.NONE && ground[i] !== Ground.ROCK && ground[i] !== Ground.SCREE;

  for (let i = 0; i < kind.length; i++) if (kind[i] === WaterKind.SEA) frontier.push(i);

  for (let layer = 0; layer < width; layer++) {
    const next: number[] = [];
    for (const idx of frontier) {
      const cx = idx % dim;
      const cy = (idx / dim) | 0;
      for (let k = 0; k < 4; k++) {
        const nx = cx + (k === 0 ? 1 : k === 1 ? -1 : 0);
        const ny = cy + (k === 2 ? 1 : k === 3 ? -1 : 0);
        if (nx < 0 || ny < 0 || nx >= dim || ny >= dim) continue;
        const nidx = ny * dim + nx;
        if (ground[nidx] === Ground.SAND || !isBeachable(nidx)) continue;
        ground[nidx] = Ground.SAND;
        vegetation[nidx] = Vegetation.NONE;
        next.push(nidx);
      }
    }
    frontier = next;
  }
}

/**
 * 崖の直下に崖錐（`Ground.SCREE`）を積む。
 *
 * 熱浸食が安息角まで崩した土砂は、実際には崖の足元へ溜まる。崖に隣接し、
 * かつ自分より 2.5 m 以上高い隣を持つ陸地セルをそれとみなす。
 */
function applyScree(
  ground: Uint8Array,
  vegetation: Uint8Array,
  height: Int16Array,
  depth: Uint16Array,
  dim: number,
): void {
  for (let y = 0; y < dim; y++) {
    for (let x = 0; x < dim; x++) {
      const i = y * dim + x;
      if (depth[i]! > 0 || ground[i] === Ground.ROCK) continue;
      let below = false;
      for (let k = 0; k < 4; k++) {
        const nx = x + (k === 0 ? 1 : k === 1 ? -1 : 0);
        const ny = y + (k === 2 ? 1 : k === 3 ? -1 : 0);
        if (nx < 0 || ny < 0 || nx >= dim || ny >= dim) continue;
        if (height[ny * dim + nx]! - height[i]! >= CLIFF_THRESHOLD_CM) {
          below = true;
          break;
        }
      }
      if (below) {
        ground[i] = Ground.SCREE;
        vegetation[i] = Vegetation.NONE;
      }
    }
  }
}

/** 段階 12: 崖エッジを検出する。仕様 03 章 2.5 節。 */
export function computeCliffs(height: Int16Array, dim: number): Uint8Array {
  const cliff = new Uint8Array(dim * dim);
  for (let y = 0; y < dim; y++) {
    for (let x = 0; x < dim; x++) {
      const i = y * dim + x;
      const here = height[i]!;
      let bits = 0;
      if (y > 0 && Math.abs(here - height[i - dim]!) >= CLIFF_THRESHOLD_CM) bits |= CliffBits.NORTH;
      if (x < dim - 1 && Math.abs(here - height[i + 1]!) >= CLIFF_THRESHOLD_CM) bits |= CliffBits.EAST;
      if (y < dim - 1 && Math.abs(here - height[i + dim]!) >= CLIFF_THRESHOLD_CM) bits |= CliffBits.SOUTH;
      if (x > 0 && Math.abs(here - height[i - 1]!) >= CLIFF_THRESHOLD_CM) bits |= CliffBits.WEST;
      cliff[i] = bits;
    }
  }
  return cliff;
}

/** 生成後にグリッドを書き換えたあと、崖と傾斜を取り直す。 */
export function recomputeDerived(g: GenerationResult): void {
  g.cliff = computeCliffs(g.height, g.dim);
  g.slopeCm = computeSlope(g.height, g.dim);
}
