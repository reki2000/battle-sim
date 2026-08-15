/**
 * 兵士の描画。
 *
 * M0 は Canvas2D で LOD ごとに単純な図形を描く疎通確認版。
 * M2 で WebGL2 のインスタンシング（1 ドローコールで 5 万体）と
 * 手続き生成スプライトに置き換える（仕様 08 章 4 節）。
 *
 * LOD の段階分けと「ズームアウトしても個体は個体のまま」という原則は
 * ここでも守る。描画を簡略化するだけで、間引きは行わない。
 */

import { Camera, Lod } from "./iso";
import type { InterpolatedPositions, SnapshotView } from "../sim/snapshot";
import { SoldierState } from "../sim/snapshot";

/** 陣営色。色覚多様性に配慮し、赤/青ではなく青/橙にする（仕様 09 章 9 節）。 */
const FACTION_COLORS = ["#3d7ab8", "#d98032"];

export class SoldierRenderer {
  /** 直前のフレームで実際に描いた数（デバッグ表示用）。 */
  drawn = 0;

  draw(
    ctx: CanvasRenderingContext2D,
    cam: Camera,
    snap: SnapshotView,
    interp: InterpolatedPositions,
    alpha: number,
    groundHeight: (x: number, y: number) => number,
  ): void {
    const n = snap.count;
    if (n === 0) {
      this.drawn = 0;
      return;
    }

    const lod = cam.lod;
    const pxPerM = cam.pxPerM;
    // 兵士の見かけの大きさ。肩幅 0.7 m を基準にする
    const size = Math.max(1, 0.7 * pxPerM);

    // NOTE: 本来アイソメトリックの描画順は x+y の昇順。M0 では配列順のまま
    // 描いており、重なりの前後が正しくない場合がある。M2 で WebGL2 に移り、
    // 深度バッファに x+y を詰めて解決する（仕様 08 章 1.1 節）。CPU ソートはしない。
    // 画面外は描かない（視錐台カリング）
    const margin = size * 4;
    let drawn = 0;

    ctx.save();
    for (let i = 0; i < n; i++) {
      const x = interp.x(i, alpha);
      const y = interp.y(i, alpha);
      const z = groundHeight(x, y);
      const p = cam.worldToScreen(x, y, z);
      if (
        p.sx < -margin ||
        p.sy < -margin ||
        p.sx > cam.viewW + margin ||
        p.sy > cam.viewH + margin
      ) {
        continue;
      }

      const state = snap.state(i);
      const faction = snap.unitId(i) % FACTION_COLORS.length;

      if (state === SoldierState.Dead || state === SoldierState.Downed) {
        ctx.fillStyle = "rgba(60,40,40,0.65)";
        ctx.fillRect(p.sx - size / 2, p.sy - size / 4, size, size / 2);
        drawn++;
        continue;
      }

      ctx.fillStyle = FACTION_COLORS[faction]!;

      if (lod >= Lod.Battle) {
        // 会戦・戦域: 点
        ctx.fillRect(p.sx, p.sy, Math.max(1, size), Math.max(1, size));
      } else if (lod === Lod.Unit) {
        // 部隊: 小さなクアッド
        ctx.fillRect(p.sx - size / 2, p.sy - size / 2, size, size);
      } else {
        // 戦術・至近: 影 + 胴 + 向きの表示
        ctx.save();
        ctx.globalAlpha = 0.28;
        ctx.fillStyle = "#000";
        ctx.beginPath();
        ctx.ellipse(p.sx, p.sy, size * 0.55, size * 0.28, 0, 0, Math.PI * 2);
        ctx.fill();
        ctx.restore();

        const bodyH = 1.7 * pxPerM * 0.55; // 見かけの背丈（投影で縮む）
        ctx.fillStyle = FACTION_COLORS[faction]!;
        ctx.fillRect(p.sx - size * 0.35, p.sy - bodyH, size * 0.7, bodyH);

        // 頭
        ctx.fillStyle = "#d9c9a8";
        const headR = size * 0.24;
        ctx.beginPath();
        ctx.arc(p.sx, p.sy - bodyH - headR * 0.6, headR, 0, Math.PI * 2);
        ctx.fill();

        if (lod === Lod.Close) {
          // 向き（武器の方向）
          const a = snap.facingRad(i);
          const dx = Math.cos(a) - Math.sin(a);
          const dy = (Math.cos(a) + Math.sin(a)) * 0.5;
          ctx.strokeStyle = "#c8c0b0";
          ctx.lineWidth = Math.max(1, size * 0.12);
          ctx.beginPath();
          ctx.moveTo(p.sx, p.sy - bodyH * 0.6);
          ctx.lineTo(p.sx + dx * size * 1.4, p.sy - bodyH * 0.6 + dy * size * 1.4);
          ctx.stroke();
        }
      }
      drawn++;
    }
    ctx.restore();
    this.drawn = drawn;
  }
}

export { FACTION_COLORS };
