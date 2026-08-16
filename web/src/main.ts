/**
 * エントリポイント。
 *
 * 地形と兵士はそれぞれ WebGL2 キャンバス、UI とミニマップは透明な Canvas2D
 * キャンバスへ描く。兵士は M2 のインスタンス描画で 1 ドローコールにまとめる。
 */

import { Camera } from "./render/iso";
import { TerrainGlRenderer } from "./render/terrain-gl";
import { SoldierRenderer } from "./render/soldiers";
import { MinimapRenderer } from "./render/minimap";
import { InterpolatedPositions, SnapshotView } from "./sim/snapshot";
import type { TerrainData } from "./sim/terrain-data";
import type { FromWorker, ToWorker } from "./sim/protocol";
import {
  parseStats,
  drawOrderVisualization,
  drawDeathCauseGraph,
  deathCauseLegend,
  commandTreeSummary,
  type ParsedStats,
} from "./render/command-overlay";

const TICK_MS = 50;

const terrainCanvas = document.getElementById("terrain-view") as HTMLCanvasElement;
const soldierCanvas = document.getElementById("soldier-view") as HTMLCanvasElement;
const overlayCanvas = document.getElementById("overlay-view") as HTMLCanvasElement;
const overlayCtx = overlayCanvas.getContext("2d", { alpha: true })!;
const hud = document.getElementById("hud") as HTMLDivElement;
const commandPanel = document.getElementById("command-panel") as HTMLDivElement;

const cam = new Camera();
const terrainGl = new TerrainGlRenderer(terrainCanvas);
const soldierRenderer = new SoldierRenderer(soldierCanvas);
const minimap = new MinimapRenderer();
const snapshot = new SnapshotView();
const interp = new InterpolatedPositions();
let dpr = 1;

let terrainData: TerrainData | null = null;
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
/** 指揮ツリー・戦闘統計。間引いて届くので、届いた最新のものを保持する。 */
let lastStats: ParsedStats | null = null;

const worker = new Worker(new URL("./sim/worker.ts", import.meta.url), {
  type: "module",
});

function send(msg: ToWorker, transfer: Transferable[] = []): void {
  worker.postMessage(msg, transfer);
}

// ── ワーカーからのメッセージ ────────────────────────────

worker.onmessage = async (ev: MessageEvent<FromWorker>) => {
  const msg = ev.data;

  if (msg.type === "ready") {
    try {
      await Promise.all([terrainGl.loadAssets(), soldierRenderer.loadAssets()]);
    } catch (error) {
      // アセットが一時的に取得できない場合もシミュレーション自体は起動し、
      // レンダラ側の単色・図形フォールバックで状態を確認できるようにする。
      console.error("事前生成アセットの読み込みに失敗しました", error);
    }
    const t = msg.terrain;
    terrainData = {
      dim: t.dim,
      cellM: t.cellM,
      sizeM: t.sizeM,
      surface: new Uint8Array(t.surface),
      height: new Int16Array(t.height),
      water: new Uint8Array(t.water),
      waterKind: new Uint8Array(t.waterKind),
      cliff: new Uint8Array(t.cliff),
    };
    terrainGl.setTerrain(terrainData);
    minimap.setTerrain(terrainData);

    const siteCount = t.battleSites.length / 7;
    if (siteCount > 0) {
      const best = t.battleSites.slice(0, 7);
      console.log(
        `会戦地候補 ${siteCount} 件。最上位: (${best[0]}, ${best[1]}) score=${best[2]}`,
      );
    }

    cam.worldSizeM = t.sizeM;
    cam.centerX = t.sizeM / 2;
    cam.centerY = t.sizeM / 2;
    cam.setViewWidthM(600);

    // 両軍を向かい合わせに配置し、指揮ツリーの陣形として互いに向かって進ませる。
    // 前列同士の目標座標を完全に一致させると、押し合いの逃げ場がなく密着しすぎて
    // 前列判定が誰も成立しなくなるため、隊列の奥行き＋わずかな隙間ぶんだけ
    // 後方へずらす（sim-headless の battle サブコマンドと同じ考え方）。
    const mid = Math.floor(t.sizeM / 2);
    const FILES = 40;
    const RANKS = 25;
    const RANK_SPACING_M = 0.8;
    const CONTACT_GAP_M = 0.5;
    const depth = RANK_SPACING_M * (RANKS - 1);
    const SHIELDWALL = 1; // organization::FORMATION_SHIELDWALL

    send({
      type: "deploy",
      xM: mid - 16,
      yM: mid - depth - CONTACT_GAP_M,
      files: FILES,
      ranks: RANKS,
      spacingMm: 800,
      faction: 0,
      unitId: 0,
      seedSalt: 1,
    });
    send({
      type: "deploy",
      xM: mid - 16,
      yM: mid + depth + CONTACT_GAP_M,
      files: FILES,
      ranks: RANKS,
      spacingMm: 800,
      faction: 1,
      unitId: 1,
      seedSalt: 2,
    });

    const perSide = FILES * RANKS;
    // organization::formation_goals の回転規約では facing=0 でランクが +Y
    // （北）へ、facing=180°（32768 brad）で -Y（南）へ伸びる。
    send({ type: "addLineUnit", faction: 0, firstId: 0, count: perSide, ranks: RANKS, formation: SHIELDWALL });
    send({ type: "addLineUnit", faction: 1, firstId: perSide, count: perSide, ranks: RANKS, formation: SHIELDWALL });
    send({
      type: "issueMoveTo",
      node: 0,
      xM: mid,
      yM: mid - depth - CONTACT_GAP_M,
      facingBrad: 0,
      formation: SHIELDWALL,
    });
    send({
      type: "issueMoveTo",
      node: 1,
      xM: mid,
      yM: mid + depth + CONTACT_GAP_M,
      facingBrad: 32768,
      formation: SHIELDWALL,
    });

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
    if (msg.stats) {
      lastStats = parseStats(msg.stats);
    }
  }
};

