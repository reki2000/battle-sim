/**
 * 憑依 UI: 指揮系統ツリーからノードを選んで「憑依」し、命令を発行する（M8）。
 *
 * 「条件付き」命令は仕様側にもまだ具体的な構造がなく、現行実装の対象外
 * （`sim/protocol.ts` の `OrderCommand` コメントを参照）。
 *
 * 位置を必要とする命令（移動・待機・退却・予備・築城）は、発行後いったん
 * 「地図をクリックして指定」の待機状態に入り、`handleMapClick` が呼ばれた
 * 時点で実際に命令を送る。対象部隊を必要とする命令（攻撃・突撃・側面・
 * 射撃・追撃）は、指揮系統リストの別ノードをクリックするまで待つ。
 * これにより「命令を出す→伝令が届くまで時間がかかる」体験を、憑依した
 * 1 ノードに対して繰り返し試せる。
 *
 * 統計は ~500ms おきに届く（`worker.ts` の STATS_INTERVAL_TICKS）ため、
 * ノード一覧は行を作り直さずテキストだけ更新する。丸ごと innerHTML を
 * 作り直すと、クリック操作の最中に要素が入れ替わって取りこぼす
 * （実際に Playwright のクリックが延々リトライする形で発覚した）。
 */

import type { CommandNode } from "../render/command-overlay";
import type { OrderCommand, ToWorker } from "../sim/protocol";
import { t } from "../i18n";

type PendingKind = "position" | "positionPair" | "targetNode";

interface PendingOrder {
  kind: PendingKind;
  node: number;
  make: (arg: { xM: number; yM: number }[] | number, own: CommandNode | undefined) => OrderCommand;
}

const STRUCTURE_KINDS = ["Stakes", "Ditch", "Abatis", "Rampart", "Palisade"];
const COMMAND_STATE_NAMES = ["指揮下", "指揮官不在", "継承中"];

function bradTo(fromXM: number, fromYM: number, toXM: number, toYM: number): number {
  const dx = toXM - fromXM;
  const dy = toYM - fromYM;
  let a = Math.atan2(dx, dy);
  if (a < 0) a += Math.PI * 2;
  return Math.round((a / (Math.PI * 2)) * 65536) & 0xffff;
}

function nodeRowText(n: CommandNode): string {
  return (
    `#${n.id} 陣営${n.faction} ${COMMAND_STATE_NAMES[n.commandState] ?? "?"} ` +
    `生存${n.alive} 崩壊${n.broken} 撃破${n.dead} 士気${n.avgMorale}`
  );
}

export class OrderPanel {
  possessedNode: number | null = null;
  private pending: PendingOrder | null = null;
  private pointBuffer: { xM: number; yM: number }[] = [];
  private nodes: CommandNode[] = [];
  private structureKind = 0;

  private readonly treeTitle: HTMLDivElement;
  private readonly listEl: HTMLDivElement;
  private readonly rows = new Map<number, HTMLDivElement>();
  private readonly controlsSlot: HTMLDivElement;
  private readonly hintSlot: HTMLDivElement;

  constructor(
    container: HTMLDivElement,
    private send: (msg: ToWorker) => void,
    private onChange: () => void,
    private onRequestCommanderDetail: (node: number) => void,
  ) {
    container.style.pointerEvents = "auto";
    container.innerHTML = "";

    this.treeTitle = document.createElement("div");
    this.treeTitle.className = "panel-title";
    this.treeTitle.textContent = t("commandTree");
    container.appendChild(this.treeTitle);

    this.listEl = document.createElement("div");
    this.listEl.className = "node-list";
    container.appendChild(this.listEl);

    this.controlsSlot = document.createElement("div");
    container.appendChild(this.controlsSlot);

    this.hintSlot = document.createElement("div");
    container.appendChild(this.hintSlot);
  }

