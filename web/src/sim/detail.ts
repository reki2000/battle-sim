/**
 * 兵士 1 体・指揮官 1 人ぶんの詳細情報を、`sim-wasm` のフラット配列 / JSON
 * 文字列レスポンスから構造化する（低頻度・クリック時呼び出し想定、M8）。
 *
 * レイアウトは `crates/sim-wasm/src/lib.rs` の対応する関数と一致させること。
 */

import type { CommanderAttrs, CommanderInfo, DecisionRecord, SituationAssessment, SoldierDetail } from "./protocol";

export function parseSoldierDetail(flat: number[]): SoldierDetail | null {
  if (flat.length < 9) return null;
  const target = flat[4]!;
  return {
    hp: flat[0]!,
    morale: flat[1]!,
    fatigue: flat[2]!,
    ammo: flat[3]!,
    target: target < 0 ? null : target,
    bravery: flat[5]!,
    discipline: flat[6]!,
    skill: flat[7]!,
    weaponReachMm: flat[8]!,
  };
}

export function parseCommanderAttrs(flat: number[]): CommanderAttrs | null {
  if (flat.length < 10) return null;
  return {
    boldness: flat[0]!,
    caution: flat[1]!,
    initiative: flat[2]!,
    obedience: flat[3]!,
    tacticalSkill: flat[4]!,
    ambition: flat[5]!,
    charisma: flat[6]!,
    flexibility: flat[7]!,
    patience: flat[8]!,
    ruthlessness: flat[9]!,
  };
}

function parseAssessment(flat: number[]): SituationAssessment {
  return {
    forceRatioPermille: flat[0] ?? 0,
    momentum: flat[1] ?? 0,
    flankLeft: flat[2] ?? 0,
    flankRight: flat[3] ?? 0,
    rearThreat: flat[4] ?? 0,
    reserveAvailable: flat[5] ?? 0,
    terrainAdvantage: flat[6] ?? 0,
    timePressure: flat[7] ?? 0,
  };
}

interface RawDecisionRecord {
  tick: number;
  chosen: string;
  score: number;
  candidates: [string, number][];
  breakdown: [string, number][];
}

function parseDecisionLog(json: string): DecisionRecord[] {
  try {
    const raw = JSON.parse(json) as RawDecisionRecord[];
    return raw.map((r) => ({
      tick: r.tick,
      chosen: r.chosen,
      score: r.score,
      candidates: r.candidates,
      breakdown: r.breakdown,
    }));
  } catch {
    return [];
  }
}

function parseKnownEnemies(flat: number[]): CommanderInfo["knownEnemies"] {
  const out: CommanderInfo["knownEnemies"] = [];
  for (let i = 0; i + 6 <= flat.length; i += 6) {
    out.push({
      node: flat[i]!,
      xM: flat[i + 1]! / 100,
      yM: flat[i + 2]! / 100,
      strength: flat[i + 3]!,
      confidence: flat[i + 4]!,
      observedTick: flat[i + 5]!,
    });
  }
  return out;
}

/** `queryCommander` への応答をまとめて組み立てる。ノードが存在しなければ null。 */
export function buildCommanderInfo(
  node: number,
  attrsFlat: number[],
  assessmentFlat: number[],
  decisionLogJson: string,
  blackboardFlat: number[],
): CommanderInfo | null {
  const attrs = parseCommanderAttrs(attrsFlat);
  if (!attrs) return null;
  return {
    node,
    attrs,
    perceived: parseAssessment(assessmentFlat.slice(0, 8)),
    actual: parseAssessment(assessmentFlat.slice(8, 16)),
    decisionLog: parseDecisionLog(decisionLogJson),
    knownEnemies: parseKnownEnemies(blackboardFlat),
  };
}