// ── 入力 ────────────────────────────────────────────────

let dragging = false;
let lastPointer = { x: 0, y: 0 };
const viewStack = document.getElementById("view-stack") as HTMLDivElement;

viewStack.addEventListener("pointerdown", (e) => {
  dragging = true;
  lastPointer = { x: e.clientX, y: e.clientY };
  viewStack.setPointerCapture(e.pointerId);
});

viewStack.addEventListener("pointerup", (e) => {
  dragging = false;
  viewStack.releasePointerCapture(e.pointerId);
});

viewStack.addEventListener("pointermove", (e) => {
  if (!dragging) return;
  cam.panByScreen(e.clientX - lastPointer.x, e.clientY - lastPointer.y);
  lastPointer = { x: e.clientX, y: e.clientY };
});

viewStack.addEventListener(
  "wheel",
  (e) => {
    e.preventDefault();
    const rect = viewStack.getBoundingClientRect();
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
  dpr = Math.min(2, window.devicePixelRatio || 1);
  const w = Math.floor(window.innerWidth * dpr);
  const h = Math.floor(window.innerHeight * dpr);

  for (const c of [terrainCanvas, soldierCanvas, overlayCanvas]) {
    c.width = w;
    c.height = h;
    c.style.width = `${window.innerWidth}px`;
    c.style.height = `${window.innerHeight}px`;
  }
  cam.viewW = w;
  cam.viewH = h;
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

  terrainGl.draw(cam);

  overlayCtx.clearRect(0, 0, overlayCanvas.width, overlayCanvas.height);

  // 20 Hz のスナップショット間を補間する。倍速時は補間しない
  const alpha =
    speed >= 8 ? 1 : Math.min(1, (now - lastSnapshotAt) / (TICK_MS / speed));
  if (terrainData) {
    const td = terrainData;
    soldierRenderer.draw(overlayCtx, cam, snapshot, interp, alpha, (x, y) =>
      terrainGl.heightAt(td, x, y),
    );
  }

  if (terrainData) {
    minimap.draw(
      overlayCtx,
      overlayCanvas.width,
      overlayCanvas.height,
      dpr,
      cam,
      snapshot,
      interp,
    );
  }

  // 命令の可視化（矢印・伝令の移動、仕様 12 章 M3）と死因内訳のグラフ（M4）。
  if (lastStats) {
    drawOrderVisualization(overlayCtx, cam, lastStats.nodes, lastStats.messengers);
    const graphW = 260 * dpr;
    const graphH = 14 * dpr;
    const graphX = overlayCanvas.width - graphW - 12 * dpr;
    const graphY = 12 * dpr;
    drawDeathCauseGraph(overlayCtx, graphX, graphY, graphW, graphH, lastStats.combat);
  }

  const lodNames = ["至近", "戦術", "部隊", "会戦", "戦域"];
  hud.textContent =
    `tick ${simTick}  ${running ? `▶ ${speed}x` : "⏸"}\n` +
    `兵士 ${soldierCount}（描画 ${soldierRenderer.drawn}）\n` +
    `視野 ${cam.viewWidthM.toFixed(0)} m  ${cam.pxPerM.toFixed(2)} px/m  LOD ${lodNames[cam.lod]}\n` +
    `${fps.toFixed(0)} fps`;

  if (lastStats) {
    commandPanel.textContent =
      `── 指揮系統 ──\n${commandTreeSummary(lastStats.nodes) || "(部隊なし)"}\n\n` +
      `── 死因内訳 ──\n${deathCauseLegend(lastStats.combat)}\n` +
      `撃破 ${lastStats.combat.kills}  戦闘不能 ${lastStats.combat.downed}  ` +
      `誤射 ${lastStats.combat.friendlyFireHits}`;
  }

  requestAnimationFrame(frame);
}

requestAnimationFrame(frame);

// ── 起動 ────────────────────────────────────────────────

send({ type: "init", seed: 0x5eed1234, sizeM: 2000, relief: 400 });
