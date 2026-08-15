/**
 * エントリポイント。
 *
 * M0 の目的は「Rust → wasm → Worker → 描画」の経路が通っていることを
 * 目で確認できるようにすること。実装は Canvas2D の最小構成で、
 * M1（地形の WebGL2 描画）と M2（兵士のインスタンシング）で置き換わる。
 */

import { Camera } from "./render/iso";
import { TerrainRenderer } from "./render/terrain";
import { SoldierRenderer } from "./render/soldiers";
import { InterpolatedPositions, SnapshotView } from "./sim/snapshot";
import type { FromWorker, ToWorker } from "./sim/protocol";

const TICK_MS = 50;

const canvas = document.getElementById("view") as HTMLCanvasElement;
const ctx = canvas.getContext("2d", { alpha: false })!;
const hud = document.getElementById("hud") as HTMLDivElement;

const cam = new Camera();
const terrainRenderer = new TerrainRenderer();
const soldierRenderer = new SoldierRenderer();
const snapshot = new SnapshotView();
const interp = new InterpolatedPositions();

let lastSnapshotAt = performance.now();
let simTick = 0;
let soldierCount = 0;
let running = false;
let speed = 1;
let fps = 0;
let fpsAccum = 0;
let fpsFrames = 0;
let fpsLast = performance.now();
/** 描画中に読み続けているスナップショットバッファ。次が来たら返す。 */
let heldBuffer: ArrayBuffer | null = null;

const worker = new Worker(new URL("./sim/worker.ts", import.meta.url), {
  type: "module",
});

function send(msg: ToWorker, transfer: Transferable[] = []): void {
  worker.postMessage(msg, transfer);
}

// ── ワーカーからのメッセージ ────────────────────────────

worker.onmessage = (ev: MessageEvent<FromWorker>) => {
  const msg = ev.data;

  if (msg.type === "ready") {
    const t = msg.terrain;
    terrainRenderer.setTerrain({
      dim: t.dim,
      cellM: t.cellM,
      sizeM: t.sizeM,
      surface: new Uint8Array(t.surface),
      height: new Int16Array(t.height),
    });

    cam.worldSizeM = t.sizeM;
    cam.centerX = t.sizeM / 2;
    cam.centerY = t.sizeM / 2;
    cam.setViewWidthM(600);

    // 両軍を向かい合わせに配置し、互いに向かって進ませる
    const mid = Math.floor(t.sizeM / 2);
    send({
      type: "deploy",
      xM: mid - 16,
      yM: mid - 60,
      files: 40,
      ranks: 25,
      spacingMm: 800,
      faction: 0,
      unitId: 0,
      seedSalt: 1,
    });
    send({
      type: "deploy",
      xM: mid - 16,
      yM: mid + 60,
      files: 40,
      ranks: 25,
      spacingMm: 800,
      faction: 1,
      unitId: 1,
      seedSalt: 2,
    });
    send({ type: "setFactionGoal", faction: 0, xM: mid, yM: mid + 40 });
    send({ type: "setFactionGoal", faction: 1, xM: mid, yM: mid - 40 });

    setRunning(true);
    return;
  }

  if (msg.type === "snapshot") {
    // 直前のバッファは描画に使い終わっているのでワーカーへ返す。
    // 今届いたぶんは次のフレームまで読み続けるので、まだ transfer できない
    // （transfer するとその場で detach され、描画中に読めなくなる）。
    if (heldBuffer) {
      send({ type: "recycleBuffer", buffer: heldBuffer }, [heldBuffer]);
    }
    heldBuffer = msg.buffer;

    snapshot.bind(msg.buffer);
    interp.push(snapshot);
    simTick = msg.tick;
    soldierCount = msg.count;
    lastSnapshotAt = performance.now();
  }
};

// ── 入力 ────────────────────────────────────────────────

let dragging = false;
let lastPointer = { x: 0, y: 0 };

canvas.addEventListener("pointerdown", (e) => {
  dragging = true;
  lastPointer = { x: e.clientX, y: e.clientY };
  canvas.setPointerCapture(e.pointerId);
});

