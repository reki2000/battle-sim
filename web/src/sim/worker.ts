/**
 * Sim Worker。
 *
 * シミュレーションはこのワーカーの中だけで回る。メインスレッドは
 * 描画と入力に専念する（仕様 01 章 3 節）。
 *
 * M0 ではスナップショットを ArrayBuffer にコピーして transfer する。
 * COOP/COEP を配信できる環境では M9 で SharedArrayBuffer に切り替え、
 * コピーをなくす。
 */

import init, { World } from "../wasm/sim.js";
import type { ToWorker, FromWorker } from "./protocol";
import { SOLDIER_STRIDE } from "./snapshot";

let world: World | null = null;
let memory: WebAssembly.Memory | null = null;
let running = false;
let speed = 1;
let accumulatorMs = 0;
let lastRealMs = 0;

/** シミュレーションの刻み（ms）。sim-math の TICK_MS と一致させること。 */
const TICK_MS = 50;
/** 1 フレームで消化するティック数の上限。スパイラルを防ぐ。 */
const MAX_TICKS_PER_FRAME = 64;

/** transfer 用のバッファを 2 枚で使い回す。 */
const bufferPool: ArrayBuffer[] = [];

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

function publishSnapshot(): void {
  if (!world || !memory) return;
  world.writeSnapshot();
  const ptr = world.soldiersPtr();
  const len = world.soldiersByteLen();

  const src = new Uint8Array(memory.buffer, ptr, len);
  const buf = takeBuffer(len);
  new Uint8Array(buf).set(src);

  post(
    {
      type: "snapshot",
      tick: world.tickCount(),
      count: (len / SOLDIER_STRIDE) | 0,
      buffer: buf,
    },
    [buf],
  );
}

function loop(): void {
  if (!world) return;

  const now = performance.now();
  if (running) {
    const elapsed = Math.min(250, now - lastRealMs);
    accumulatorMs += elapsed * speed;
    let n = 0;
    while (accumulatorMs >= TICK_MS && n < MAX_TICKS_PER_FRAME) {
      world.tick();
      accumulatorMs -= TICK_MS;
      n++;
    }
    if (n === MAX_TICKS_PER_FRAME) {
      // 追いつけなかったぶんは捨てる（時間が伸びるより遅くなる方がまし）
      accumulatorMs = 0;
    }
  }
  lastRealMs = now;

  publishSnapshot();
  // シミュレーションは 20 Hz なので、描画の 60 fps より遅く回してよい
  setTimeout(loop, TICK_MS / 2);
}

self.onmessage = async (ev: MessageEvent<ToWorker>) => {
  const msg = ev.data;

  switch (msg.type) {
    case "init": {
      const wasm = await init();
      memory = wasm.memory;
      world = new World(
        msg.seed >>> 0,
        Math.floor(msg.seed / 2 ** 32) >>> 0,
        msg.sizeM,
        msg.relief,
      );

      // 地形グリッドは 1 度だけ渡す。以降は工兵の作業で
      // 変更されたチャンクだけを差分で送る（M6）。
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
          simVersion: World.simVersion(),
          snapshotVersion: World.snapshotVersion(),
          soldierStride: World.soldierStride(),
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

      lastRealMs = performance.now();
      loop();
      break;
    }

    case "deploy": {
      world?.deployBlock(
        msg.xM,
        msg.yM,
        msg.files,
        msg.ranks,
        msg.spacingMm,
        msg.faction,
        msg.unitId,
        msg.seedSalt,
      );
      break;
    }

    case "setFactionGoal": {
      world?.setFactionGoal(msg.faction, msg.xM, msg.yM);
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
  }
};
