/**
 * 地形データの共通型とヘルパ。
 *
 * ワーカーから届いた地形グリッドを、地形レンダラ（render/terrain-gl.ts）と
 * ミニマップ（render/minimap.ts）の両方から同じ形で参照するために独立させてある。
 *
 * 属性は直交した軸に分かれている（`terrain/types.ts` と
 * `crates/sim-terrain/src/lib.rs` が同じ定義を持つ）。描画で 1 枚のタイルに
 * 潰すのは `render/terrain-tile.ts` の仕事。
 */

import { tileFor } from "../render/terrain-tile";

export { WaterKind, CliffBits } from "../terrain/types";

export interface TerrainData {
  dim: number;
  cellM: number;
  sizeM: number;
  /** 標高（cm） */
  height: Int16Array;
  /** 地質（`Ground`） */
  ground: Uint8Array;
  /** 植生（`Vegetation`） */
  vegetation: Uint8Array;
  /** 人工物（`Overlay`） */
  overlay: Uint8Array;
  /** 水深（cm）。0 = 陸地 */
  water: Uint16Array;
  /** 水域種別（`WaterKind`） */
  waterKind: Uint8Array;
  /** 崖ビットマスク（`CliffBits`） */
  cliff: Uint8Array;
}

export function cellIndex(data: TerrainData, x: number, y: number): number {
  const cx = Math.max(0, Math.min(data.dim - 1, x));
  const cy = Math.max(0, Math.min(data.dim - 1, y));
  return cy * data.dim + cx;
}

export function cliffAt(data: TerrainData, x: number, y: number): number {
  return data.cliff[cellIndex(data, x, y)]!;
}

export function waterKindAt(data: TerrainData, x: number, y: number): number {
  return data.waterKind[cellIndex(data, x, y)]!;
}

/** セルの表示タイル（`render/terrain-tile.ts`）。 */
export function tileAtIndex(data: TerrainData, i: number): number {
  return tileFor(data.ground[i]!, data.vegetation[i]!, data.water[i]!, data.overlay[i]!);
}
