/**
 * 観戦モード（自動カメラ）・リプレイの保存/読み込み・会戦報告（M8）。
 *
 * どれも「低頻度・メインスレッド完結」の機能なので 1 ファイルにまとめる。
 */

import type { Camera } from "../render/iso";
import type { CombatStats, CommandNode } from "../render/command-overlay";
import type { Recording, ToWorker } from "../sim/protocol";
import { t, toggleLang } from "../i18n";

const SPECTATOR_INTERVAL_MS = 4000;
const SPECTATOR_VIEW_WIDTH_M = 180;

export class SessionPanel {
  spectatorOn = false;
  private lastSwitchAt = 0;
  private prevDeaths = new Map<number, number>();
  private roundRobinIdx = 0;
  private battleReportShown = false;
  private statusText = "";

  constructor(
    private container: HTMLDivElement,
    private reportContainer: HTMLDivElement,
    private send: (msg: ToWorker) => void,
  ) {
    this.container.style.pointerEvents = "auto";
    this.render();
  }

  toggleSpectator(): void {
    this.spectatorOn = !this.spectatorOn;
    this.lastSwitchAt = 0;
    this.render();
  }

  /** 毎フレーム呼ぶ。観戦モード中なら見どころへカメラを寄せる。 */
  updateSpectator(cam: Camera, nodes: CommandNode[], now: number): void {
    if (!this.spectatorOn || nodes.length === 0) return;
    if (now - this.lastSwitchAt < SPECTATOR_INTERVAL_MS) return;
    this.lastSwitchAt = now;

    const engaged = nodes.filter((n) => n.hasUnit && n.alive > 0);
    if (engaged.length === 0) return;

    // 「見どころ」＝前回サンプル以降にもっとも損耗が進んだ部隊。
    // 動きがなければ、部隊を順番に見せる（ラウンドロビン）。
    let best: CommandNode | null = null;
    let bestDelta = 0;
    for (const n of engaged) {
      const prev = this.prevDeaths.get(n.id) ?? 0;
      const delta = n.dead + n.downed - prev;
      this.prevDeaths.set(n.id, n.dead + n.downed);
      if (delta > bestDelta) {
        bestDelta = delta;
        best = n;
      }
    }
    if (!best) {
      this.roundRobinIdx = (this.roundRobinIdx + 1) % engaged.length;
      best = engaged[this.roundRobinIdx]!;
    }

    cam.centerX = best.centroidXM;
    cam.centerY = best.centroidYM;
    cam.setViewWidthM(SPECTATOR_VIEW_WIDTH_M);
  }

  requestSaveReplay(): void {
    this.send({ type: "getRecording" });
  }

  onRecording(recording: Recording): void {
    const json = JSON.stringify(recording, null, 0);
    const blob = new Blob([json], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `battle-sim-replay-${recording.finalTick}.json`;
    a.click();
    URL.revokeObjectURL(url);
  }

  async loadReplayFile(file: File): Promise<void> {
    const text = await file.text();
    const recording = JSON.parse(text) as Recording;
    this.battleReportShown = false;
    this.reportContainer.style.display = "none";
    this.send({ type: "loadReplay", recording });
  }

  onReplayResult(result: {
    matched: boolean;
    atTick: number;
    hashLo: number;
    hashHi: number;
  }): void {
    this.statusText = result.matched
      ? t("replayMatched", { tick: result.atTick })
      : t("replayMismatch", { tick: result.atTick });
    this.render();
  }

  /** 陣営のどちらかが全滅したら 1 度だけ会戦報告を出す。 */
  checkBattleEnd(nodes: CommandNode[], combat: CombatStats): void {
    if (this.battleReportShown) return;
    const roots = nodes.filter((n) => n.parent < 0);
    if (roots.length < 2) return;
    const aliveByFaction = new Map<number, number>();
    for (const n of nodes) {
      aliveByFaction.set(n.faction, (aliveByFaction.get(n.faction) ?? 0) + n.alive);
    }
    const factions = Array.from(aliveByFaction.keys());
    const anyWiped = factions.some((f) => (aliveByFaction.get(f) ?? 0) === 0);
    const anyAlive = factions.some((f) => (aliveByFaction.get(f) ?? 0) > 0);
    if (!(anyWiped && anyAlive)) return;

    this.battleReportShown = true;
    this.reportContainer.style.display = "block";
    this.reportContainer.innerHTML = "";
    const title = document.createElement("div");
    title.className = "panel-title";
    title.textContent = t("battleReport");
    this.reportContainer.appendChild(title);
    for (const [faction, alive] of aliveByFaction) {
      const p = document.createElement("div");
      p.textContent = `陣営${faction}: 生存 ${alive}`;
      this.reportContainer.appendChild(p);
    }
    const stats = document.createElement("div");
    stats.textContent =
      `${t("kills")} ${combat.kills}  ${t("downed")} ${combat.downed}  ${t("friendlyFire")} ${combat.friendlyFireHits}`;
    this.reportContainer.appendChild(stats);
  }

  render(): void {
    const c = this.container;
    c.innerHTML = "";

    const specBtn = document.createElement("button");
    specBtn.textContent = this.spectatorOn ? t("spectatorOn") : t("spectatorOff");
    specBtn.onclick = () => this.toggleSpectator();
    c.appendChild(specBtn);

    const saveBtn = document.createElement("button");
    saveBtn.textContent = t("saveReplay");
    saveBtn.onclick = () => this.requestSaveReplay();
    c.appendChild(saveBtn);

    const loadBtn = document.createElement("button");
    loadBtn.textContent = t("loadReplay");
    const fileInput = document.createElement("input");
    fileInput.type = "file";
    fileInput.accept = "application/json";
    fileInput.style.display = "none";
    fileInput.onchange = () => {
      const f = fileInput.files?.[0];
      if (f) void this.loadReplayFile(f);
    };
    loadBtn.onclick = () => fileInput.click();
    c.appendChild(loadBtn);
    c.appendChild(fileInput);

    if (this.statusText) {
      const p = document.createElement("div");
      p.className = "status-line";
      p.textContent = this.statusText;
      c.appendChild(p);
    }

    const langBtn = document.createElement("button");
    langBtn.textContent = t("lang");
    langBtn.onclick = () => toggleLang();
    c.appendChild(langBtn);
  }
}
