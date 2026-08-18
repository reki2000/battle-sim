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
  type ParsedStats,
} from "./render/command-overlay";
import { OrderPanel } from "./ui/orders";
import { DetailPanel } from "./ui/detail-panel";
import { SessionPanel } from "./ui/session-panel";
import { ScenarioPanel } from "./ui/scenario-panel";
import { t, onLangChange } from "./i18n";
import { getQuality, onQualityChange } from "./quality";

const TICK_MS = 50;
/** クリックとドラッグを区別するしきい値（px）。 */
const CLICK_MOVE_THRESHOLD = 6;
/** 兵士選択のクリック許容半径（px）。 */
const SOLDIER_PICK_RADIUS_PX = 24;
/** 兵士を追従し始めたときの視野幅（m）。 */
const FOLLOW_VIEW_WIDTH_M = 80;

const terrainCanvas = document.getElementById("terrain-view") as HTMLCanvasElement;
const soldierCanvas = document.getElementById("soldier-view") as HTMLCanvasElement;
const overlayCanvas = document.getElementById("overlay-view") as HTMLCanvasElement;
const overlayCtx = overlayCanvas.getContext("2d", { alpha: true })!;
const hud = document.getElementById("hud") as HTMLDivElement;
const commandPanel = document.getElementById("command-panel") as HTMLDivElement;
const detailPanelEl = document.getElementById("detail-panel") as HTMLDivElement;
const sessionPanelEl = document.getElementById("session-panel") as HTMLDivElement;
const scenarioPanelEl = document.getElementById("scenario-panel") as HTMLDivElement;
const battleReportEl = document.getElementById("battle-report") as HTMLDivElement;

/**
 * 会戦プリセットを選んでいないときの起動設定（従来の対称デモ配置）。
 * `?soldiers=N` による規模の上書きもこちらだけに効く。
 */
const SANDBOX_INIT = { seed: 0x5eed1234, sizeM: 2000, relief: 400 };
/** `?scenario=` をまだ消化していないか。効かせるのは起動直後の 1 回だけ。 */
let urlScenarioPending = true;

const cam = new Camera();
const terrainGl = new TerrainGlRenderer(terrainCanvas);
const soldierRenderer = new SoldierRenderer(soldierCanvas);
const minimap = new MinimapRenderer();
const snapshot = new SnapshotView();
const interp = new InterpolatedPositions();
let dpr = 1;
let prevFollowed: number | null = null;

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

// ── 憑依・詳細パネル・観戦モード / リプレイ（M8）────────────

const orderPanel = new OrderPanel(
  commandPanel,
  send,
  () => {},
  (node) => send({ type: "queryCommander", node }),
);
const detailPanel = new DetailPanel(detailPanelEl);
const sessionPanel = new SessionPanel(sessionPanelEl, battleReportEl, send);
const scenarioPanel = new ScenarioPanel(scenarioPanelEl, send, SANDBOX_INIT);

onLangChange(() => {
  orderPanel.refreshLang();
  detailPanel.render();
  sessionPanel.render();
  scenarioPanel.render();
});

onQualityChange(() => {
  resize();
  sessionPanel.render();
});

// ── ワーカーからのメッセージ ────────────────────────────

