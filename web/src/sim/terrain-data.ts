/**
 * 地形データの共通型とヘルパ。
 *
 * ワーカーから届いた地形グリッドを、地形レンダラ（terrain-gl.ts）と
 * ミニマップ（minimap.ts）の両方から同じ形で参照するために独立させてある。
 */

/** crates/sim-terrain/src/lib.rs の WaterKind と同じ並び。 */
export enum WaterKind {
  None = 0,
  Lake = 1,
  River = 2,
  Sea = 3,
}

/** crates/sim-terrain/src/lib.rs の cliff_bits と同じビット割り当て。 */
export const CliffBits = {
  NORTH: 1 << 0,
  EAST: 1 << 1,
  SOUTH: 1 << 2,
  WEST: 1 << 3,
} as const;

export interface TerrainData {
  dim: number;
  cellM: number;
  sizeM: number;
  surface: Uint8Array;
  height: Int16Array;
  /** 水深（10 cm 単位）。0 = 陸地。 */
  water: Uint8Array;
  waterKind: Uint8Array;
  /** 崖ビットマスク（[`CliffBits`]）。 */
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

export function waterKindAt(data: TerrainData, x: number, y: number): WaterKind {
  return data.waterKind[cellIndex(data, x, y)]! as WaterKind;
}
