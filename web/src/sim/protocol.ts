/**
 * メインスレッドと Sim Worker のメッセージ定義。
 *
 * 境界を跨ぐ呼び出しはフレームあたり定数回に抑える。
 * エンティティごとのメッセージは送らない（仕様 01 章 4 節）。
 */

export interface TerrainPayload {
  dim: number;
  cellM: number;
  sizeM: number;
  /** Int16Array（標高 cm）の中身 */
  height: ArrayBuffer;
  /** Uint8Array（Ground）の中身 */
  ground: ArrayBuffer;
  /** Uint8Array（Vegetation）の中身 */
  vegetation: ArrayBuffer;
  /** Uint8Array（Overlay）の中身 */
  overlay: ArrayBuffer;
  /** Uint16Array（水深 cm、0 = 陸地）の中身 */
  water: ArrayBuffer;
  /** Uint8Array（WaterKind）の中身 */
  waterKind: ArrayBuffer;
  /** Uint8Array（崖ビットマスク）の中身 */
  cliff: ArrayBuffer;
  /**
   * 会戦地候補。1 件あたり
   * [xM, yM, score, passablePermille, asymmetryPermille, opennessPermille, bottleneckCount]
   * の平坦な配列（sim-wasm の battleSites() と同じレイアウト）。
   */
  battleSites: number[];
}

/**
 * `init` の再現に必要な最小情報。録画・リプレイで使う。
 *
 * `scenario` が付いていれば、地形も両軍もそのプリセット（`sim_core::scenario`）
 * から決まるので、`seed`/`sizeM`/`relief` は無視される。プリセットを選んだ
 * 会戦のリプレイは、プリセットの index さえあれば再現できる。
 */
export interface InitConfig {
  seed: number;
  sizeM: number;
  relief: number;
  /** 会戦プリセットの index。省略時は従来の対称デモ配置。 */
  scenario?: number;
}

/** `scenarioListJson()` の 1 要素。`crates/sim-wasm` の `scenario_json` と一致させること。 */
export interface ScenarioContingentInfo {
  nameJa: string;
  commanderJa: string;
  archetype: string;
  count: number;
  troopType: number;
}

export interface ScenarioArmyInfo {
  faction: number;
  nameJa: string;
  commanderJa: string;
  archetype: string;
  /** 開始時の会戦プラン。指揮官 AI に任せる場合は "-"。 */
  battlePlan: string;
  soldiers: number;
  contingents: ScenarioContingentInfo[];
}

export interface ScenarioInfo {
  id: string;
  nameJa: string;
  nameEn: string;
  year: number;
  placeJa: string;
  summaryJa: string;
  historicalStrengthJa: string;
  scaleNoteJa: string;
  sizeM: number;
  relief: number;
  seedLo: number;
  seedHi: number;
  soldiers: number;
  armies: ScenarioArmyInfo[];
}

/**
 * 憑依 UI から発行できる命令。仕様 05 章 4 節 / 08 章 9 節「命令 UI 全種」。
 * 「条件付き」命令は仕様側にもまだ具体的な構造がなく、現行実装の対象外
 * （`web/src/ui/orders.ts` 冒頭のスコープ注記を参照）。
 */
export type OrderCommand =
  | {
      type: "issueMoveTo";
      node: number;
      xM: number;
      yM: number;
      facingBrad: number;
      formation: number;
    }
  | { type: "issueHold"; node: number; xM: number; yM: number; facingBrad: number; allowPursuit: boolean }
  | { type: "issueAttack"; node: number; targetNode: number; approach: number }
  | { type: "issueCharge"; node: number; targetNode: number }
  | { type: "issueFlank"; node: number; targetNode: number; sideRight: boolean }
  | { type: "issueWithdraw"; node: number; xM: number; yM: number; fighting: boolean }
  | { type: "issueShootAt"; node: number; targetNode: number; mode: number }
  | { type: "issueReserve"; node: number; xM: number; yM: number }
  | { type: "issuePursue"; node: number; targetNode: number; maxDistanceM: number }
  | { type: "issueHuntPerson"; node: number; targetSoldier: number }
  | { type: "issueOccupyArea"; node: number; xM: number; yM: number; radiusM: number }
  | {
      type: "issueGuardArea";
      node: number;
      xM: number;
      yM: number;
      radiusM: number;
      interceptRadiusM: number;
    }
  | {
      type: "queueBuildStructure";
      kind: number;
      axM: number;
      ayM: number;
      bxM: number;
      byM: number;
      owner: number;
      priority: number;
    };

