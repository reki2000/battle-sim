/**
 * 兵士 1 体・指揮官 1 人の詳細パネルと、兵士の追従カメラ状態（M8）。
 *
 * 表示データそのものは `sim/worker.ts` からの `soldierInfo` / `commanderInfo`
 * 応答を受け取って渡してもらう（この class は問い合わせを発行しない）。
 */

import type { CommanderInfo, SoldierDebug, SoldierDetail } from "../sim/protocol";
import { t } from "../i18n";

/** `metrics::MissionKind` の識別子を表示名にする。 */
function missionLabel(mission: string): string {
  const key = MISSION_LABELS[mission];
  return key ? t(key) : mission;
}

const MISSION_LABELS: Record<string, string> = {
  no_unit: "missionNoUnit",
  no_objective: "missionNoObjective",
  move_to: "missionMoveTo",
  hold: "missionHold",
  attack: "missionAttack",
  charge: "missionCharge",
  flank: "missionFlank",
  envelop: "missionEnvelop",
  screen: "missionScreen",
  reserve: "missionReserve",
  withdraw: "missionWithdraw",
  pursue: "missionPursue",
  shoot_at: "missionShootAt",
  hunt_person: "missionHuntPerson",
  occupy_area: "missionOccupyArea",
  guard_area: "missionGuardArea",
};

/** `organization::MissionState` の識別子を表示名にする。 */
function missionStateLabel(state: string): string {
  const key = MISSION_STATE_LABELS[state];
  return key ? t(key) : state;
}

const MISSION_STATE_LABELS: Record<string, string> = {
  idle: "missionStateIdle",
  moving: "missionStateMoving",
  deploying: "missionStateDeploying",
  engaged: "missionStateEngaged",
  secured: "missionStateSecured",
  failed: "missionStateFailed",
  returning: "missionStateReturning",
};

/** `soldier_ai::IndividualAction` の識別子を表示名にする。 */
function actionLabel(action: string): string {
  const key = ACTION_LABELS[action];
  return key ? t(key) : action;
}

const ACTION_LABELS: Record<string, string> = {
  follow_order: "actionFollowOrder",
  join_fight: "actionJoinFight",
  support: "actionSupport",
  defend: "actionDefend",
  hold_slot: "actionHoldSlot",
  fall_back: "actionFallBack",
  flee: "actionFlee",
  rally: "actionRally",
};

export class DetailPanel {
  /** 追従中の兵士 ID。死亡後もその場に留まるので追従は外さない。 */
  followedSoldierId: number | null = null;
  private mode: "soldier" | "commander" | null = null;
  private soldier: {
    id: number;
    node: number;
    detail: SoldierDetail | null;
    debug: SoldierDebug | null;
  } | null = null;
  private commander: CommanderInfo | null = null;

  constructor(private container: HTMLDivElement) {
    this.container.style.pointerEvents = "auto";
  }

