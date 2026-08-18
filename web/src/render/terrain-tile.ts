/**
 * 描画用の地表タイル。
 *
 * シミュレーションの地形は地質・植生・水・人工物の 4 軸に分かれているが、
 * 描画は 1 セルにつき 1 枚のタイルを貼る。ここはその**表示だけの射影**で、
 * 4 軸から 16 種のタイルを選ぶ。ImageGen で事前生成した 4×4 のタイル
 * アトラス（`render/generated-assets.ts`）とミニマップの両方がこれを使う。
 *
 * 戦術判定にこの値を使ってはいけない。移動・遮蔽・防御は 4 軸の合成
 * （`sim-terrain` の効果テーブル）が決めるもので、ここで潰した情報は
 * 見た目のためだけに捨てている。
 */

import {
  FORD_MAX_DEPTH_CM,
  Ground,
  Overlay,
  SHALLOW_MAX_DEPTH_CM,
  Vegetation,
} from "../terrain/types";

/** アトラスのタイル番号。4×4 のアトラスに合わせて 16 種。 */
export const TerrainTile = {
  DEEP_WATER: 0,
  SHALLOW_WATER: 1,
  FORD: 2,
  MARSH: 3,
  MUD: 4,
  GRASS: 5,
  MEADOW: 6,
  FARMLAND: 7,
  SCRUB: 8,
  LIGHT_FOREST: 9,
  DENSE_FOREST: 10,
  ROCK: 11,
  SCREE: 12,
  SAND: 13,
  ROAD: 14,
  BRIDGE: 15,
} as const;

export const TILE_COUNT = 16;

/**
 * 4 軸から表示タイルを選ぶ。
 *
 * 優先順位は「上に乗っているものほど強い」——人工物、水、特徴のある地質、
 * 植生、の順。道の上に林を描いても、水の上に草を描いても嘘になる。
 */
export function tileFor(
  ground: number,
  vegetation: number,
  waterDepthCm: number,
  overlay: number,
): number {
  if (overlay === Overlay.ROAD) return TerrainTile.ROAD;
  if (overlay === Overlay.BRIDGE) return TerrainTile.BRIDGE;

  if (waterDepthCm > SHALLOW_MAX_DEPTH_CM) return TerrainTile.DEEP_WATER;
  if (waterDepthCm > FORD_MAX_DEPTH_CM) return TerrainTile.SHALLOW_WATER;
  if (waterDepthCm > 0) return TerrainTile.FORD;

  switch (ground) {
    case Ground.MARSH:
      return TerrainTile.MARSH;
    case Ground.MUD:
      return TerrainTile.MUD;
    case Ground.ROCK:
      return TerrainTile.ROCK;
    case Ground.SCREE:
      return TerrainTile.SCREE;
    case Ground.SAND:
      return TerrainTile.SAND;
    default:
      break;
  }

  switch (vegetation) {
    case Vegetation.DENSE:
      return TerrainTile.DENSE_FOREST;
    case Vegetation.WOODLAND:
    case Vegetation.SPARSE:
      return TerrainTile.LIGHT_FOREST;
    case Vegetation.SCRUB:
      return TerrainTile.SCRUB;
    case Vegetation.MEADOW:
      return TerrainTile.MEADOW;
    case Vegetation.FARMLAND:
      return TerrainTile.FARMLAND;
    default:
      return TerrainTile.GRASS;
  }
}

/**
 * タイルごとの基準色。アトラスが読めていないときのフォールバックと、
 * ミニマップの描画に使う、唯一の定義。
 */
export const TILE_COLORS: ReadonlyArray<readonly [number, number, number]> = [
  [26, 54, 92], // DeepWater
  [58, 110, 148], // ShallowWater
  [96, 140, 160], // Ford
  [74, 92, 66], // Marsh
  [104, 88, 64], // Mud
  [104, 132, 74], // Grass
  [122, 148, 82], // Meadow
  [154, 142, 90], // Farmland
  [110, 116, 70], // Scrub
  [64, 100, 58], // LightForest
  [40, 72, 44], // DenseForest
  [130, 126, 118], // Rock
  [146, 138, 124], // Scree
  [198, 182, 138], // Sand
  [150, 132, 104], // Road
  [128, 100, 72], // Bridge
];
