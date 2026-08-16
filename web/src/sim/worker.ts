/**
 * Sim Worker。
 *
 * シミュレーションはこのワーカーの中だけで回る。メインスレッドは
 * 描画と入力に専念する（仕様 01 章 3 節）。
 *
 * M0 ではスナップショットを ArrayBuffer にコピーして transfer する。
 * COOP/COEP を配信できる環境では M9 で SharedArrayBuffer に切り替え、
 * コピーをなくす。
 *
 * M8: 憑依 UI から発行された命令は `recordingLog` に記録し、
 * `getRecording` で丸ごと取り出せる（リプレイの保存）。`loadReplay` で
 * 読み込んだ記録は、記録時と同じ tick で同じ命令を再適用することで
 * 完全に再現する（`atTick` は命令適用時点の `world.tickCount()`。ワーカーの
 * イベントループはティックの合間にしかメッセージを処理しないため、
 * この tick 値だけで十分に一意な再生ポイントになる）。リプレイ再生中に
 * ライブの命令が来たら、そこから先の記録済みキューを捨てて新しい命令を
 * 記録し直す（＝分岐）。
 */

import init, { World } from "../wasm/sim.js";
import type {
  CommanderInfo,
  FromWorker,
  InitConfig,
  OrderCommand,
  Recording,
  ScenarioInfo,
  StatsPayload,
  ToWorker,
} from "./protocol";
import { buildCommanderInfo, parseSoldierDetail } from "./detail";
import { loadCachedTerrain, saveCachedTerrain, terrainCacheKey } from "./terrain-cache";

let world: World | null = null;
let memory: WebAssembly.Memory | null = null;
let running = false;
/** `loop()` が既に回っているか。会戦を選び直しても 2 本目を始めない。 */
let looping = false;
let speed = 1;
let accumulatorMs = 0;
let lastRealMs = 0;
let soldierStride = 16;

/** シミュレーションの刻み（ms）。sim-math の TICK_MS と一致させること。 */
const TICK_MS = 50;
/** 1 フレームで消化するティック数の上限。スパイラルを防ぐ。 */
const MAX_TICKS_PER_FRAME = 64;

/** transfer 用のバッファを 2 枚で使い回す。 */
const bufferPool: ArrayBuffer[] = [];

// ── M8: 命令の録画・リプレイ ─────────────────────────────
let recordedInit: InitConfig | null = null;
let recordingLog: { atTick: number; msg: OrderCommand }[] = [];
let replayQueue: { atTick: number; msg: OrderCommand }[] = [];
let replayTargetTick = -1;
let replayExpected: { lo: number; hi: number } | null = null;

function post(msg: FromWorker, transfer: Transferable[] = []): void {
  (self as unknown as Worker).postMessage(msg, transfer);
}

function takeBuffer(byteLen: number): ArrayBuffer {
  for (let i = 0; i < bufferPool.length; i++) {
    if (bufferPool[i]!.byteLength === byteLen) {
      return bufferPool.splice(i, 1)[0]!;
    }
  }
  return new ArrayBuffer(byteLen);
}

/** 統計は毎スナップショットではなく、この tick 数おきにだけ送る。 */
const STATS_INTERVAL_TICKS = 10;
let lastStatsTick = -STATS_INTERVAL_TICKS;

function publishSnapshot(): void {
  if (!world || !memory) return;
  world.writeSnapshot();
  const ptr = world.soldiersPtr();
  const len = world.soldiersByteLen();

  const src = new Uint8Array(memory.buffer, ptr, len);
  const buf = takeBuffer(len);
  new Uint8Array(buf).set(src);

  const tick = world.tickCount();
  let stats: StatsPayload | undefined;
  if (tick - lastStatsTick >= STATS_INTERVAL_TICKS) {
    lastStatsTick = tick;
    stats = {
      combat: Array.from(world.combatStats()),
      nodes: Array.from(world.commandNodes()),
      commandEvents: Array.from(world.commandEvents(64)),
      messengers: Array.from(world.messengers()),
    };
  }

  post(
    stats
      ? { type: "snapshot", tick, count: (len / soldierStride) | 0, buffer: buf, stats }
      : { type: "snapshot", tick, count: (len / soldierStride) | 0, buffer: buf },
    [buf],
  );
}

/** リプレイキューのうち、現在の tick までに発行すべき命令を再適用する。 */
function drainReplayQueue(): void {
  if (!world) return;
  while (replayQueue.length > 0 && replayQueue[0]!.atTick <= world.tickCount()) {
    const entry = replayQueue.shift()!;
    applyCommand(entry.msg, false);
  }
}

