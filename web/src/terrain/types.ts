/**
 * 地形グリッドの型と属性 ID。
 *
 * 仕様は `docs/spec/03-terrain.md`。地形の属性は**直交した軸**に分けて持つ。
 * 一つの `Surface` 列挙に地質・植生・水・人工物を詰め込むと、「砂地の疎林」
 * のような組み合わせが表現できず、工兵の伐採（植生だけ消す）が地質まで
 * 書き換えてしまう。軸を分けたことで、それぞれが独立に読み書きできる。
 *
 * この定義は `crates/sim-terrain/src/lib.rs` の同名の列挙と 1 対 1 に対応する。
 * 値を足すときは両方を直すこと（`tools/check_terrain_effects.mjs` が検査する）。
 */

/** セルの地質・地表素材。植生とは独立。 */
export const Ground = {
  /** 裸地・締まった土 */
  SOIL: 0,
  /** 草地の土壌 */
  GRASS: 1,
  SAND: 2,
  ROCK: 3,
  MUD: 4,
  /** 河床。水深は water グリッドが持つ */
  RIVER_BED: 5,
  /** 海底 */
  SEA_BED: 6,
  /** 崖錐。熱浸食が崖の下に積む不安定な礫 */
  SCREE: 7,
  /** 湿地の土壌 */
  MARSH: 8,
} as const;
export type GroundId = (typeof Ground)[keyof typeof Ground];
export const GROUND_COUNT = 9;

/** セルの植生。地質とは独立。 */
export const Vegetation = {
  NONE: 0,
  /** 短い草。遮蔽にならない */
  SHORT_GRASS: 1,
  /** 疎林 */
  SPARSE: 2,
  /** 林 */
  WOODLAND: 3,
  /** 密林 */
  DENSE: 4,
  /** 灌木 */
  SCRUB: 5,
  /** 丈の高い草地 */
  MEADOW: 6,
  /** 耕地 */
  FARMLAND: 7,
} as const;
export type VegetationId = (typeof Vegetation)[keyof typeof Vegetation];
export const VEGETATION_COUNT = 8;

/** 人工物のオーバーレイ。地質・植生の上に重なり、効果を上書きする。 */
export const Overlay = {
  NONE: 0,
  ROAD: 1,
  BRIDGE: 2,
} as const;
export type OverlayId = (typeof Overlay)[keyof typeof Overlay];
export const OVERLAY_COUNT = 3;

/** 水域の種別。水深（cm）と対になるメタデータ。 */
export const WaterKind = {
  NONE: 0,
  LAKE: 1,
  RIVER: 2,
  SEA: 3,
} as const;
export type WaterKindId = (typeof WaterKind)[keyof typeof WaterKind];

/** 崖ビット。隣接セルとの高低差が閾値を超えるとそのビットが立つ。 */
export const CliffBits = {
  /** y - 1 側 */
  NORTH: 1 << 0,
  /** x + 1 側 */
  EAST: 1 << 1,
  /** y + 1 側 */
  SOUTH: 1 << 2,
  /** x - 1 側 */
  WEST: 1 << 3,
} as const;

/** 崖として扱う隣接セル間の高低差（cm）。仕様 03 章 2.5 節。 */
export const CLIFF_THRESHOLD_CM = 250;

/** 渡渉可能な最大水深（cm）。これ以下の河川セルが渡渉点になる。 */
export const FORD_MAX_DEPTH_CM = 40;
/** 浅瀬の上限水深（cm）。これを超えると徒歩でも渡れない。 */
export const SHALLOW_MAX_DEPTH_CM = 150;

/** 海の付け方。指定した辺に沿って標高を下げ、海岸線を作る。 */
export type SeaEdge = "none" | "north" | "east" | "south" | "west";

/** 生成パラメータ。仕様 03 章 6 節。 */
export interface TerrainParams {
  /** 64 bit のワールドシード。BigInt か 10/16 進の文字列で与える */
  seed: bigint | string | number;
  /** マップの一辺（m） */
  sizeM: number;
  /** セルの一辺（m） */
  cellM: number;
  /** 起伏の激しさ。0 = 平原、1000 = 山岳 */
  relief: number;
  /** 基準標高（m） */
  baseHeightM: number;
  /** 熱浸食の反復回数 */
  thermalIterations: number;
  /** 河川の密度。0 = なし、1000 = 網目状 */
  riverDensity: number;
  /** 敷設する道路の本数 */
  roadCount: number;
  /** 海の有無と位置 */
  seaEdge: SeaEdge;
  /** 海面の標高（cm） */
  seaLevelCm: number;
  /** 森の被覆率（1000 分率）。ハードな割当ではなくバイアス */
  forestCover: number;
  /** 湿地のできやすさ（1000 分率） */
  marshBias: number;
  /** 砂地の出やすさ（1000 分率） */
  sandBias: number;
  /** 点標高制約。指定した地点の標高を、半径内で滑らかに引き寄せる */
  constraints?: HeightConstraint[];
}

/**
 * 点標高制約。`radiusM` の縁でちょうど 0 に減衰する Wendland 核で、
 * 現在の標高と目標の残差を投影する。
 */
export interface HeightConstraint {
  xM: number;
  zM: number;
  heightM: number;
  radiusM: number;
}

/** 会戦地の候補。仕様 03 章 2.6 節（段階 14）。 */
export interface BattleSiteCandidate {
  xM: number;
  yM: number;
  score: number;
  /** 展開可能面積の比率（1000 分率） */
  passablePermille: number;
  /** 地形の非対称性（1000 分率、大きいほど南北で地形差がある） */
  asymmetryPermille: number;
  /** 視界の開け具合（1000 分率） */
  opennessPermille: number;
  /** ボトルネック（渡渉点・橋・浅瀬）のサンプル数 */
  bottleneckCount: number;
}

/**
 * 生成された地形。各グリッドは行優先（`y * dim + x`）。
 *
 * `passability` はここには無い。効果テーブルからの導出値であり、
 * テーブルの正本を持つ側（Rust の `sim-terrain`）が読み込み時に計算する。
 */
export interface TerrainGrids {
  dim: number;
  cellM: number;
  seed: bigint;
  /** 標高（cm） */
  height: Int16Array;
  ground: Uint8Array;
  vegetation: Uint8Array;
  overlay: Uint8Array;
  /** 水深（cm）。0 = 陸地 */
  water: Uint16Array;
  waterKind: Uint8Array;
  /** 湿度（0..255）。泥濘化と植生判定が使う */
  moisture: Uint8Array;
  /** 崖ビットマスク（`CliffBits`） */
  cliff: Uint8Array;
  battleSites: BattleSiteCandidate[];
}

/** マップの一辺（m）。 */
export function sizeM(t: Pick<TerrainGrids, "dim" | "cellM">): number {
  return t.dim * t.cellM;
}

export function cellIndex(dim: number, x: number, y: number): number {
  const cx = Math.max(0, Math.min(dim - 1, x));
  const cy = Math.max(0, Math.min(dim - 1, y));
  return cy * dim + cx;
}