canvas.addEventListener("pointerup", (e) => {
  dragging = false;
  canvas.releasePointerCapture(e.pointerId);
});

canvas.addEventListener("pointermove", (e) => {
  if (!dragging) return;
  cam.panByScreen(e.clientX - lastPointer.x, e.clientY - lastPointer.y);
  lastPointer = { x: e.clientX, y: e.clientY };
});

canvas.addEventListener(
  "wheel",
  (e) => {
    e.preventDefault();
    const rect = canvas.getBoundingClientRect();
    // 対数ズーム。1 ノッチで約 1.15 倍（仕様 08 章 2.1）
    const step = -Math.sign(e.deltaY) * Math.log2(1.15);
    cam.zoomAt(e.clientX - rect.left, e.clientY - rect.top, step);
  },
  { passive: false },
);

window.addEventListener("keydown", (e) => {
  switch (e.key) {
    case " ":
      e.preventDefault();
      setRunning(!running);
      break;
    case "1":
      setSpeed(1);
      break;
    case "2":
      setSpeed(2);
      break;
    case "3":
      setSpeed(4);
      break;
    case "4":
      setSpeed(8);
      break;
    case "5":
      setSpeed(16);
      break;
    // 視野幅のプリセット。5 km 〜 10 m の全域を確認するため
    case "q":
      cam.setViewWidthM(5000);
      break;
    case "w":
      cam.setViewWidthM(1000);
      break;
    case "e":
      cam.setViewWidthM(200);
      break;
    case "r":
      cam.setViewWidthM(40);
      break;
    case "t":
      cam.setViewWidthM(10);
      break;
  }
});

function setRunning(v: boolean): void {
  running = v;
  send({ type: "setRunning", running: v });
}

function setSpeed(v: number): void {
  speed = v;
  send({ type: "setSpeed", speed: v });
}

// ── 描画ループ ──────────────────────────────────────────

function resize(): void {
  const dpr = Math.min(2, window.devicePixelRatio || 1);
  canvas.width = Math.floor(window.innerWidth * dpr);
  canvas.height = Math.floor(window.innerHeight * dpr);
  canvas.style.width = `${window.innerWidth}px`;
  canvas.style.height = `${window.innerHeight}px`;
  cam.viewW = canvas.width;
  cam.viewH = canvas.height;
  cam.clampZoom();
}

window.addEventListener("resize", resize);
resize();

function frame(now: number): void {
  // FPS
  fpsAccum += now - fpsLast;
  fpsLast = now;
  fpsFrames++;
  if (fpsAccum >= 500) {
    fps = (fpsFrames * 1000) / fpsAccum;
    fpsAccum = 0;
    fpsFrames = 0;
  }

  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.fillStyle = "#0d1014";
  ctx.fillRect(0, 0, canvas.width, canvas.height);

  terrainRenderer.draw(ctx, cam);

  // 20 Hz のスナップショット間を補間する。倍速時は補間しない
  const alpha =
    speed >= 8 ? 1 : Math.min(1, (now - lastSnapshotAt) / (TICK_MS / speed));
  soldierRenderer.draw(ctx, cam, snapshot, interp, alpha, (x, y) =>
    terrainRenderer.heightAt(x, y),
  );

  const lodNames = ["至近", "戦術", "部隊", "会戦", "戦域"];
  hud.textContent =
    `tick ${simTick}  ${running ? `▶ ${speed}x` : "⏸"}\n` +
    `兵士 ${soldierCount}（描画 ${soldierRenderer.drawn}）\n` +
    `視野 ${cam.viewWidthM.toFixed(0)} m  ${cam.pxPerM.toFixed(2)} px/m  LOD ${lodNames[cam.lod]}\n` +
    `${fps.toFixed(0)} fps`;

  requestAnimationFrame(frame);
}

requestAnimationFrame(frame);

// ── 起動 ────────────────────────────────────────────────

send({ type: "init", seed: 0x5eed1234, sizeM: 2000, relief: 400 });
