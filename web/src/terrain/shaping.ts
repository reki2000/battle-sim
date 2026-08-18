/**
 * シナリオによる地形の整形（stamping）。仕様 `docs/spec/03-terrain.md` 3.3 節。
 *
 * 手続き生成された地形は「それらしい野原」までしか作れない。史実の会戦を
 * 再現するには、その上に**その会戦固有の地勢**——森の縁、雨で泥濘化した耕地、
 * 緩い登り——を焼き込む必要がある。ここはそのための最小の道具立てで、
 * シナリオが宣言的に並べたスタンプを生成の最後に一度だけ適用する。
 *
 * 属性が直交したことで、スタンプは**触りたい軸だけ**を指定できる。
 * 「刈り取りの済んだ耕地」は植生だけを `FARMLAND` にすればよく、下の土は
 * 生成器が決めたままでよい。「泥濘」は地質を `MUD` にして植生を剥ぐ。
 *
 * 浮動小数点は使わない。座標は m 単位の整数、標高は cm 単位の整数で扱う。
 */

import { recomputeDerived, type GenerationResult } from "./generate";
import { Vegetation, WaterKind } from "./types";

/** 地質・植生のどちらか（または両方）を指定する塗り。省略した軸は触らない。 */
export interface CoverPaint {
  ground?: number;
  vegetation?: number;
  /**
   * 塗る前に水を抜く。
   *
   * 既定では水域のセルを塗り替えない（水深と地質が食い違うと描画も水系
   * ロジックも壊れる）。ただし「ここは森だ」と宣言する整形にとって、
   * 手続き生成が置いた湖は消すべき対象であって、守るべき地物ではない。
   * 森の帯が湖に穴を開けられると、会戦場を挟むはずの森が途切れる。
   */
  drain?: boolean;
}

export type Stamp =
  /** 軸並行の矩形を塗る（耕地、泥濘の帯など）。 */
  | ({ kind: "cover_rect"; xM: number; yM: number; wM: number; hM: number } & CoverPaint)
  /**
   * 線分 a-b から `halfWidthM` 以内を塗る帯。斜めに走る森の縁を表すために使う
   * （矩形では会戦場が狭まっていく形を作れない）。
   */
  | ({
      kind: "cover_belt";
      axM: number;
      ayM: number;
      bxM: number;
      byM: number;
      halfWidthM: number;
    } & CoverPaint)
  /**
   * 矩形から水を抜く（水深 0・種別なし）。
   *
   * 手続き生成は窪地を湖として埋めるので、シナリオが「ここは乾いた会戦場だ」
   * と決めた区画にも水が残ることがある。水域のセルは `cover_rect` などの
   * 塗り替え対象から外れる（水深と地質が食い違うと描画も水系ロジックも壊れる）
   * ので、先にここで水を抜く。水は騎兵を完全に止めるため、展開地に湖が残った
   * まま騎兵を置くと、動けない兵の塊ができて隊列が押し合いで自壊する。
   */
  | ({ kind: "drain"; xM: number; yM: number; wM: number; hM: number } & CoverPaint)
  /**
   * `axis` 方向に沿って標高へ緩い勾配を足す。`fromM` 以前は `fromCm`、
   * `toM` 以降は `toCm` で一定なので、どこにも段差ができない
   * （矩形に限定すると境界が崖になってしまう）。
   */
  | {
      kind: "height_ramp";
      axis: "x" | "y";
      fromM: number;
      toM: number;
      fromCm: number;
      toCm: number;
    };

/** スタンプを順に適用し、派生グリッド（崖・傾斜）を整合させる。 */
export function applyStamps(t: GenerationResult, stamps: readonly Stamp[]): void {
  if (stamps.length === 0) return;
  for (const stamp of stamps) applyOne(t, stamp);
  recomputeDerived(t);
}

