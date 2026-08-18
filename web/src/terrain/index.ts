/**
 * 地形生成器の入口。
 *
 * 生成は 3 段構えになっている。
 *
 * 1. `generateFields` が場（標高・水系・地質・植生）を作る
 * 2. `layRoads` と `applyStamps` が人工物とシナリオ固有の地勢を焼き込む
 * 3. `evaluateBattleSites` が展開に向く場所を評価する
 *
 * 出力は整数グリッドだけで、シミュレーション本体（Rust）はこれを受け取って
 * 動く。生成器そのものはシミュレーションの一部ではない。
 */

import { computeCliffs, generateFields, recomputeDerived } from "./generate";
import { evaluateBattleSites } from "./battle-site";
import { layRoads } from "./roads";
import { applyStamps, type Stamp } from "./shaping";
import type { TerrainGrids, TerrainParams } from "./types";
import type { ScenarioTerrain } from "./scenarios";

export * from "./types";
export * from "./effects";
export { computeCliffs, generateFields, recomputeDerived } from "./generate";
export { encodeTerrain, decodeTerrain, VERSION as TERRAIN_BINARY_VERSION } from "./serialize";
export { applyStamps, type Stamp } from "./shaping";
export { layRoads } from "./roads";
export { evaluateBattleSites } from "./battle-site";
export { SCENARIO_TERRAINS, scenarioTerrainById, scenarioTerrainByIndex } from "./scenarios";
export type { ScenarioTerrain } from "./scenarios";

/** 返す会戦地候補の件数。 */
const BATTLE_SITE_TOP_N = 8;

export function generateTerrain(
  params: Partial<TerrainParams>,
  stamps: readonly Stamp[] = [],
): TerrainGrids {
  const fields = generateFields(params);
  layRoads(fields, params.roadCount ?? 0);
  applyStamps(fields, stamps);
  // 整形は道路の下の地質も書き換えるので、傾斜と崖を取り直してから評価する
  recomputeDerived(fields);

  return {
    dim: fields.dim,
    cellM: fields.cellM,
    seed: fields.seed,
    height: fields.height,
    ground: fields.ground,
    vegetation: fields.vegetation,
    overlay: fields.overlay,
    water: fields.water,
    waterKind: fields.waterKind,
    moisture: fields.moisture,
    cliff: fields.cliff,
    battleSites: evaluateBattleSites(fields, BATTLE_SITE_TOP_N),
  };
}

/** シナリオの地形を生成する。 */
export function generateScenarioTerrain(scenario: ScenarioTerrain): TerrainGrids {
  return generateTerrain(scenario.params, scenario.stamps);
}

/** バイナリから復元するときの崖の取り直し。 */
export function cliffsFromHeight(height: Int16Array, dim: number): Uint8Array {
  return computeCliffs(height, dim);
}
