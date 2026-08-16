/**
 * 指揮系統ツリー・命令の可視化と、戦闘統計の簡易グラフ（M3/M4）。
 *
 * データは `sim-wasm` の `combatStats`/`commandNodes`/`messengers` と同じ
 * フラット配列レイアウトで届く（`sim/protocol.ts` の `StatsPayload`）。
 * ここでその場でパースして描画する。低頻度（`STATS_INTERVAL_TICKS` おき）
 * にしか届かないので、パース自体のコストは無視できる。
 */

import type { Camera } from "./iso";
import type { StatsPayload } from "../sim/protocol";

export interface CombatStats {
  attacks: number;
  hits: number;
  damage: number;
  kills: number;
  downed: number;
  pursuitKills: number;
  meleeKills: number;
  missileKills: number;
  crushKills: number;
  bleedKills: number;
  shotsFired: number;
  friendlyFireHits: number;
  /** 以下 M5（騎兵）。 */
  chargeKills: number;
  dismounts: number;
  horseRefusals: number;
}

export function parseCombatStats(flat: number[]): CombatStats {
  const [
    attacks = 0,
    hits = 0,
    damage = 0,
    kills = 0,
    downed = 0,
    pursuitKills = 0,
    meleeKills = 0,
    missileKills = 0,
    crushKills = 0,
    bleedKills = 0,
    shotsFired = 0,
    friendlyFireHits = 0,
    chargeKills = 0,
    dismounts = 0,
    horseRefusals = 0,
  ] = flat;
  return {
    attacks,
    hits,
    damage,
    kills,
    downed,
    pursuitKills,
    meleeKills,
    missileKills,
    crushKills,
    bleedKills,
    shotsFired,
    friendlyFireHits,
    chargeKills,
    dismounts,
    horseRefusals,
  };
}

export interface CommandNode {
  id: number;
  parent: number;
  echelon: number;
  faction: number;
  commanderId: number;
  commandState: number;
  hasUnit: boolean;
  formation: number;
  alive: number;
  downed: number;
  dead: number;
  broken: number;
  avgMorale: number;
  avgFatigue: number;
  centroidXM: number;
  centroidYM: number;
}

const NODE_STRIDE = 16;

export function parseCommandNodes(flat: number[]): CommandNode[] {
  const out: CommandNode[] = [];
  for (let i = 0; i + NODE_STRIDE <= flat.length; i += NODE_STRIDE) {
    out.push({
      id: flat[i]!,
      parent: flat[i + 1]!,
      echelon: flat[i + 2]!,
      faction: flat[i + 3]!,
      commanderId: flat[i + 4]!,
      commandState: flat[i + 5]!,
      hasUnit: flat[i + 6]! !== 0,
      formation: flat[i + 7]!,
      alive: flat[i + 8]!,
      downed: flat[i + 9]!,
      dead: flat[i + 10]!,
      broken: flat[i + 11]!,
      avgMorale: flat[i + 12]!,
      avgFatigue: flat[i + 13]!,
      centroidXM: flat[i + 14]! / 100,
      centroidYM: flat[i + 15]! / 100,
    });
  }
  return out;
}

export interface Messenger {
  fromNode: number;
  toNode: number;
  posXM: number;
  posYM: number;
  destXM: number;
  destYM: number;
  state: number;
  method: number;
}

const MESSENGER_STRIDE = 8;

export function parseMessengers(flat: number[]): Messenger[] {
  const out: Messenger[] = [];
  for (let i = 0; i + MESSENGER_STRIDE <= flat.length; i += MESSENGER_STRIDE) {
    out.push({
      fromNode: flat[i]!,
      toNode: flat[i + 1]!,
      posXM: flat[i + 2]! / 100,
      posYM: flat[i + 3]! / 100,
      destXM: flat[i + 4]! / 100,
      destYM: flat[i + 5]! / 100,
      state: flat[i + 6]!,
      method: flat[i + 7]!,
    });
  }
  return out;
}

export interface ParsedStats {
  combat: CombatStats;
  nodes: CommandNode[];
  messengers: Messenger[];
}