function applyOne(t: GenerationResult, stamp: Stamp): void {
  switch (stamp.kind) {
    case "cover_rect":
      paintCells(t, (x, y) => inRect(x, y, stamp.xM, stamp.yM, stamp.wM, stamp.hM), stamp);
      break;

    case "cover_belt": {
      const limitSq = stamp.halfWidthM * stamp.halfWidthM;
      paintCells(
        t,
        (x, y) => distSqToSegment(x, y, stamp.axM, stamp.ayM, stamp.bxM, stamp.byM) <= limitSq,
        stamp,
      );
      break;
    }

    case "drain":
      forEachCell(t, (i, x, y) => {
        if (!inRect(x, y, stamp.xM, stamp.yM, stamp.wM, stamp.hM)) return;
        t.water[i] = 0;
        t.waterKind[i] = WaterKind.NONE;
        paint(t, i, stamp);
      });
      break;

    case "height_ramp":
      forEachCell(t, (i, x, y) => {
        const p = stamp.axis === "x" ? x : y;
        const delta = rampValueCm(p, stamp.fromM, stamp.toM, stamp.fromCm, stamp.toCm);
        t.height[i] = Math.max(-32768, Math.min(32767, t.height[i]! + delta));
      });
      break;
  }
}

/**
 * 塗りを適用する。**水域のセルは塗り替えない**——水深と地質が食い違うと
 * 描画も水系ロジックも壊れるため。水を消したいなら先に `drain` を置く。
 */
function paintCells(
  t: GenerationResult,
  hit: (xM: number, yM: number) => boolean,
  cover: CoverPaint,
): void {
  forEachCell(t, (i, x, y) => {
    if (t.water[i]! > 0 && !cover.drain) return;
    if (!hit(x, y)) return;
    if (t.water[i]! > 0) {
      t.water[i] = 0;
      t.waterKind[i] = WaterKind.NONE;
    }
    paint(t, i, cover);
  });
}

function paint(t: GenerationResult, i: number, cover: CoverPaint): void {
  if (cover.ground !== undefined) t.ground[i] = cover.ground;
  if (cover.vegetation !== undefined) t.vegetation[i] = cover.vegetation;
}

function forEachCell(
  t: GenerationResult,
  visit: (i: number, xM: number, yM: number) => void,
): void {
  const half = t.cellM >> 1;
  for (let cy = 0; cy < t.dim; cy++) {
    const yM = cy * t.cellM + half;
    for (let cx = 0; cx < t.dim; cx++) {
      visit(cy * t.dim + cx, cx * t.cellM + half, yM);
    }
  }
}

function inRect(x: number, y: number, xM: number, yM: number, wM: number, hM: number): boolean {
  return x >= xM && x < xM + wM && y >= yM && y < yM + hM;
}

/** `fromM`..`toM` を `fromCm`..`toCm` で線形補間し、範囲外は端の値で一定。 */
export function rampValueCm(
  p: number,
  fromM: number,
  toM: number,
  fromCm: number,
  toCm: number,
): number {
  const ascending = fromM <= toM;
  const lo = ascending ? fromM : toM;
  const hi = ascending ? toM : fromM;
  const loCm = ascending ? fromCm : toCm;
  const hiCm = ascending ? toCm : fromCm;
  if (p <= lo || hi === lo) return loCm;
  if (p >= hi) return hiCm;
  return loCm + Math.trunc(((hiCm - loCm) * (p - lo)) / (hi - lo));
}

/** 点から線分までの距離の 2 乗（m²）。 */
export function distSqToSegment(
  px: number,
  py: number,
  ax: number,
  ay: number,
  bx: number,
  by: number,
): number {
  const abx = bx - ax;
  const aby = by - ay;
  const apx = px - ax;
  const apy = py - ay;
  const lenSq = abx * abx + aby * aby;
  if (lenSq === 0) return apx * apx + apy * apy;
  const t = Math.max(0, Math.min(1, (apx * abx + apy * aby) / lenSq));
  const cx = apx - abx * t;
  const cy = apy - aby * t;
  return cx * cx + cy * cy;
}

/** 「森を植える」など、よく使う塗りの組み合わせ。 */
export const Cover = {
  denseForest: { vegetation: Vegetation.DENSE } satisfies CoverPaint,
  woodland: { vegetation: Vegetation.WOODLAND } satisfies CoverPaint,
  farmland: { vegetation: Vegetation.FARMLAND } satisfies CoverPaint,
  meadow: { vegetation: Vegetation.MEADOW } satisfies CoverPaint,
} as const;