/** 録画された 1 コマンド。`atTick` は発行時点の `world.tickCount()`。 */
export interface RecordedCommand {
  atTick: number;
  msg: OrderCommand;
}

/** 保存・読み込みできるリプレイの中身。 */
export interface Recording {
  init: InitConfig;
  log: RecordedCommand[];
  finalTick: number;
  finalHashLo: number;
  finalHashHi: number;
}

export type ToWorker =
  | { type: "init"; seed: number; sizeM: number; relief: number; scenario?: number }
  | {
      type: "deploy";
      xM: number;
      yM: number;
      files: number;
      ranks: number;
      spacingMm: number;
      faction: number;
      unitId: number;
      troopType: number;
      seedSalt: number;
    }
  | { type: "setFactionGoal"; faction: number; xM: number; yM: number }
  | {
      type: "addLineUnit";
      faction: number;
      firstId: number;
      count: number;
      ranks: number;
      formation: number;
    }
  | OrderCommand
  | { type: "setCommanderArchetype"; node: number; archetype: number; seedSalt: number }
  | { type: "setRunning"; running: boolean }
  | { type: "setSpeed"; speed: number }
  | { type: "recycleBuffer"; buffer: ArrayBuffer }
  | { type: "querySoldier"; id: number }
  | { type: "queryCommander"; node: number }
  | { type: "getRecording" }
  | { type: "loadReplay"; recording: Recording };

/**
 * M3/M4 の集計データ。毎ティックではなく間引いて送る（仕様 01 章 4 節：
 * 境界を跨ぐ呼び出しはフレームあたり定数回に抑える）。
 * レイアウトは `sim-wasm` の `combatStats`/`commandNodes`/`commandEvents`/
 * `messengers` と同じフラット配列。
 */
export interface StatsPayload {
  combat: number[];
  nodes: number[];
  commandEvents: number[];
  messengers: number[];
}

/** `soldierDetail` の 9 要素。`crates/sim-wasm/src/lib.rs::soldier_detail` と一致させること。 */
export interface SoldierDetail {
  hp: number;
  morale: number;
  fatigue: number;
  ammo: number;
  /** 交戦相手の兵士 ID。いなければ null。 */
  target: number | null;
  bravery: number;
  discipline: number;
  skill: number;
  weaponReachMm: number;
}

/**
 * `soldierDebugJson` が返す判断まわりの状態（B0 のデバッグ表示）。
 * `crates/sim-wasm/src/lib.rs::soldier_debug_json` と一致させること。
 */
export interface SoldierDebugCandidate {
  /** 候補になった敵の兵士 ID。 */
  target: number;
  score: number;
  distanceMm: number;
  /** その敵が既に交戦中か。 */
  fighting: boolean;
  /** 既にその敵へ向かっている味方の人数。 */
  load: number;
}

/** `soldierDebugJson` の `awareness`。`sim-core::perception::LocalAwareness` と一致させること。 */
export interface SoldierAwareness {
  /** 最後に周囲を見た tick。 */
  updatedTick: number;
  enemies: number;
  allies: number;
  threatFront: number;
  threatFlank: number;
  threatRear: number;
  /** 1.5 m 以内の味方（押し合いの元）。 */
  crowding: number;
  /** 近くで崩れている味方。 */
  localBroken: number;
  /** もっとも近い敵。見えていなければ null。 */
  nearestEnemy: number | null;
  nearestEnemyMm: number;
  /** 味方が組み合っていて援護に入れる相手。無ければ null。 */
  supportableEnemy: number | null;
}

/** `soldierDebugJson` の `actionScores` の 1 件。`soldier_ai::IndividualAction` の識別子と点数。 */
export interface SoldierActionScore {
  action: string;
  score: number;
}

/** 地点任務の範囲（`soldierDebugJson` の `area`）。 */
export interface SoldierMissionArea {
  centerXMm: number;
  centerYMm: number;
  radiusMm: number;
  /** 地点防衛の追撃限界。占拠任務では 0。 */
  leashMm: number;
}

