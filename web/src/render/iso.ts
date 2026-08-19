/**
 * クォータービューの座標変換とカメラ。
 *
 * 仕様は docs/spec/08-rendering.md の 1〜2 節。
 * 2:1 ダイメトリック投影。タイル 1 枚 = 2 m 四方。
 */

/** タイル 1 枚の一辺（m）。地形グリッドのセルと 1:1 で対応する。 */
export const TILE_M = 2;
/** zoom = 1 のときのタイル幅（px）。 */
export const TILE_W = 64;
/** zoom = 1 のときのタイル高（px）。 */
export const TILE_H = 32;
/** 標高 1 m あたりの画面上の持ち上げ（px、zoom = 1）。 */
export const Z_SCALE = 24;

/** zoom = 1 のときの水平 px/m。 */
export const PX_PER_M = TILE_W / TILE_M / 2; // 16

/** 描画 LOD。仕様 08 章 2.2 節の表に対応する。 */
export enum Lod {
  /** 至近: 全身の剛体パーツと関節アニメーション */
  Close = 0,
  /** 戦術: 部位数を減らしたアニメーションポリゴン */
  Tactical = 1,
  /** 部隊: 低頂点の人型ポリゴン */
  Unit = 2,
  /** 会戦: 三角錐マーカー */
  Battle = 3,
  /** 戦域: 間引いた三角形マーカー */
  Theater = 4,
}

/** px/m から LOD を決める。 */
export function lodForScale(pxPerM: number): Lod {
  if (pxPerM >= 48) return Lod.Close;
  if (pxPerM >= 12) return Lod.Tactical;
  if (pxPerM >= 3) return Lod.Unit;
  if (pxPerM >= 0.8) return Lod.Battle;
  return Lod.Theater;
}

export interface ScreenPoint {
  sx: number;
  sy: number;
}

export class Camera {
  /** 注視しているワールド座標（m）。 */
  centerX = 0;
  centerY = 0;
  /** 対数ズーム。実ズーム = 2^zoomLog2。 */
  zoomLog2 = 0;
  /** ビューポート（px）。 */
  viewW = 1;
  viewH = 1;

  /** ワールドの一辺（m）。パンの範囲制限に使う。 */
  worldSizeM = 5000;

  /** 実ズーム倍率。 */
  get zoom(): number {
    return Math.pow(2, this.zoomLog2);
  }

  /** 水平方向の px/m。 */
  get pxPerM(): number {
    return PX_PER_M * this.zoom;
  }

  /** 現在の LOD。 */
  get lod(): Lod {
    return lodForScale(this.pxPerM);
  }

  /** 画面に収まるワールドの横幅（m）。 */
  get viewWidthM(): number {
    return this.viewW / this.pxPerM;
  }

  /**
   * ズームの下限・上限。
   * 視野幅 5000 m 〜 10 m（仕様 08 章 2.1）を満たすように決める。
   */
  zoomBounds(): { min: number; max: number } {
    // 視野 5000 m のときの zoom
    const minZoom = this.viewW / (5000 * PX_PER_M);
    // 視野 10 m のときの zoom
    const maxZoom = this.viewW / (10 * PX_PER_M);
    return { min: Math.log2(minZoom), max: Math.log2(maxZoom) };
  }

  clampZoom(): void {
    const b = this.zoomBounds();
    this.zoomLog2 = Math.min(b.max, Math.max(b.min, this.zoomLog2));
  }

  /** ワールド座標（m）→ スクリーン座標（px）。 */
  worldToScreen(x: number, y: number, z = 0): ScreenPoint {
    const k = this.zoom;
    const cx = (this.centerX - this.centerY) * PX_PER_M * k;
    const cy = ((this.centerX + this.centerY) * (TILE_H / TILE_M / 2)) * k;
    return {
      sx: (x - y) * PX_PER_M * k - cx + this.viewW / 2,
      sy: ((x + y) * (TILE_H / TILE_M / 2) - z * Z_SCALE) * k - cy + this.viewH / 2,
    };
  }

  /**
   * スクリーン座標（px）→ ワールド座標（m）。標高 0 の平面との交点。
   *
   * 起伏のある地形では厳密には正しくないが、カメラ操作（ズーム中心の固定、
   * クリック位置の推定）には十分。
   */
  screenToWorld(sx: number, sy: number): { x: number; y: number } {
    const k = this.zoom;
    const cx = (this.centerX - this.centerY) * PX_PER_M * k;
    const cy = ((this.centerX + this.centerY) * (TILE_H / TILE_M / 2)) * k;
    const dx = (sx - this.viewW / 2 + cx) / (PX_PER_M * k);
    const dy = (sy - this.viewH / 2 + cy) / ((TILE_H / TILE_M / 2) * k);
    // dx = x - y, dy = x + y
    return { x: (dy + dx) / 2, y: (dy - dx) / 2 };
  }

  /** カーソル位置を固定したままズームする（地図アプリと同じ挙動）。 */
  zoomAt(sx: number, sy: number, deltaLog2: number): void {
    const before = this.screenToWorld(sx, sy);
    this.zoomLog2 += deltaLog2;
    this.clampZoom();
    const after = this.screenToWorld(sx, sy);
    this.centerX += before.x - after.x;
    this.centerY += before.y - after.y;
    this.clampToWorld();
  }

  /** スクリーン上の移動量ぶんパンする。 */
  panByScreen(dsx: number, dsy: number): void {
    const k = this.zoom;
    const dx = -dsx / (PX_PER_M * k);
    const dy = -dsy / ((TILE_H / TILE_M / 2) * k);
    this.centerX += (dy + dx) / 2;
    this.centerY += (dy - dx) / 2;
    this.clampToWorld();
  }

  /** カメラがワールドから離れすぎないようにする。 */
  clampToWorld(): void {
    const m = this.worldSizeM * 0.1;
    this.centerX = Math.min(this.worldSizeM + m, Math.max(-m, this.centerX));
    this.centerY = Math.min(this.worldSizeM + m, Math.max(-m, this.centerY));
  }

  /** 指定の視野幅（m）になるようズームを合わせる。 */
  setViewWidthM(meters: number): void {
    this.zoomLog2 = Math.log2(this.viewW / (meters * PX_PER_M));
    this.clampZoom();
  }
}
