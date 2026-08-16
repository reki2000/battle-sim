/**
 * ミニマップ。
 *
 * 仕様 09 章 6 節。地形の縮小サムネイル（真上からの投影、アイソメトリック
 * ではない）の上に、両軍の兵士を点として重ねる。地形が変わったときにだけ
 * サムネイルを焼き直し、兵士の点は毎フレーム軽量に打ち直す。
 *
 * 独立した `<canvas>` 要素は使わない。兵士を描いている既存のオーバーレイ
 * （Canvas2D、透明、WebGL の地形キャンバスの上に重なっている）の隅に
 * そのまま描き込む。理由は実装上の都合ではなく回避策: 別キャンバス要素を
 * `position: fixed`/`absolute` で重ねると、この環境（ソフトウェアレンダリング
 * の Chromium）ではコンポジタが上下の配置を取り違え、指定した位置と
 * 上下対称の位置に幽霊描画が出るというバグを踏んだ。単一のオーバーレイ
 * キャンバスに描く分には問題が起きないため、この形にしてある。
 */

import { SURFACE_COLORS } from "./surface-colors";
import type { TerrainData } from "../sim/terrain-data";
import type { Camera } from "./iso";
import type { InterpolatedPositions, SnapshotView } from "../sim/snapshot";
import { FACTION_COLORS } from "./soldiers";
import { getQuality } from "../quality";

/** ミニマップの一辺（CSS px）。 */
const SIZE_PX = 200;
/** 画面端からの余白（CSS px）。 */
const MARGIN_PX = 12;

export class MinimapRenderer {
  private thumb: HTMLCanvasElement | null = null;
  private terrain: TerrainData | null = null;

  setTerrain(data: TerrainData): void {
    this.terrain = data;

    // サムネイルはミニマップの解像度に直接間引いて焼く（テクスチャとして
    // 使うわけではないので、間引きに上限テクスチャサイズの制約はない）。
    const thumb = document.createElement("canvas");
    thumb.width = SIZE_PX;
    thumb.height = SIZE_PX;
    const tctx = thumb.getContext("2d")!;
    const img = tctx.createImageData(SIZE_PX, SIZE_PX);

    for (let py = 0; py < SIZE_PX; py++) {
      const gy = Math.min(data.dim - 1, Math.floor((py / SIZE_PX) * data.dim));
      for (let px = 0; px < SIZE_PX; px++) {
        const gx = Math.min(data.dim - 1, Math.floor((px / SIZE_PX) * data.dim));
        const i = gy * data.dim + gx;
        const c = SURFACE_COLORS[data.surface[i]! & 0x0f]!;
        const o = (py * SIZE_PX + px) * 4;
        img.data[o] = c[0];
        img.data[o + 1] = c[1];
        img.data[o + 2] = c[2];
        img.data[o + 3] = 255;
      }
    }
    tctx.putImageData(img, 0, 0);
    this.thumb = thumb;
  }

  /**
   * `ctx` の右下隅に描く。`ctx` は画面いっぱいのオーバーレイキャンバスの
   * デバイスピクセル座標系（すでに DPR ぶん拡大されている）を想定する。
   */
  draw(
    ctx: CanvasRenderingContext2D,
    viewW: number,
    viewH: number,
    dpr: number,
    cam: Camera,
    snap: SnapshotView,
    interp: InterpolatedPositions,
  ): void {
    if (!this.thumb || !this.terrain) return;
    const size = SIZE_PX * dpr;
    const margin = MARGIN_PX * dpr;
    const ox = viewW - size - margin;
    const oy = viewH - size - margin;

    ctx.save();
    ctx.translate(ox, oy);

    ctx.fillStyle = "#0d1014";
    ctx.fillRect(-1, -1, size + 2, size + 2);
    ctx.drawImage(this.thumb, 0, 0, size, size);

    const world = this.terrain.sizeM;
    const n = snap.count;
    const stride = Math.max(1, Math.floor(n / getQuality().thinTargetMinimapDots));
    for (let i = 0; i < n; i += stride) {
      const x = (interp.x(i, 1) / world) * size;
      const y = (interp.y(i, 1) / world) * size;
      const faction = snap.faction(i) % FACTION_COLORS.length;
      ctx.fillStyle = FACTION_COLORS[faction]!;
      ctx.fillRect(x - dpr, y - dpr, 2 * dpr, 2 * dpr);
    }

    // 現在の視野範囲を枠で示す
    const halfW = (cam.viewWidthM / 2 / world) * size;
    const cx = (cam.centerX / world) * size;
    const cy = (cam.centerY / world) * size;
    ctx.strokeStyle = "rgba(255,255,255,0.7)";
    ctx.lineWidth = dpr;
    ctx.strokeRect(cx - halfW, cy - halfW, halfW * 2, halfW * 2);

    ctx.strokeStyle = "rgba(255,255,255,0.25)";
    ctx.strokeRect(0, 0, size, size);
    ctx.restore();
  }
}
