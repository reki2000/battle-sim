/**
 * 会戦地の自動評価。仕様 `docs/spec/03-terrain.md` 2 節（段階 14）。
 *
 * 「ここで会戦をやるとおもしろい」場所をスコアリングで探す。評価するのは
 * 展開可能面積・地形の非対称性・ボトルネックの存在・視界の開け具合の 4 つで、
 * これらの合成として上位候補を返す。実際の初期配置決定はシナリオ側の仕事で、
 * ここでは候補地点とその評価内訳を提供するだけ。
 */

import { composeEffect, passabilityWithSlope } from "./effects";
import { FORD_MAX_DEPTH_CM, SHALLOW_MAX_DEPTH_CM, type BattleSiteCandidate } from "./types";
import type { GenerationResult } from "./generate";

const RADIUS_M = 250;
const CANDIDATE_STRIDE_M = 300;
const SAMPLE_STEP_M = 16;
/** 非対称性を 1000 分率に正規化するときの基準（cm）。これ以上の高低差で頭打ち。 */
const ASYMMETRY_CAP_CM = 600;
/** 開放度が最も好ましいとみなす値（1000 分率）。開けすぎず閉じすぎず。 */
const OPENNESS_TARGET = 650;

/**
 * 会戦地の候補を評価し、スコア降順で上位 `topN` 件を返す。
 *
 * マップが小さすぎて半径ぶんの窓が取れない場合は空を返す。
 */
export function evaluateBattleSites(t: GenerationResult, topN: number): BattleSiteCandidate[] {
  const sizeM = t.dim * t.cellM;
  const radius = Math.max(t.cellM * 8, Math.min(RADIUS_M, Math.floor(sizeM / 3)));
  if (sizeM < radius * 2 + CANDIDATE_STRIDE_M) return [];

  const stride = Math.max(CANDIDATE_STRIDE_M, t.cellM * 4);
  const step = Math.max(SAMPLE_STEP_M, t.cellM);
  const candidates: BattleSiteCandidate[] = [];

  for (let cy = radius; cy <= sizeM - radius; cy += stride) {
    for (let cx = radius; cx <= sizeM - radius; cx += stride) {
      const c = evaluateOne(t, cx, cy, radius, step);
      if (c) candidates.push(c);
    }
  }

  candidates.sort((a, b) => b.score - a.score || a.xM - b.xM || a.yM - b.yM);
  return candidates.slice(0, topN);
}

function evaluateOne(
  t: GenerationResult,
  cxM: number,
  cyM: number,
  radiusM: number,
  stepM: number,
): BattleSiteCandidate | null {
  const cellM = t.cellM;
  let total = 0;
  let passable = 0;
  let coverSum = 0;
  let northSum = 0;
  let northCount = 0;
  let southSum = 0;
  let southCount = 0;
  let bottleneck = 0;

  for (let dy = -radiusM; dy <= radiusM; dy += stepM) {
    for (let dx = -radiusM; dx <= radiusM; dx += stepM) {
      if (dx * dx + dy * dy > radiusM * radiusM) continue;
      const gx = Math.max(0, Math.min(t.dim - 1, Math.floor((cxM + dx) / cellM)));
      const gy = Math.max(0, Math.min(t.dim - 1, Math.floor((cyM + dy) / cellM)));
      const i = gy * t.dim + gx;

      total++;
      const depth = t.water[i]!;
      const eff = composeEffect(t.ground[i]!, t.vegetation[i]!, depth, t.overlay[i]!);
      const pass = passabilityWithSlope(
        t.ground[i]!,
        t.vegetation[i]!,
        depth,
        t.overlay[i]!,
        t.slopeCm[i]!,
        cellM,
      );
      if (pass > 0) passable++;
      coverSum += eff.cover;

      // 渡渉点・橋・浅瀬。少数の兵しか同時に通れない場所が戦術を作る
      if (
        t.overlay[i] === 2 ||
        (depth > 0 && depth <= SHALLOW_MAX_DEPTH_CM) ||
        (depth > 0 && depth <= FORD_MAX_DEPTH_CM)
      ) {
        bottleneck++;
      }

      const h = t.height[i]!;
      if (dy < 0) {
        northSum += h;
        northCount++;
      } else {
        southSum += h;
        southCount++;
      }
    }
  }

  if (total === 0 || northCount === 0 || southCount === 0) return null;

  const passablePermille = Math.max(0, Math.min(1000, Math.floor((passable * 1000) / total)));
  const avgCover = Math.floor(coverSum / total);
  const opennessPermille = Math.max(0, Math.min(1000, 1000 - avgCover));
  const heightDiffCm = Math.abs(Math.floor(northSum / northCount) - Math.floor(southSum / southCount));
  const asymmetryPermille = Math.floor(
    (Math.min(heightDiffCm, ASYMMETRY_CAP_CM) * 1000) / ASYMMETRY_CAP_CM,
  );

  const opennessScore = 1000 - Math.abs(opennessPermille - OPENNESS_TARGET);
  const score =
    passablePermille * 3 + opennessScore * 2 + asymmetryPermille + Math.min(bottleneck, 20) * 50;

  return {
    xM: cxM,
    yM: cyM,
    score,
    passablePermille,
    asymmetryPermille,
    opennessPermille,
    bottleneckCount: Math.min(65535, bottleneck),
  };
}
