/**
 * 兵士 1 体・指揮官 1 人の詳細パネルと、兵士の追従カメラ状態（M8）。
 *
 * 表示データそのものは `sim/worker.ts` からの `soldierInfo` / `commanderInfo`
 * 応答を受け取って渡してもらう（この class は問い合わせを発行しない）。
 */

import type { CommanderInfo, SoldierDetail } from "../sim/protocol";
import { t } from "../i18n";

export class DetailPanel {
  /** 追従中の兵士 ID。死亡後もその場に留まるので追従は外さない。 */
  followedSoldierId: number | null = null;
  private mode: "soldier" | "commander" | null = null;
  private soldier: { id: number; node: number; detail: SoldierDetail | null } | null = null;
  private commander: CommanderInfo | null = null;

  constructor(private container: HTMLDivElement) {
    this.container.style.pointerEvents = "auto";
  }

  showSoldier(id: number, node: number, detail: SoldierDetail | null): void {
    this.mode = "soldier";
    this.soldier = { id, node, detail };
    this.commander = null;
    this.render();
  }

  showCommander(info: CommanderInfo): void {
    this.mode = "commander";
    this.commander = info;
    this.render();
  }

  clear(): void {
    this.mode = null;
    this.soldier = null;
    this.commander = null;
    this.render();
  }

  render(): void {
    const c = this.container;
    c.innerHTML = "";
    if (this.mode === "soldier" && this.soldier) {
      c.appendChild(this.renderSoldier(this.soldier));
    } else if (this.mode === "commander" && this.commander) {
      c.appendChild(this.renderCommander(this.commander));
    } else {
      c.textContent = t("clickSoldierToSelect");
    }
  }

  private renderSoldier(s: { id: number; node: number; detail: SoldierDetail | null }): HTMLDivElement {
    const wrap = document.createElement("div");
    const title = document.createElement("div");
    title.className = "panel-title";
    title.textContent = `${t("soldierDetail")} #${s.id}`;
    wrap.appendChild(title);

    if (!s.detail) {
      const p = document.createElement("div");
      p.textContent = "-";
      wrap.appendChild(p);
      return wrap;
    }
    const d = s.detail;
    const lines = [
      `${t("hp")}: ${d.hp}`,
      `${t("morale")}: ${d.morale}`,
      `${t("fatigue")}: ${d.fatigue}`,
      `${t("ammo")}: ${d.ammo}`,
      `${t("target")}: ${d.target ?? t("none")}`,
      `${t("bravery")}: ${d.bravery}  ${t("discipline")}: ${d.discipline}  ${t("skill")}: ${d.skill}`,
    ];
    for (const line of lines) {
      const p = document.createElement("div");
      p.textContent = line;
      wrap.appendChild(p);
    }

    const followBtn = document.createElement("button");
    const following = this.followedSoldierId === s.id;
    followBtn.textContent = following ? t("unfollow") : t("follow");
    followBtn.onclick = () => {
      this.followedSoldierId = following ? null : s.id;
      this.render();
    };
    wrap.appendChild(followBtn);

    if (this.followedSoldierId !== null) {
      const p = document.createElement("div");
      p.className = "follow-status";
      p.textContent = t("followingSoldier", { id: this.followedSoldierId });
      wrap.appendChild(p);
    }

    return wrap;
  }

  private renderCommander(info: CommanderInfo): HTMLDivElement {
    const wrap = document.createElement("div");
    const title = document.createElement("div");
    title.className = "panel-title";
    title.textContent = `${t("commanderDetail")} #${info.node}`;
    wrap.appendChild(title);

    const attrLine = document.createElement("div");
    const a = info.attrs;
    attrLine.textContent =
      `boldness${a.boldness} caution${a.caution} init${a.initiative} obed${a.obedience} ` +
      `tactic${a.tacticalSkill} ambition${a.ambition} charisma${a.charisma} flex${a.flexibility} ` +
      `patience${a.patience} ruth${a.ruthlessness}`;
    wrap.appendChild(attrLine);

    const perceived = document.createElement("div");
    perceived.textContent = `${t("perceived")}: ${t("forceRatio")} ${info.perceived.forceRatioPermille} ${t("momentum")} ${info.perceived.momentum}`;
    wrap.appendChild(perceived);

    const actual = document.createElement("div");
    actual.textContent = `${t("actual")}: ${t("forceRatio")} ${info.actual.forceRatioPermille} ${t("momentum")} ${info.actual.momentum}`;
    wrap.appendChild(actual);

    const known = document.createElement("div");
    known.className = "panel-subtitle";
    known.textContent = `${t("knownEnemies")} (${info.knownEnemies.length})`;
    wrap.appendChild(known);
    for (const e of info.knownEnemies.slice(0, 6)) {
      const p = document.createElement("div");
      p.textContent = `#${e.node} @(${e.xM.toFixed(0)},${e.yM.toFixed(0)}) 兵力${e.strength} 確度${e.confidence}`;
      wrap.appendChild(p);
    }

    const log = document.createElement("div");
    log.className = "panel-subtitle";
    log.textContent = t("decisionLog");
    wrap.appendChild(log);
    for (const rec of info.decisionLog.slice(-5)) {
      const p = document.createElement("div");
      p.textContent = `t${rec.tick} → ${rec.chosen} (${rec.score})`;
      wrap.appendChild(p);
    }

    return wrap;
  }
}