export interface SoldierDebug {
  id: number;
  /** 所属する葉部隊の指揮ノード。無所属なら -1。 */
  node: number;
  /** 受けている任務（`metrics::MissionKind` の識別子）。 */
  mission: string;
  /** 現在の局所行動（`follow_order` / `join_fight`）。 */
  action: string;
  /** 個人判断で向かっている敵。いなければ null。 */
  focus: number | null;
  /** 任務が名指しした対象（人物追跡）。 */
  orderedFocus: number | null;
  /** 隊形スロットの番号。 */
  slot: number;
  goalXMm: number;
  goalYMm: number;
  slotXMm: number;
  slotYMm: number;
  slotDistanceMm: number;
  reactionRadiusMm: number;
  /** 次に局所判断をするまでの tick。予定が無ければ -1。 */
  thinkInTicks: number;
  /** 次に陣形目標を取り込むまでの tick（＝命令への反応待ち）。 */
  formationSampleInTicks: number;
  /** 現在の行動を続けると決めている残り tick。 */
  commitTicksLeft: number;
  /** 任務の段階（`organization::MissionState` の識別子、B5）。 */
  missionState: string;
  /** その段階に入った tick。 */
  missionSinceTick: number;
  /** 地点任務で範囲の中にいる味方・敵の人数。 */
  insideFriendly: number;
  insideEnemy: number;
  /** 地点任務の範囲。地点任務でなければ null。 */
  area: SoldierMissionArea | null;
  /** この兵士がいま把握している周囲（B1 の局所知覚）。 */
  awareness: SoldierAwareness;
  /** 選べた行動とその点数（B2）。点数の高い順ではなく列挙順で並ぶ。 */
  actionScores: SoldierActionScore[];
  /** 直近の判断で選んだ行動。まだ判断していなければ null。 */
  chosenAction: string | null;
  /** 候補と点数を記録した tick。まだ判断していなければ null。 */
  decidedAtTick: number | null;
  /** 認識していた敵と、その点数（高い順）。 */
  candidates: SoldierDebugCandidate[];
}

/** `commanderAttrs` の 10 要素。`organization::CommanderAttrs` と一致させること。 */
export interface CommanderAttrs {
  boldness: number;
  caution: number;
  initiative: number;
  obedience: number;
  tacticalSkill: number;
  ambition: number;
  charisma: number;
  flexibility: number;
  patience: number;
  ruthlessness: number;
}

/** `commanderAssessment` の 8 要素 1 組ぶん。 */
export interface SituationAssessment {
  forceRatioPermille: number;
  momentum: number;
  flankLeft: number;
  flankRight: number;
  rearThreat: number;
  reserveAvailable: number;
  terrainAdvantage: number;
  timePressure: number;
}

export interface DecisionRecord {
  tick: number;
  chosen: string;
  score: number;
  candidates: [string, number][];
  breakdown: [string, number][];
}

export interface CommanderInfo {
  node: number;
  attrs: CommanderAttrs;
  perceived: SituationAssessment;
  actual: SituationAssessment;
  decisionLog: DecisionRecord[];
  /** Blackboard が知っている敵部隊（憑依中の視界制限表示用）。 */
  knownEnemies: { node: number; xM: number; yM: number; strength: number; confidence: number; observedTick: number }[];
}

export type FromWorker =
  | {
      type: "ready";
      /** `init`: 起動直後（デモ・シナリオを配置してよい）。`replay`: `loadReplay`
       * による World 再構築（記録済みの命令で再現するので、ここで新たに部隊を
       * 配置してはいけない）。 */
      reason: "init" | "replay";
      simVersion: number;
      snapshotVersion: number;
      soldierStride: number;
      terrain: TerrainPayload;
      /** 会戦プリセットから作られたなら、その index。両軍はワーカー側で
       * 配置済みなので、メインスレッドはデモ配置を送ってはいけない。 */
      scenario?: number;
      /** 選択できる会戦プリセットの一覧（`init` のときだけ付く）。 */
      scenarios?: ScenarioInfo[];
    }
  | {
      type: "snapshot";
      tick: number;
      count: number;
      buffer: ArrayBuffer;
      stats?: StatsPayload;
    }
  | {
      type: "soldierInfo";
      id: number;
      node: number;
      detail: SoldierDetail | null;
      debug: SoldierDebug | null;
    }
  | { type: "commanderInfo"; info: CommanderInfo }
  | { type: "recording"; recording: Recording }
  | {
      type: "replayResult";
      matched: boolean;
      atTick: number;
      hashLo: number;
      hashHi: number;
      expectedHashLo: number;
      expectedHashHi: number;
    };