worker.onmessage = async (ev: MessageEvent<FromWorker>) => {
  const msg = ev.data;

  if (msg.type === "soldierInfo") {
    detailPanel.showSoldier(msg.id, msg.node, msg.detail);
    return;
  }
  if (msg.type === "commanderInfo") {
    detailPanel.showCommander(msg.info);
    return;
  }
  if (msg.type === "recording") {
    sessionPanel.onRecording(msg.recording);
    return;
  }
  if (msg.type === "replayResult") {
    sessionPanel.onReplayResult(msg);
    return;
  }

  if (msg.type === "ready") {
    snapshot.setStride(msg.soldierStride);
    try {
      await Promise.all([terrainGl.loadAssets(), soldierRenderer.loadAssets()]);
    } catch (error) {
      // アセットが一時的に取得できない場合もシミュレーション自体は起動し、
      // レンダラ側の単色・図形フォールバックで状態を確認できるようにする。
      console.error("事前生成アセットの読み込みに失敗しました", error);
    }
    const terrain = msg.terrain;
    terrainData = {
      dim: terrain.dim,
      cellM: terrain.cellM,
      sizeM: terrain.sizeM,
      height: new Int16Array(terrain.height),
      ground: new Uint8Array(terrain.ground),
      vegetation: new Uint8Array(terrain.vegetation),
      overlay: new Uint8Array(terrain.overlay),
      water: new Uint16Array(terrain.water),
      waterKind: new Uint8Array(terrain.waterKind),
      cliff: new Uint8Array(terrain.cliff),
    };
    terrainGl.setTerrain(terrainData);
    minimap.setTerrain(terrainData);

    const siteCount = terrain.battleSites.length / 7;
    if (siteCount > 0) {
      const best = terrain.battleSites.slice(0, 7);
      console.log(
        `会戦地候補 ${siteCount} 件。最上位: (${best[0]}, ${best[1]}) score=${best[2]}`,
      );
    }

    cam.worldSizeM = terrain.sizeM;
    cam.centerX = terrain.sizeM / 2;
    cam.centerY = terrain.sizeM / 2;
    cam.setViewWidthM(600);

    // スナップショット・統計はこの World インスタンスに紐づくので、
    // init/リプレイ読み込みのどちらでも一度リセットする。
    heldBuffer = null;
    lastStats = null;
    simTick = 0;
    soldierCount = 0;
    detailPanel.followedSoldierId = null;
    detailPanel.clear();
    orderPanel.possessedNode = null;
    orderPanel.update([]);

    if (msg.scenarios) {
      scenarioPanel.setScenarios(msg.scenarios, msg.scenario);
      // `?scenario=agincourt_1415`（id でも index でも可）で、起動直後に
      // そのプリセットへ切り替える。効かせるのは最初の 1 回だけ——毎回見ると、
      // UI からデモ配置へ戻したときに URL が上書きし返してしまう。
      const requested = urlScenarioPending
        ? new URLSearchParams(location.search).get("scenario")
        : null;
      urlScenarioPending = false;
      if (requested !== null && msg.scenario === undefined) {
        const byId = msg.scenarios.findIndex((s) => s.id === requested);
        const index = byId >= 0 ? byId : Number(requested);
        if (Number.isInteger(index) && index >= 0 && index < msg.scenarios.length) {
          scenarioPanel.selectFromUrl(index);
          return;
        }
      }
    }

    if (msg.reason === "init" && msg.scenario === undefined) {
      // 両軍を向かい合わせに配置し、指揮ツリーの陣形として互いに向かって進ませる。
      // 前列同士の目標座標を完全に一致させると、押し合いの逃げ場がなく密着しすぎて
      // 前列判定が誰も成立しなくなるため、隊列の奥行き＋わずかな隙間ぶんだけ
      // 後方へずらす（sim-headless の battle サブコマンドと同じ考え方）。
      const mid = Math.floor(terrain.sizeM / 2);
      // `?soldiers=N` で総兵数を上書きできる（M9: 50,000 体規模の性能検証用）。
      // 既定の縦横比（40:25）を保ったまま、片陣営あたり N/2 体になるよう調整する。
      const soldiersParam = Number(new URLSearchParams(location.search).get("soldiers"));
      const FILES = soldiersParam > 0 ? Math.round(Math.sqrt((soldiersParam / 2) * (40 / 25))) : 40;
      const RANKS = soldiersParam > 0 ? Math.max(1, Math.round(soldiersParam / 2 / FILES)) : 25;
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
        troopType: 0,
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
        troopType: 1,
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
    } else if (msg.reason === "init") {
      // 会戦プリセットは両軍・指揮系統・築城までワーカー側で配置済み。
      // ここから命令は出さない——動き出すのは指揮官 AI の判断による。
      cam.setViewWidthM(msg.terrain.sizeM * 0.6);
      setRunning(true);
    } else {
      // リプレイは worker 側で既に running=true にしている。
      // メインスレッドの表示用フラグだけ合わせる（再送はしない）。
      running = true;
    }
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
      orderPanel.update(lastStats.nodes);
      sessionPanel.checkBattleEnd(lastStats.nodes, lastStats.combat);
    }
  }
};