function finishReplay(): void {
  if (!world || !replayExpected) return;
  const hashLo = world.stateHashLo();
  const hashHi = world.stateHashHi();
  const matched = hashLo === replayExpected.lo && hashHi === replayExpected.hi;
  post({
    type: "replayResult",
    matched,
    atTick: world.tickCount(),
    hashLo,
    hashHi,
    expectedHashLo: replayExpected.lo,
    expectedHashHi: replayExpected.hi,
  });
  replayTargetTick = -1;
  replayExpected = null;
}

function loop(): void {
  if (!world) return;

  const now = performance.now();
  if (running) {
    const elapsed = Math.min(250, now - lastRealMs);
    accumulatorMs += elapsed * speed;
    let due = Math.min(Math.floor(accumulatorMs / TICK_MS), MAX_TICKS_PER_FRAME);
    // tickMany は複数 tick をまとめて進めるため、リプレイの目標 tick を
    // 一気に飛び越えてしまうと、そこで比較するはずの state_hash がずれた
    // 時点のものになる。目標 tick をちょうど跨がないよう上限を切る。
    if (replayTargetTick >= 0) {
      due = Math.min(due, Math.max(0, replayTargetTick - world.tickCount()));
    }
    if (due > 0) {
      if (replayQueue.length === 0) {
        // リプレイのコマンドを tick 単位で差し込む必要が無ければ、
        // 境界を跨ぐ呼び出し回数を減らすため tickMany でまとめて進める
        // （M9: 早送り時の JS/wasm 境界オーバーヘッド削減）。
        world.tickMany(due);
        if (
          replayTargetTick >= 0 &&
          world.tickCount() >= replayTargetTick &&
          replayQueue.length === 0
        ) {
          finishReplay();
        }
      } else {
        for (let n = 0; n < due; n++) {
          drainReplayQueue();
          world.tick();
          if (
            replayTargetTick >= 0 &&
            world.tickCount() >= replayTargetTick &&
            replayQueue.length === 0
          ) {
            finishReplay();
            break;
          }
        }
      }
      accumulatorMs -= due * TICK_MS;
    }
    if (due === MAX_TICKS_PER_FRAME) {
      // 追いつけなかったぶんは捨てる(時間が伸びるより遅くなる方がまし)
      accumulatorMs = 0;
    }
  }
  lastRealMs = now;

  publishSnapshot();
  // シミュレーションは 20 Hz なので、描画の 60 fps より遅く回してよい
  setTimeout(loop, TICK_MS / 2);
}

/** `deploy`/`setFactionGoal`/`addLineUnit`/命令/`setCommanderArchetype` を World に適用する。
 * ライブ入力とリプレイ再生の両方がこの一本の経路を通ることで、両者の挙動を一致させる。
 */
function applyCommand(
  msg: Extract<ToWorker, { type: string }>,
  record: boolean,
): void {
  if (!world) return;
  switch (msg.type) {
    case "deploy":
      world.deployBlock(
        msg.xM,
        msg.yM,
        msg.files,
        msg.ranks,
        msg.spacingMm,
        msg.faction,
        msg.unitId,
        msg.troopType,
        msg.seedSalt,
      );
      break;
    case "setFactionGoal":
      world.setFactionGoal(msg.faction, msg.xM, msg.yM);
      break;
    case "addLineUnit":
      world.addLineUnit(msg.faction, msg.firstId, msg.count, msg.ranks, msg.formation);
      break;
    case "issueMoveTo":
      world.issueMoveTo(msg.node, msg.xM, msg.yM, msg.facingBrad, msg.formation);
      break;
    case "issueHold":
      world.issueHold(msg.node, msg.xM, msg.yM, msg.facingBrad, msg.allowPursuit);
      break;
    case "issueAttack":
      world.issueAttack(msg.node, msg.targetNode, msg.approach);
      break;
    case "issueCharge":
      world.issueCharge(msg.node, msg.targetNode);
      break;
    case "issueFlank":
      world.issueFlank(msg.node, msg.targetNode, msg.sideRight);
      break;
    case "issueWithdraw":
      world.issueWithdraw(msg.node, msg.xM, msg.yM, msg.fighting);
      break;
    case "issueShootAt":
      world.issueShootAt(msg.node, msg.targetNode, msg.mode);
      break;
    case "issueReserve":
      world.issueReserve(msg.node, msg.xM, msg.yM);
      break;
    case "issuePursue":
      world.issuePursue(msg.node, msg.targetNode, msg.maxDistanceM);
      break;
    case "queueBuildStructure":
      world.queueBuildStructure(
        msg.kind,
        msg.axM,
        msg.ayM,
        msg.bxM,
        msg.byM,
        msg.owner,
        msg.priority,
      );
      break;
    case "setCommanderArchetype":
      world.setCommanderArchetype(msg.node, msg.archetype, msg.seedSalt);
      break;
    default:
      return;
  }
  if (record) {
    recordingLog.push({ atTick: world.tickCount(), msg: msg as OrderCommand });
  }
}