export function parseStats(payload: StatsPayload): ParsedStats {
  return {
    combat: parseCombatStats(payload.combat),
    nodes: parseCommandNodes(payload.nodes),
    messengers: parseMessengers(payload.messengers),
  };
}

const METHOD_COLORS = ["#e8c96a", "#6ac9e8", "#e86a9c"]; // Messenger / Flag / Acoustic
const MESSENGER_STATE_COLORS = ["#e8c96a", "#e8c96a", "#7ad17a", "#c94f4f"]; // Riding/Delivering/Delivered/Lost

/** 命令の可視化: 発令元ノードから伝令の現在位置へ線を引き、先端に伝令の点を描く。 */
export function drawOrderVisualization(
  ctx: CanvasRenderingContext2D,
  cam: Camera,
  nodes: CommandNode[],
  messengers: Messenger[],
): void {
  if (messengers.length === 0) return;
  const byId = new Map(nodes.map((n) => [n.id, n]));
  ctx.save();
  ctx.lineWidth = 1.5;
  for (const m of messengers) {
    const from = byId.get(m.fromNode);
    if (!from) continue;
    const p1 = cam.worldToScreen(from.centroidXM, from.centroidYM);
    const p2 = cam.worldToScreen(m.posXM, m.posYM);
    const color = METHOD_COLORS[m.method] ?? "#e8c96a";
    ctx.strokeStyle = color;
    ctx.globalAlpha = 0.7;
    ctx.beginPath();
    ctx.moveTo(p1.sx, p1.sy);
    ctx.lineTo(p2.sx, p2.sy);
    ctx.stroke();
    ctx.globalAlpha = 1;
    ctx.fillStyle = MESSENGER_STATE_COLORS[m.state] ?? color;
    ctx.beginPath();
    ctx.arc(p2.sx, p2.sy, 3.5, 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.restore();
}

const CAUSE_LABELS = ["追撃", "白兵", "射撃", "圧迫", "出血"] as const;
const CAUSE_COLORS = ["#e07a3f", "#3d7ab8", "#d9b23d", "#8a5ac9", "#8c8c8c"];

/** 死因内訳の帯グラフ。M4 受け入れ条件（追撃が最大の死因か）を一目で見るため。 */
export function drawDeathCauseGraph(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  stats: CombatStats,
): void {
  const values = [stats.pursuitKills, stats.meleeKills, stats.missileKills, stats.crushKills, stats.bleedKills];
  const total = values.reduce((s, v) => s + v, 0);
  ctx.save();
  ctx.fillStyle = "rgba(10,14,20,0.6)";
  ctx.fillRect(x, y, w, h);
  if (total > 0) {
    let cx = x;
    for (let i = 0; i < values.length; i++) {
      const cw = (values[i]! / total) * w;
      if (cw <= 0) continue;
      ctx.fillStyle = CAUSE_COLORS[i]!;
      ctx.fillRect(cx, y, cw, h);
      cx += cw;
    }
  }
  ctx.strokeStyle = "rgba(255,255,255,0.25)";
  ctx.strokeRect(x + 0.5, y + 0.5, w - 1, h - 1);
  ctx.restore();
}

export function deathCauseLegend(stats: CombatStats): string {
  const values = [stats.pursuitKills, stats.meleeKills, stats.missileKills, stats.crushKills, stats.bleedKills];
  const total = values.reduce((s, v) => s + v, 0);
  if (total === 0) return "死因: なし";
  return CAUSE_LABELS.map((label, i) => `${label} ${values[i]}`).join(" / ");
}

export function commandTreeSummary(nodes: CommandNode[]): string {
  const stateNames = ["指揮下", "指揮官不在", "継承中"];
  return nodes
    .map(
      (n) =>
        `#${n.id} 陣営${n.faction} ${stateNames[n.commandState] ?? "?"} ` +
        `生存${n.alive} 崩壊${n.broken} 撃破${n.dead} 士気${n.avgMorale}`,
    )
    .join("\n");
}