  /** 統計が届くたびに呼ぶ。行のテキスト/クラスだけ更新し、要素は作り直さない。 */
  update(nodes: CommandNode[]): void {
    this.nodes = nodes;
    this.treeTitle.textContent = t("commandTree");

    const seen = new Set<number>();
    for (const n of nodes) {
      seen.add(n.id);
      let row = this.rows.get(n.id);
      if (!row) {
        row = document.createElement("div");
        row.className = "node-row";
        row.onclick = () => this.onNodeClick(n.id);
        this.rows.set(n.id, row);
        this.listEl.appendChild(row);
      }
      row.textContent = nodeRowText(n);
      row.classList.toggle("possessed", n.id === this.possessedNode);
      row.classList.toggle("pickable", this.pending?.kind === "targetNode");
    }
    for (const [id, row] of this.rows) {
      if (!seen.has(id)) {
        row.remove();
        this.rows.delete(id);
      }
    }
  }

  /** 言語切替時に呼ぶ。ノード一覧はテキストのみ、命令パネルは作り直す。 */
  refreshLang(): void {
    this.update(this.nodes);
    this.renderControls();
    this.renderHint();
  }

  /** 地図（ワールド座標）クリック。位置指定待ちの命令があれば消化する。 */
  handleMapClick(xM: number, yM: number): boolean {
    if (!this.pending) return false;
    const own = this.nodeById(this.pending.node);
    if (this.pending.kind === "position") {
      this.dispatch(this.pending.make([{ xM, yM }], own));
      this.clearPending();
      return true;
    }
    if (this.pending.kind === "positionPair") {
      this.pointBuffer.push({ xM, yM });
      if (this.pointBuffer.length >= 2) {
        this.dispatch(this.pending.make(this.pointBuffer, own));
        this.clearPending();
      } else {
        this.renderHint();
      }
      return true;
    }
    return false;
  }

  private dispatch(cmd: OrderCommand): void {
    this.send(cmd);
  }

  private clearPending(): void {
    this.pending = null;
    this.pointBuffer = [];
    for (const row of this.rows.values()) row.classList.remove("pickable");
    this.renderHint();
  }

  private nodeById(id: number): CommandNode | undefined {
    return this.nodes.find((n) => n.id === id);
  }

  private startPending(p: PendingOrder): void {
    this.pending = p;
    this.pointBuffer = [];
    for (const [id, row] of this.rows) {
      row.classList.toggle("pickable", p.kind === "targetNode" && id !== p.node);
    }
    this.renderHint();
  }

  private onNodeClick(id: number): void {
    if (this.pending?.kind === "targetNode") {
      this.dispatch(this.pending.make(id, this.nodeById(this.pending.node)));
      this.clearPending();
      return;
    }
    this.possessedNode = this.possessedNode === id ? null : id;
    for (const [rowId, row] of this.rows) row.classList.toggle("possessed", rowId === this.possessedNode);
    this.onChange();
    this.renderControls();
  }

  private renderHint(): void {
    const c = this.hintSlot;
    c.innerHTML = "";
    if (!this.pending) return;
    const hint = document.createElement("div");
    hint.className = "pick-hint";
    hint.textContent =
      this.pending.kind === "targetNode"
        ? t("pickTargetNode")
        : this.pending.kind === "positionPair"
          ? `${t("pickPositionPair")} (${this.pointBuffer.length}/2)`
          : t("pickPosition");
    const cancel = document.createElement("button");
    cancel.textContent = t("cancel");
    cancel.onclick = () => this.clearPending();
    hint.appendChild(cancel);
    c.appendChild(hint);
  }

  private renderControls(): void {
    const c = this.controlsSlot;
    c.innerHTML = "";
    if (this.possessedNode === null) return;
    c.appendChild(this.buildOrderControls());
  }

