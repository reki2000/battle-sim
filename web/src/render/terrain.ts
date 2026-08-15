/**
 * 地形の描画。
 *
 * M0 は Canvas2D の疎通確認版。地表グリッドを 1 セル = 1 px のビットマップに
 * 焼き、アイソメトリック変換（線形写像なので setTransform で厳密に表せる）で
 * 貼る。標高はヒルシェードとして色に混ぜる。
 *
 * M1 で WebGL2 のタイルマップ描画に置き換える（仕様 08 章 3 節）。
 * そのとき、ここで確認した座標変換とヒルシェードの考え方はそのまま使う。
 */

import { Camera, TILE_H, TILE_M, PX_PER_M } from "./iso";

/** 地表タイプごとの基準色。sim-terrain の Surface と同じ順序。 */
const SURFACE_COLORS: ReadonlyArray<readonly [number, number, number]> = [
  [26, 54, 92], // DeepWater
  [58, 110, 148], // ShallowWater
  [96, 140, 160], // Ford
  [74, 92, 66], // Marsh
  [104, 88, 64], // Mud
  [104, 132, 74], // Grass
  [122, 148, 82], // Meadow
  [154, 142, 90], // Farmland
  [110, 116, 70], // Scrub
  [64, 100, 58], // LightForest
  [40, 72, 44], // DenseForest
  [130, 126, 118], // Rock
  [146, 138, 124], // Scree
  [198, 182, 138], // Sand
  [150, 132, 104], // Road
  [128, 100, 72], // Bridge
];

export interface TerrainData {
  dim: number;
  cellM: number;
  sizeM: number;
  surface: Uint8Array;
  height: Int16Array;
}

export class TerrainRenderer {
  private bitmap: HTMLCanvasElement | null = null;
  private data: TerrainData | null = null;

  /**
   * 地形ビットマップを焼く。地表色にヒルシェードを掛ける。
   *
   * dim×dim ピクセル。5 km / 2 m = 2500² = 6.25 M px なので数十 ms かかるが、
   * 地形が変わったときだけでよい。
   */
  setTerrain(data: TerrainData): void {
    this.data = data;
    const { dim, surface, height, cellM } = data;

    const canvas = document.createElement("canvas");
    canvas.width = dim;
    canvas.height = dim;
    const ctx = canvas.getContext("2d", { willReadFrequently: false })!;
    const img = ctx.createImageData(dim, dim);
    const px = img.data;

    // 北西からの平行光でヒルシェードを作る
    const cellCm = cellM * 100;
    for (let y = 0; y < dim; y++) {
      for (let x = 0; x < dim; x++) {
        const i = y * dim + x;
        const c = SURFACE_COLORS[surface[i]! & 0x0f]!;

        const hx = height[y * dim + Math.min(dim - 1, x + 1)]! - height[i]!;
        const hy = height[Math.min(dim - 1, y + 1) * dim + x]! - height[i]!;
        // 勾配（無次元）から明度係数へ。±0.5 の勾配で ±35% 程度
        const shade = 1 + (-(hx + hy) / cellCm) * 0.7;
        const s = Math.max(0.45, Math.min(1.5, shade));

        const o = i * 4;
        px[o] = Math.min(255, c[0] * s);
        px[o + 1] = Math.min(255, c[1] * s);
        px[o + 2] = Math.min(255, c[2] * s);
        px[o + 3] = 255;
      }
    }
    ctx.putImageData(img, 0, 0);
    this.bitmap = canvas;
  }

  /**
   * 地形を描く。
   *
   * アイソメトリック投影 (x,y) -> ((x-y)·P, (x+y)·Q) は線形写像なので、
   * ビットマップのピクセル座標からスクリーン座標への変換を
   * setTransform でそのまま表現できる。
   */
  draw(ctx: CanvasRenderingContext2D, cam: Camera): void {
    if (!this.bitmap || !this.data) return;
    const { cellM } = this.data;
    const k = cam.zoom;
    const P = PX_PER_M * k; // 水平係数
    const Q = (TILE_H / TILE_M / 2) * k; // 垂直係数

    const origin = cam.worldToScreen(0, 0);

    ctx.save();
    // ピクセル (cx, cy) は世界座標 (cx·cell, cy·cell) に対応する
    ctx.setTransform(
      cellM * P,
      cellM * Q,
      -cellM * P,
      cellM * Q,
      origin.sx,
      origin.sy,
    );
    // 拡大時はセルの輪郭が見える方が地形を読みやすい
    ctx.imageSmoothingEnabled = cam.pxPerM < 8;
    ctx.drawImage(this.bitmap, 0, 0);
    ctx.restore();
  }

  /** ワールド座標の標高（m）。兵士を地面に置くために使う。 */
  heightAt(xM: number, yM: number): number {
    if (!this.data) return 0;
    const { dim, cellM, height } = this.data;
    const cx = Math.max(0, Math.min(dim - 1, Math.floor(xM / cellM)));
    const cy = Math.max(0, Math.min(dim - 1, Math.floor(yM / cellM)));
    return height[cy * dim + cx]! / 100;
  }
}
