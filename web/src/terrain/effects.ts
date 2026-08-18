/**
 * 地形が兵士に与える効果。仕様 `docs/spec/03-terrain.md` 3 節。
 *
 * **この 4 つのテーブルが効果値の正本**で、`crates/sim-terrain/src/effects.rs`
 * はその写しである。両者が食い違うと、生成時に敷いた道路のコストと
 * シミュレーション中の移動速度が噛み合わなくなる（地形は見た目どおりなのに
 * 兵が動かない、という形で出る）。`tools/check_terrain_effects.mjs` が
 * 両ファイルを突き合わせて CI で落とす。
 *
 * 効果は 4 つの軸から**合成**する。統合された地表 ID を引くのではなく、
 * 地質・植生・水・人工物のそれぞれが独立に倍率を持ち、掛け合わせる。
 * これにより「砂地の疎林」のような、テーブルに書いていない組み合わせも
 * 自動的に意味のある値になる。
 */

import {
  FORD_MAX_DEPTH_CM,
  Ground,
  Overlay,
  SHALLOW_MAX_DEPTH_CM,
  Vegetation,
  type GroundId,
  type OverlayId,
  type VegetationId,
} from "./types";

/** すべて 1000 分率の倍率。1000 = 等倍。 */
export interface TerrainEffect {
  /** 移動速度倍率 */
  moveMult: number;
  /** 隊列維持のしやすさ */
  cohesionMult: number;
  /** 視界遮蔽率（0..1000）。合成では軸の最大値を採る */
  cover: number;
  /** 防御倍率 */
  defenseMult: number;
  /** 疲労消費倍率 */
  fatigueMult: number;
  /** 騎兵の可否（0 = 進入不可、1000 = 制限なし） */
  cavalryMult: number;
}

function eff(
  moveMult: number,
  cohesionMult: number,
  cover: number,
  defenseMult: number,
  fatigueMult: number,
  cavalryMult: number,
): TerrainEffect {
  return { moveMult, cohesionMult, cover, defenseMult, fatigueMult, cavalryMult };
}

/** 効果を持たない中立値。合成の単位元。 */
export const NEUTRAL: TerrainEffect = eff(1000, 1000, 0, 1000, 1000, 1000);

/** 地質の効果。`Ground` の並びと 1 対 1。 */
export const GROUND_EFFECTS: readonly TerrainEffect[] = [
  eff(1000, 1000, 0, 1000, 1000, 1000), // SOIL
  eff(1000, 1000, 0, 1000, 1000, 1000), // GRASS
  eff(850, 850, 0, 1000, 1250, 700), // SAND
  eff(700, 600, 100, 1100, 1400, 500), // ROCK
  eff(500, 600, 0, 900, 1900, 400), // MUD
  eff(1000, 1000, 0, 1000, 1000, 1000), // RIVER_BED（水そのものの penalty は水軸が持つ）
  eff(1000, 1000, 0, 1000, 1000, 1000), // SEA_BED
  eff(600, 450, 0, 1000, 1500, 0), // SCREE
  eff(350, 400, 200, 800, 2200, 0), // MARSH
];

/** 植生の効果。`Vegetation` の並びと 1 対 1。 */
export const VEGETATION_EFFECTS: readonly TerrainEffect[] = [
  eff(1000, 1000, 0, 1000, 1000, 1000), // NONE
  eff(1000, 1000, 0, 1000, 1000, 1000), // SHORT_GRASS
  eff(850, 800, 300, 1050, 1150, 500), // SPARSE（疎林。騎兵は通れるが速度が落ちる）
  eff(650, 500, 600, 1100, 1300, 0), // WOODLAND
  eff(400, 250, 900, 1200, 1600, 0), // DENSE
  eff(800, 750, 250, 1000, 1200, 700), // SCRUB
  eff(900, 900, 150, 1000, 1100, 1000), // MEADOW
  eff(950, 950, 50, 1000, 1050, 1000), // FARMLAND
];

/**
 * 人工物の効果。**合成ではなく上書き**する。
 *
 * 道は森を切り開いて敷くものなので、路面の上に林の減速が残るのはおかしい。
 * 橋も同じで、下の水深は移動に効かない。
 */
export const OVERLAY_EFFECTS: readonly TerrainEffect[] = [
  NEUTRAL, // NONE（使われない）
  eff(1150, 1100, 0, 1000, 900, 1000), // ROAD
  eff(1000, 1000, 0, 900, 1000, 1000), // BRIDGE
];