  private buildOrderControls(): HTMLDivElement {
    const node = this.possessedNode!;
    const wrap = document.createElement("div");
    wrap.className = "order-controls";

    const title = document.createElement("div");
    title.className = "panel-title";
    title.textContent = t("possessing", { node });
    wrap.appendChild(title);

    const detailBtn = document.createElement("button");
    detailBtn.textContent = t("commanderDetail");
    detailBtn.onclick = () => this.onRequestCommanderDetail(node);
    wrap.appendChild(detailBtn);

    const btn = (labelKey: Parameters<typeof t>[0], onClick: () => void): HTMLButtonElement => {
      const b = document.createElement("button");
      b.textContent = t(labelKey);
      b.onclick = onClick;
      wrap.appendChild(b);
      return b;
    };

    btn("orderMove", () =>
      this.startPending({
        kind: "position",
        node,
        make: (pts, own) => {
          const p = (pts as { xM: number; yM: number }[])[0]!;
          const ox = own?.centroidXM ?? p.xM;
          const oy = own?.centroidYM ?? p.yM;
          return {
            type: "issueMoveTo",
            node,
            xM: p.xM,
            yM: p.yM,
            facingBrad: bradTo(ox, oy, p.xM, p.yM),
            formation: own?.formation ?? 0,
          };
        },
      }),
    );

    btn("orderHold", () =>
      this.startPending({
        kind: "position",
        node,
        make: (pts, own) => {
          const p = (pts as { xM: number; yM: number }[])[0]!;
          const ox = own?.centroidXM ?? p.xM;
          const oy = own?.centroidYM ?? p.yM;
          return {
            type: "issueHold",
            node,
            xM: p.xM,
            yM: p.yM,
            facingBrad: bradTo(ox, oy, p.xM, p.yM),
            allowPursuit: true,
          };
        },
      }),
    );

    btn("orderAttack", () =>
      this.startPending({
        kind: "targetNode",
        node,
        make: (target) => ({ type: "issueAttack", node, targetNode: target as number, approach: 0 }),
      }),
    );

    btn("orderCharge", () =>
      this.startPending({
        kind: "targetNode",
        node,
        make: (target) => ({ type: "issueCharge", node, targetNode: target as number }),
      }),
    );

    btn("orderFlankLeft", () =>
      this.startPending({
        kind: "targetNode",
        node,
        make: (target) => ({ type: "issueFlank", node, targetNode: target as number, sideRight: false }),
      }),
    );

    btn("orderFlankRight", () =>
      this.startPending({
        kind: "targetNode",
        node,
        make: (target) => ({ type: "issueFlank", node, targetNode: target as number, sideRight: true }),
      }),
    );

    btn("orderWithdraw", () =>
      this.startPending({
        kind: "position",
        node,
        make: (pts) => {
          const p = (pts as { xM: number; yM: number }[])[0]!;
          return { type: "issueWithdraw", node, xM: p.xM, yM: p.yM, fighting: false };
        },
      }),
    );

    btn("orderShoot", () =>
      this.startPending({
        kind: "targetNode",
        node,
        make: (target) => ({ type: "issueShootAt", node, targetNode: target as number, mode: 0 }),
      }),
    );

    btn("orderReserve", () =>
      this.startPending({
        kind: "position",
        node,
        make: (pts) => {
          const p = (pts as { xM: number; yM: number }[])[0]!;
          return { type: "issueReserve", node, xM: p.xM, yM: p.yM };
        },
      }),
    );

    btn("orderPursue", () =>
      this.startPending({
        kind: "targetNode",
        node,
        make: (target) => ({ type: "issuePursue", node, targetNode: target as number, maxDistanceM: 60 }),
      }),
    );

    const engRow = document.createElement("div");
    engRow.className = "order-row";
    const select = document.createElement("select");
    for (let i = 0; i < STRUCTURE_KINDS.length; i++) {
      const opt = document.createElement("option");
      opt.value = String(i);
      opt.textContent = STRUCTURE_KINDS[i]!;
      select.appendChild(opt);
    }
    select.value = String(this.structureKind);
    select.onchange = () => {
      this.structureKind = Number(select.value);
    };
    engRow.appendChild(select);
    const engBtn = document.createElement("button");
    engBtn.textContent = t("orderEngineer");
    engBtn.onclick = () =>
      this.startPending({
        kind: "positionPair",
        node,
        make: (pts, own) => {
          const [a, b] = pts as { xM: number; yM: number }[];
          return {
            type: "queueBuildStructure",
            kind: this.structureKind,
            axM: a!.xM,
            ayM: a!.yM,
            bxM: b!.xM,
            byM: b!.yM,
            owner: own?.faction ?? 0,
            priority: 5,
          };
        },
      });
    engRow.appendChild(engBtn);
    wrap.appendChild(engRow);

    return wrap;
  }
}