// ── 入力 ────────────────────────────────────────────────

let dragging = false;
let lastPointer = { x: 0, y: 0 };
let pointerDownPos = { x: 0, y: 0 };
const viewStack = document.getElementById("view-stack") as HTMLDivElement;

viewStack.addEventListener("pointerdown", (e) => {
  dragging = true;
  lastPointer = { x: e.clientX, y: e.clientY };
  pointerDownPos = { x: e.clientX, y: e.clientY };
  viewStack.setPointerCapture(e.pointerId);
});

viewStack.addEventListener("pointerup", (e) => {
  dragging = false;
  viewStack.releasePointerCapture(e.pointerId);
  const moved = Math.hypot(e.clientX - pointerDownPos.x, e.clientY - pointerDownPos.y);
  if (moved < CLICK_MOVE_THRESHOLD) {
    handleViewClick(e);
  }
});

viewStack.addEventListener("pointermove", (e) => {
  if (!dragging) return;
  cam.panByScreen(e.clientX - lastPointer.x, e.clientY - lastPointer.y);
  lastPointer = { x: e.clientX, y: e.clientY };
});

/**
 * ドラッグを伴わないクリック。憑依 UI が位置/対象を待っていればそれを消化し、
 * そうでなければ最寄りの兵士を選んで詳細パネルに問い合わせる。
 */
function handleViewClick(e: PointerEvent): void {
  const rect = viewStack.getBoundingClientRect();
  const sx = e.clientX - rect.left;
  const sy = e.clientY - rect.top;
  const world = cam.screenToWorld(sx, sy);
  if (orderPanel.handleMapClick(world.x, world.y)) return;
  selectNearestSoldier(world.x, world.y);
}

function selectNearestSoldier(xM: number, yM: number): void {
  if (snapshot.count === 0) return;
  const thresholdM = SOLDIER_PICK_RADIUS_PX / cam.pxPerM;
  let bestI = -1;
  let bestD = thresholdM * thresholdM;
  for (let i = 0; i < snapshot.count; i++) {
    const dx = snapshot.x(i) - xM;
    const dy = snapshot.y(i) - yM;
    const d = dx * dx + dy * dy;
    if (d < bestD) {
      bestD = d;
      bestI = i;
    }
  }
  if (bestI < 0) return;
  send({ type: "querySoldier", id: bestI });
}

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
  dpr = Math.min(getQuality().dprCap, window.devicePixelRatio || 1);
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

  // 20 Hz のスナップショット間を補間する。倍速時は補間しない
  const alpha =
    speed >= 8 ? 1 : Math.min(1, (now - lastSnapshotAt) / (TICK_MS / speed));

  // 一兵士追従モード（M8）。死亡後もその場に留まるので追従を外さない。
  const followed = detailPanel.followedSoldierId;
  if (followed !== null && followed < snapshot.count) {
    if (followed !== prevFollowed) cam.setViewWidthM(FOLLOW_VIEW_WIDTH_M);
    cam.centerX = interp.x(followed, alpha);
    cam.centerY = interp.y(followed, alpha);
  } else if (lastStats) {
    // 観戦モード（M8）。追従中は競合しないよう止める。
    sessionPanel.updateSpectator(cam, lastStats.nodes, now);
  }
  prevFollowed = followed;

  terrainGl.draw(cam);

  overlayCtx.clearRect(0, 0, overlayCanvas.width, overlayCanvas.height);

  if (terrainData) {
    const td = terrainData;
    soldierRenderer.draw(overlayCtx, cam, snapshot, interp, alpha, now / 1000, (x, y) =>
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
    `${fps.toFixed(0)} fps` +
    (lastStats
      ? `\n${t("deathCauses")}: ${deathCauseLegend(lastStats.combat)}\n` +
        `${t("kills")} ${lastStats.combat.kills}  ${t("downed")} ${lastStats.combat.downed}  ` +
        `${t("friendlyFire")} ${lastStats.combat.friendlyFireHits}`
      : "");

  requestAnimationFrame(frame);
}

requestAnimationFrame(frame);

// ── 起動 ────────────────────────────────────────────────

send({ type: "init", ...SANDBOX_INIT });
