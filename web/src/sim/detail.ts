/**
 * 兵士 1 体・指揮官 1 人ぶんの詳細情報を、`sim-wasm` のフラット配列 / JSON
 * 文字列レスポンスから構造化する（低頻度・クリック時呼び出し想定、M8）。
 *
 * レイアウトは `crates/sim-wasm/src/lib.rs` の対応する関数と一致させること。
 */

import type {
  CommanderAttrs,
  CommanderInfo,
  DecisionRecord,
  SituationAssessment,
  SoldierDebug,
  SoldierDetail,
} from "./protocol";

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

/**
 * `soldierDebugJson` の応答を構造化する。Rust 側は数値と null しか出さないので、
 * 形が違えば（古い wasm を読んでいる等）null にして UI 側で「-」を出す。
 */
export function parseSoldierDebug(json: string): SoldierDebug | null {
  let raw: unknown;
  try {
    raw = JSON.parse(json);
  } catch {
    return null;
  }
  if (raw === null || typeof raw !== "object") return null;
  const d = raw as Record<string, unknown>;
  if (typeof d.id !== "number" || typeof d.mission !== "string" || typeof d.action !== "string") {
    return null;
  }
  const num = (key: string): number => (typeof d[key] === "number" ? (d[key] as number) : 0);
  const maybeId = (key: string): number | null =>
    typeof d[key] === "number" ? (d[key] as number) : null;
  const actionScores = Array.isArray(d.actionScores)
    ? (d.actionScores as Record<string, unknown>[]).map((a) => ({
        action: typeof a.action === "string" ? a.action : "",
        score: typeof a.score === "number" ? a.score : 0,
      }))
    : [];
  const candidates = Array.isArray(d.candidates)
    ? (d.candidates as Record<string, unknown>[]).map((c) => ({
        target: typeof c.target === "number" ? c.target : -1,
        score: typeof c.score === "number" ? c.score : 0,
        distanceMm: typeof c.distanceMm === "number" ? c.distanceMm : 0,
        fighting: c.fighting === true,
        load: typeof c.load === "number" ? c.load : 0,
      }))
    : [];
  const rawAwareness = (d.awareness ?? {}) as Record<string, unknown>;
  const awarenessNum = (key: string): number =>
    typeof rawAwareness[key] === "number" ? (rawAwareness[key] as number) : 0;
  const awarenessId = (key: string): number | null =>
    typeof rawAwareness[key] === "number" ? (rawAwareness[key] as number) : null;
  const rawArea = d.area as Record<string, unknown> | null | undefined;
  const area =
    rawArea && typeof rawArea === "object"
      ? {
          centerXMm: typeof rawArea.centerXMm === "number" ? rawArea.centerXMm : 0,
          centerYMm: typeof rawArea.centerYMm === "number" ? rawArea.centerYMm : 0,
          radiusMm: typeof rawArea.radiusMm === "number" ? rawArea.radiusMm : 0,
          leashMm: typeof rawArea.leashMm === "number" ? rawArea.leashMm : 0,
        }
      : null;
  return {
    id: d.id,
    node: num("node"),
    mission: d.mission,
    action: d.action,
    focus: maybeId("focus"),
    orderedFocus: maybeId("orderedFocus"),
    slot: num("slot"),
    goalXMm: num("goalXMm"),
    goalYMm: num("goalYMm"),
    slotXMm: num("slotXMm"),
    slotYMm: num("slotYMm"),
    slotDistanceMm: num("slotDistanceMm"),
    reactionRadiusMm: num("reactionRadiusMm"),
    thinkInTicks: num("thinkInTicks"),
    formationSampleInTicks: num("formationSampleInTicks"),
    commitTicksLeft: num("commitTicksLeft"),
    missionState: typeof d.missionState === "string" ? d.missionState : "idle",
    missionSinceTick: num("missionSinceTick"),
    insideFriendly: num("insideFriendly"),
    insideEnemy: num("insideEnemy"),
    area,
    awareness: {
      updatedTick: awarenessNum("updatedTick"),
      enemies: awarenessNum("enemies"),
      allies: awarenessNum("allies"),
      threatFront: awarenessNum("threatFront"),
      threatFlank: awarenessNum("threatFlank"),
      threatRear: awarenessNum("threatRear"),
      crowding: awarenessNum("crowding"),
      localBroken: awarenessNum("localBroken"),
      nearestEnemy: awarenessId("nearestEnemy"),
      nearestEnemyMm: awarenessNum("nearestEnemyMm"),
      supportableEnemy: awarenessId("supportableEnemy"),
    },
    actionScores,
    chosenAction: typeof d.chosenAction === "string" ? d.chosenAction : null,
    decidedAtTick: maybeId("decidedAtTick"),
    candidates,
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