  showSoldier(
    id: number,
    node: number,
    detail: SoldierDetail | null,
    debug: SoldierDebug | null = null,
  ): void {
    this.mode = "soldier";
    this.soldier = { id, node, detail, debug };
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

  private renderSoldier(s: {
    id: number;
    node: number;
    detail: SoldierDetail | null;
    debug: SoldierDebug | null;
  }): HTMLDivElement {
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

    if (s.debug) {
      wrap.appendChild(this.renderDecision(s.debug));
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

  /**
   * 兵士の判断まわり（B0）。任務・現在行動・行動候補・目標地点・認識中の敵・
   * 隊形スロット・反応待ち時間を、後続フェーズの挙動改善を目で追えるように出す。
   */
  private renderDecision(d: SoldierDebug): HTMLDivElement {
    const wrap = document.createElement("div");
    const title = document.createElement("div");
    title.className = "panel-subtitle";
    title.textContent = t("decisionState");
    wrap.appendChild(title);

    const m = (mm: number): string => (mm / 1000).toFixed(1);
    const ticks = (v: number): string => (v < 0 ? "-" : `${v} ${t("ticksUnit")}`);
    const lines = [
      `${t("mission")}: ${missionLabel(d.mission)}${d.node >= 0 ? ` (#${d.node})` : ""}`,
      `${t("missionState")}: ${missionStateLabel(d.missionState)} (t${d.missionSinceTick})`,
      `${t("currentAction")}: ${actionLabel(d.action)}`,
      `${t("focusTarget")}: ${d.focus ?? t("none")}${
        d.orderedFocus !== null ? ` / ${t("orderHuntPerson")} #${d.orderedFocus}` : ""
      }`,
      `${t("goalPoint")}: (${m(d.goalXMm)}, ${m(d.goalYMm)}) m`,
      `${t("formationSlot")}: #${d.slot} (${m(d.slotXMm)}, ${m(d.slotYMm)}) m`,
      `${t("slotDistance")}: ${m(d.slotDistanceMm)} m`,
      `${t("reactionRadius")}: ${m(d.reactionRadiusMm)} m`,
      `${t("thinkIn")}: ${ticks(d.thinkInTicks)}  ${t("formationSampleIn")}: ${ticks(
        d.formationSampleInTicks,
      )}`,
      `${t("commitLeft")}: ${ticks(d.commitTicksLeft)}`,
    ];
    for (const line of lines) {
      const p = document.createElement("div");
      p.textContent = line;
      wrap.appendChild(p);
    }

    // 地点任務なら、範囲と追撃限界、範囲の中の人数（B5）。
    if (d.area) {
      const line = document.createElement("div");
      line.textContent =
        `${t("missionArea")}: (${m(d.area.centerXMm)}, ${m(d.area.centerYMm)}) r${m(d.area.radiusMm)} m` +
        `${d.area.leashMm > 0 ? ` / ${t("pursuitLeash")} ${m(d.area.leashMm)} m` : ""}` +
        ` / ${t("insideArea")} ${d.insideFriendly}v${d.insideEnemy}`;
      wrap.appendChild(line);
    }

    const a = d.awareness;
    const awarenessLines = [
      `${t("perceivedNeighbours")}: ${t("enemyCount", { n: a.enemies })} / ${t("allyCount", {
        n: a.allies,
      })}`,
      `${t("threatDirections")}: ${t("front")} ${a.threatFront} / ${t("flank")} ${a.threatFlank} / ${t("rear")} ${a.threatRear}`,
      `${t("nearestEnemy")}: ${
        a.nearestEnemy === null ? t("none") : `#${a.nearestEnemy} ${m(a.nearestEnemyMm)} m`
      }`,
      `${t("supportableFight")}: ${
        a.supportableEnemy === null ? t("none") : `#${a.supportableEnemy}`
      }`,
      `${t("crowding")}: ${a.crowding}  ${t("localBroken")}: ${a.localBroken}`,
    ];
    for (const line of awarenessLines) {
      const p = document.createElement("div");
      p.textContent = line;
      wrap.appendChild(p);
    }

    // 行動の点数（B2）。何と何を比べて、いまの行動を選んだのかを出す。
    // 記録は「選んだ兵士が次に判断した瞬間」に入るので、選んだ直後は空になる。
    const scoresTitle = document.createElement("div");
    scoresTitle.className = "panel-subtitle";
    scoresTitle.textContent = t("actionScores");
    wrap.appendChild(scoresTitle);
    if (d.actionScores.length === 0) {
      const p = document.createElement("div");
      p.textContent = t("noCandidates");
      wrap.appendChild(p);
    }
    for (const entry of [...d.actionScores].sort((a, b) => b.score - a.score)) {
      const p = document.createElement("div");
      const chosen = entry.action === d.chosenAction ? " ←" : "";
      p.textContent = `${actionLabel(entry.action)}: ${entry.score}${chosen}`;
      wrap.appendChild(p);
    }

    const candidates = document.createElement("div");
    candidates.className = "panel-subtitle";
    candidates.textContent = `${t("actionCandidates")}${
      d.decidedAtTick !== null ? ` (t${d.decidedAtTick})` : ""
    }`;
    wrap.appendChild(candidates);
    if (d.candidates.length === 0) {
      const p = document.createElement("div");
      p.textContent = t("noCandidates");
      wrap.appendChild(p);
    }
    for (const c of d.candidates) {
      const p = document.createElement("div");
      p.textContent =
        `#${c.target} ${c.score} pt / ${m(c.distanceMm)} m` +
        `${c.fighting ? ` / ${t("candidateFighting")}` : ""}` +
        `${c.load > 0 ? ` / ${t("candidateLoad")} ${c.load}` : ""}`;
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
