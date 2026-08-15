/**
 * メインスレッドと Sim Worker のメッセージ定義。
 *
 * 境界を跨ぐ呼び出しはフレームあたり定数回に抑える。
 * エンティティごとのメッセージは送らない（仕様 01 章 4 節）。
 */

export interface TerrainPayload {
  dim: number;
  cellM: number;
  sizeM: number;
  /** Uint8Array（地表タイプ）の中身 */
  surface: ArrayBuffer;
  /** Int16Array（標高 cm）の中身 */
  height: ArrayBuffer;
}

export type ToWorker =
  | { type: "init"; seed: number; sizeM: number; relief: number }
  | {
      type: "deploy";
      xM: number;
      yM: number;
      files: number;
      ranks: number;
      spacingMm: number;
      faction: number;
      unitId: number;
      seedSalt: number;
    }
  | { type: "setFactionGoal"; faction: number; xM: number; yM: number }
  | { type: "setRunning"; running: boolean }
  | { type: "setSpeed"; speed: number }
  | { type: "recycleBuffer"; buffer: ArrayBuffer };

export type FromWorker =
  | {
      type: "ready";
      simVersion: number;
      snapshotVersion: number;
      soldierStride: number;
      terrain: TerrainPayload;
    }
  | { type: "snapshot"; tick: number; count: number; buffer: ArrayBuffer };
