/**
 * 会戦プリセットの選択（`sim_core::scenario`）。
 *
 * 一覧はワーカーの `ready` に乗って届く。選ぶと `init` を送り直し、
 * ワールドを丸ごと組み直す——地形・陣容・指揮官の性格までプリセットが決める。
 *
 * 命令は一切与えない。動き出すのは指揮官 AI の判断なので、このパネルは
 * 「誰が指揮していて、どういう性格か」を見せることに紙面を使う。
 */

import type { ScenarioInfo, ToWorker } from "../sim/protocol";
import { t } from "../i18n";

/** 従来の対称デモ配置を表す選択肢。プリセットの index とは別物なので負の値。 */
export const SANDBOX = -1;

/** アーキタイプ名（`commander_ai::ARCHETYPES`）の日本語表示。 */
const ARCHETYPE_JA: Record<string, string> = {
  honor_hungry_knight: "名誉に飢えた騎士",
  veteran_mercenary: "歴戦の傭兵",
  cautious_commander: "慎重な指揮官",
  stubborn_baron: "頑固な諸侯",
  disciplinarian: "規律家",
  reckless_youth: "無謀な若者",
  professional_marshal: "職業軍人",
  cunning_captain: "狡猾な隊長",
};

/** 会戦プランの日本語表示。 */
const PLAN_JA: Record<string, string> = {
  CenterPush: "中央突破",
  DoubleEnvelopment: "両翼包囲",
  RefusedFlank: "斜線陣",
  DefendHighGround: "高地の守勢",
  FeignedRetreat: "偽装退却",
  EchelonAttack: "梯形攻撃",
  CavalryReserve: "騎兵を予備に",
  Delay: "遅滞",
};

export function archetypeLabel(archetype: string): string {
  return ARCHETYPE_JA[archetype] ?? archetype;
}

export class ScenarioPanel {
  private scenarios: ScenarioInfo[] = [];
  /** 現在表示しているワールドの出所。`SANDBOX` なら対称デモ配置。 */
  private current = SANDBOX;
  /** 詳細（陣容・指揮官）を開いているか。 */
  private expanded = false;

  constructor(
    private container: HTMLDivElement,
    private send: (msg: ToWorker) => void,
    /** 対称デモ配置で起動し直すときの設定。 */
    private sandboxInit: { seed: number; sizeM: number; relief: number },
  ) {
    this.container.style.pointerEvents = "auto";
    this.render();
  }

  /** ワーカーの `ready` から一覧と現在の選択を受け取る。 */
  setScenarios(scenarios: ScenarioInfo[], current: number | undefined): void {
    this.scenarios = scenarios;
    this.current = current ?? SANDBOX;
    this.render();
  }

  /** `?scenario=` で指定されたプリセットへ、起動直後に切り替える。 */
  selectFromUrl(index: number): void {
    this.select(index);
  }

  private select(index: number): void {
    this.current = index;
    if (index === SANDBOX) {
      this.send({ type: "init", ...this.sandboxInit });
    } else {
      const s = this.scenarios[index]!;
      this.send({
        type: "init",
        seed: s.seedLo,
        sizeM: s.sizeM,
        relief: s.relief,
        scenario: index,
      });
    }
    this.render();
  }

  private currentScenario(): ScenarioInfo | null {
    return this.current === SANDBOX ? null : (this.scenarios[this.current] ?? null);
  }

  render(): void {
    const c = this.container;
    c.innerHTML = "";

    const title = document.createElement("div");
    title.className = "panel-title";
    title.textContent = t("battlePreset");
    c.appendChild(title);

    const row = document.createElement("div");
    row.className = "order-row";

    const select = document.createElement("select");
    const sandbox = document.createElement("option");
    sandbox.value = String(SANDBOX);
    sandbox.textContent = t("sandboxBattle");
    select.appendChild(sandbox);
    for (const [i, s] of this.scenarios.entries()) {
      const opt = document.createElement("option");
      opt.value = String(i);
      opt.textContent = `${s.nameJa}（${s.year}）`;
      select.appendChild(opt);
    }
    select.value = String(this.current);
    select.onchange = () => this.select(Number(select.value));
    row.appendChild(select);

    const scenario = this.currentScenario();
    if (scenario) {
      const toggle = document.createElement("button");
      toggle.textContent = this.expanded ? t("hideOrderOfBattle") : t("showOrderOfBattle");
      toggle.onclick = () => {
        this.expanded = !this.expanded;
        this.render();
      };
      row.appendChild(toggle);
    }
    c.appendChild(row);

    if (!scenario) {
      const note = document.createElement("div");
      note.className = "panel-subtitle";
      note.textContent = t("sandboxNote");
      c.appendChild(note);
      return;
    }

    for (const line of [
      `${scenario.placeJa} / ${scenario.year}`,
      scenario.summaryJa,
      scenario.historicalStrengthJa,
      `${t("deployed")}: ${scenario.soldiers}（${scenario.scaleNoteJa}）`,
    ]) {
      const p = document.createElement("div");
      p.className = "panel-subtitle";
      p.textContent = line;
      c.appendChild(p);
    }

    for (const army of scenario.armies) {
      const head = document.createElement("div");
      head.textContent =
        `■ ${army.nameJa} ${army.soldiers} — ${army.commanderJa}` +
        `（${archetypeLabel(army.archetype)}）`;
      c.appendChild(head);

      const plan = document.createElement("div");
      plan.className = "panel-subtitle";
      plan.textContent = `　${t("battlePlan")}: ${PLAN_JA[army.battlePlan] ?? army.battlePlan}`;
      c.appendChild(plan);

      if (!this.expanded) continue;
      for (const unit of army.contingents) {
        const row = document.createElement("div");
        row.className = "panel-subtitle";
        row.textContent =
          `　・${unit.nameJa} ${unit.count} — ${unit.commanderJa}` +
          `（${archetypeLabel(unit.archetype)}）`;
        c.appendChild(row);
      }
    }
  }
}
