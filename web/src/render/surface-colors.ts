/**
 * 地表タイプごとの基準色。
 *
 * sim-terrain の `Surface` enum と同じ並び（仕様 03 章 2.4 節）。
 * 地形の WebGL2 描画とミニマップの両方から参照する、唯一の定義。
 */
export const SURFACE_COLORS: ReadonlyArray<readonly [number, number, number]> = [
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