/** 現在のリプレイを打ち切り、続きをライブ入力として録画していく（＝分岐）。 */
function branchFromReplayIfNeeded(): void {
  if (replayQueue.length === 0 && replayTargetTick < 0) return;
  const now = world?.tickCount() ?? 0;
  recordingLog = recordingLog.filter((e) => e.atTick <= now);
  replayQueue = [];
  replayTargetTick = -1;
  replayExpected = null;
}

const MUTATING_TYPES = new Set<ToWorker["type"]>([
  "deploy",
  "setFactionGoal",
  "addLineUnit",
  "issueMoveTo",
  "issueHold",
  "issueAttack",
  "issueCharge",
  "issueFlank",
  "issueWithdraw",
  "issueShootAt",
  "issueReserve",
  "issuePursue",
  "queueBuildStructure",
  "setCommanderArchetype",
]);

/** `init` 直後・`loadReplay` での World 再構築後、共通の地形ペイロードを組み立てて送る。 */
function postReady(reason: "init" | "replay", scenarios?: ScenarioInfo[]): void {
  if (!world || !memory) return;
  soldierStride = World.soldierStride();

  const dim = world.terrainDim();
  const cells = dim * dim;
  const surface = new Uint8Array(
    new Uint8Array(memory.buffer, world.terrainSurfacePtr(), cells),
  );
  const height = new Int16Array(
    new Int16Array(memory.buffer, world.terrainHeightPtr(), cells),
  );
  const water = new Uint8Array(
    new Uint8Array(memory.buffer, world.terrainWaterPtr(), cells),
  );
  const waterKind = new Uint8Array(
    new Uint8Array(memory.buffer, world.terrainWaterKindPtr(), cells),
  );
  const cliff = new Uint8Array(
    new Uint8Array(memory.buffer, world.terrainCliffPtr(), cells),
  );
  const battleSites = Array.from(world.battleSites());

  post(
    {
      type: "ready",
      reason,
      simVersion: World.simVersion(),
      snapshotVersion: World.snapshotVersion(),
      soldierStride,
      ...(recordedInit?.scenario !== undefined ? { scenario: recordedInit.scenario } : {}),
      ...(scenarios ? { scenarios } : {}),
      terrain: {
        dim,
        cellM: world.terrainCellM(),
        sizeM: world.terrainSizeM(),
        surface: surface.buffer,
        height: height.buffer,
        water: water.buffer,
        waterKind: waterKind.buffer,
        cliff: cliff.buffer,
        battleSites,
      },
    },
    [surface.buffer, height.buffer, water.buffer, waterKind.buffer, cliff.buffer],
  );
}

/**
 * 地形キャッシュを引きつつ World を作る（M9）。キャッシュがあれば
 * `fromCachedTerrain` で再生成をスキップし、無ければ通常どおり生成してから
 * 次回のために非同期で保存する（起動をブロックしない・失敗は握りつぶす）。
 * `init`・`loadReplay` の両方から使う。
 */
async function createWorld(config: InitConfig): Promise<World> {
  const { seed, sizeM, relief, scenario } = config;
  const seedLo = seed >>> 0;
  const seedHi = Math.floor(seed / 2 ** 32) >>> 0;
  // 会戦プリセットの地形は生成後に整形される（森の縁・泥濘・緩斜面）ので、
  // 同じ (seed, sizeM, relief) でも中身が違う。キャッシュキーを分ける。
  const cacheKey = terrainCacheKey(seed, sizeM, relief, scenario);
  const cached = await loadCachedTerrain(cacheKey);
  if (cached) {
    // 同じグリッドから作るので、以後の挙動は再生成した場合と完全に一致する
    // （sim-wasm の from_cached_terrain_matches_a_freshly_generated_world /
    // scenario_from_cached_terrain_matches_a_freshly_built_one で検証済み）。
    const restored =
      scenario === undefined
        ? World.fromCachedTerrain(
            seedLo,
            seedHi,
            cached.dim,
            cached.cellM,
            cached.height,
            cached.surface,
            cached.passability,
            cached.water,
            cached.waterKind,
            cached.cliff,
            Int32Array.from(cached.battleSitesFlat),
          )
        : World.fromScenarioCachedTerrain(
            scenario,
            cached.dim,
            cached.cellM,
            cached.height,
            cached.surface,
            cached.passability,
            cached.water,
            cached.waterKind,
            cached.cliff,
            Int32Array.from(cached.battleSitesFlat),
          );
    if (restored) return restored;
  }
  // プリセットの index が未知なら（記録が新しい版で作られた等）、
  // 従来の手続き生成へ落として起動だけは通す。
  const w =
    scenario === undefined
      ? new World(seedLo, seedHi, sizeM, relief)
      : (World.fromScenario(scenario) ?? new World(seedLo, seedHi, sizeM, relief));
  const dim = w.terrainDim();
  const cells = dim * dim;
  const mem = memory!;
  void saveCachedTerrain(cacheKey, {
    dim,
    cellM: w.terrainCellM(),
    height: new Int16Array(new Int16Array(mem.buffer, w.terrainHeightPtr(), cells)),
    surface: new Uint8Array(new Uint8Array(mem.buffer, w.terrainSurfacePtr(), cells)),
    passability: new Uint8Array(new Uint8Array(mem.buffer, w.terrainPassabilityPtr(), cells)),
    water: new Uint8Array(new Uint8Array(mem.buffer, w.terrainWaterPtr(), cells)),
    waterKind: new Uint8Array(new Uint8Array(mem.buffer, w.terrainWaterKindPtr(), cells)),
    cliff: new Uint8Array(new Uint8Array(mem.buffer, w.terrainCliffPtr(), cells)),
    battleSitesFlat: Array.from(w.battleSites()),
  });
  return w;
}