/**
 * 水深の効果。深さで 3 段に分かれる。
 *
 * 浅瀬の防御 600 が意味するもの: 川を渡っている最中の部隊は隊列が崩れ、
 * 防御が 4 割落ちる。渡渉中に襲われると壊滅する。中世会戦の定番だった
 * 「渡渉点を押さえる」戦術が、この 1 行から出る。
 */
export function waterEffect(depthCm: number): TerrainEffect {
  if (depthCm <= 0) return NEUTRAL;
  if (depthCm <= FORD_MAX_DEPTH_CM) return eff(550, 400, 0, 700, 1500, 900);
  if (depthCm <= SHALLOW_MAX_DEPTH_CM) return eff(400, 300, 0, 600, 1800, 300);
  return eff(0, 0, 0, 1000, 1000, 0);
}

function mul(a: number, b: number, c: number): number {
  return Math.floor((a * b * c) / 1_000_000);
}

/**
 * 4 軸から効果を合成する。
 *
 * 倍率は掛け算、遮蔽だけは最大値を採る（林の中の泥地は、泥のぶん遅くなるが
 * 遮蔽は林のぶんだけあり、足し合わさりはしない）。
 */
export function composeEffect(
  ground: number,
  vegetation: number,
  waterDepthCm: number,
  overlay: number,
): TerrainEffect {
  if (overlay !== Overlay.NONE) {
    return OVERLAY_EFFECTS[overlay] ?? NEUTRAL;
  }
  const g = GROUND_EFFECTS[ground] ?? NEUTRAL;
  const v = VEGETATION_EFFECTS[vegetation] ?? NEUTRAL;
  const w = waterEffect(waterDepthCm);
  return {
    moveMult: mul(g.moveMult, v.moveMult, w.moveMult),
    cohesionMult: mul(g.cohesionMult, v.cohesionMult, w.cohesionMult),
    cover: Math.max(g.cover, v.cover, w.cover),
    defenseMult: mul(g.defenseMult, v.defenseMult, w.defenseMult),
    fatigueMult: mul(g.fatigueMult, v.fatigueMult, w.fatigueMult),
    cavalryMult: mul(g.cavalryMult, v.cavalryMult, w.cavalryMult),
  };
}

/** 傾斜を無視した通行コストの基準値（0 = 不通、255 = 最速）。 */
export function basePassability(
  ground: number,
  vegetation: number,
  waterDepthCm: number,
  overlay: number,
): number {
  const e = composeEffect(ground, vegetation, waterDepthCm, overlay);
  if (e.moveMult <= 0) return 0;
  return Math.min(255, Math.max(1, Math.floor((e.moveMult * 255) / 1150)));
}

/**
 * 傾斜のペナルティを織り込んだ通行コスト。
 *
 * `slopeCm` は隣接 4 セルとの最大高低差（cm）、`cellM` はセルの一辺（m）。
 * tan 0.55 を超える斜面は通行不能（仕様 03 章 3.1 節）。
 */
export function passabilityWithSlope(
  ground: number,
  vegetation: number,
  waterDepthCm: number,
  overlay: number,
  slopeCm: number,
  cellM: number,
): number {
  const base = basePassability(ground, vegetation, waterDepthCm, overlay);
  if (base === 0) return 0;
  const slopePercent = Math.floor((slopeCm * 100) / (cellM * 100));
  if (slopePercent > 55) return 0;
  const penalty = Math.min(55, slopePercent);
  const v = base - Math.floor((base * penalty) / 100);
  return Math.min(255, Math.max(1, v));
}

/** 徒渉できない深さか。 */
export function isDeepWater(depthCm: number): boolean {
  return depthCm > SHALLOW_MAX_DEPTH_CM;
}

/** 林として扱う植生か（工兵の伐採対象、遮蔽の判定）。 */
export function isForest(vegetation: number): boolean {
  return vegetation === Vegetation.WOODLAND || vegetation === Vegetation.DENSE;
}

/** 掘削・整地の作業速度倍率（1000 分率）。仕様 07 章 3 節。 */
export function workMultPermille(ground: number, vegetation: number, depthCm: number): number {
  if (isDeepWater(depthCm)) return 0;
  if (ground === Ground.ROCK || ground === Ground.SCREE) return 400;
  if (vegetation === Vegetation.DENSE) return 700;
  if (vegetation === Vegetation.WOODLAND || vegetation === Vegetation.SCRUB) return 850;
  if (ground === Ground.MUD || ground === Ground.MARSH) return 1300;
  if (depthCm > 0 || ground === Ground.SAND) return 900;
  return 1000;
}

export type { GroundId, VegetationId, OverlayId };