self.onmessage = async (ev: MessageEvent<ToWorker>) => {
  const msg = ev.data;

  if (MUTATING_TYPES.has(msg.type)) {
    branchFromReplayIfNeeded();
    applyCommand(msg, true);
    return;
  }

  switch (msg.type) {
    case "init": {
      if (!memory) {
        const wasm = await init();
        memory = wasm.memory;
      }
      recordedInit = {
        seed: msg.seed,
        sizeM: msg.sizeM,
        relief: msg.relief,
        ...(msg.scenario !== undefined ? { scenario: msg.scenario } : {}),
      };
      world = await createWorld(recordedInit);
      recordingLog = [];
      replayQueue = [];
      replayTargetTick = -1;
      replayExpected = null;

      lastStatsTick = -STATS_INTERVAL_TICKS;
      postReady("init", JSON.parse(World.scenarioListJson()) as ScenarioInfo[]);

      lastRealMs = performance.now();
      // `init` は会戦を選び直すたびに来る。ループは 1 本だけ回す。
      if (!looping) {
        looping = true;
        loop();
      }
      break;
    }

    case "setRunning": {
      running = msg.running;
      lastRealMs = performance.now();
      accumulatorMs = 0;
      break;
    }

    case "setSpeed": {
      speed = msg.speed;
      break;
    }

    case "recycleBuffer": {
      if (bufferPool.length < 3) bufferPool.push(msg.buffer);
      break;
    }

    case "querySoldier": {
      if (!world) break;
      const node = world.nodeForSoldier(msg.id);
      const flat = Array.from(world.soldierDetail(msg.id));
      post({ type: "soldierInfo", id: msg.id, node, detail: parseSoldierDetail(flat) });
      break;
    }

    case "queryCommander": {
      if (!world) break;
      const attrsFlat = Array.from(world.commanderAttrs(msg.node));
      const assessmentFlat = Array.from(world.commanderAssessment(msg.node));
      const decisionLogJson = world.commanderDecisionLogJson(msg.node);
      const blackboardFlat = Array.from(world.blackboardEnemyForces(msg.node));
      const info = buildCommanderInfo(
        msg.node,
        attrsFlat,
        assessmentFlat,
        decisionLogJson,
        blackboardFlat,
      );
      if (info) post({ type: "commanderInfo", info: info as CommanderInfo });
      break;
    }

    case "getRecording": {
      if (!world || !recordedInit) break;
      const recording: Recording = {
        init: recordedInit,
        log: recordingLog.map((e) => ({ atTick: e.atTick, msg: e.msg })),
        finalTick: world.tickCount(),
        finalHashLo: world.stateHashLo(),
        finalHashHi: world.stateHashHi(),
      };
      post({ type: "recording", recording });
      break;
    }

    case "loadReplay": {
      const rec = msg.recording;
      recordedInit = rec.init;
      world = await createWorld(rec.init);
      recordingLog = [];
      replayQueue = rec.log.map((e) => ({ atTick: e.atTick, msg: e.msg }));
      replayTargetTick = rec.finalTick;
      replayExpected = { lo: rec.finalHashLo, hi: rec.finalHashHi };
      lastStatsTick = -STATS_INTERVAL_TICKS;

      postReady("replay");

      running = true;
      accumulatorMs = 0;
      lastRealMs = performance.now();
      break;
    }
  }
};
