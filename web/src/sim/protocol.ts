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
  /** Uint8Array（水深、10 cm 単位）の中身 */
  water: ArrayBuffer;
  /** Uint8Array（WaterKind）の中身 */
  waterKind: ArrayBuffer;
  /** Uint8Array（崖ビットマスク）の中身 */
  cliff: ArrayBuffer;
  /**
   * 会戦地候補。1 件あたり
   * [xM, yM, score, passablePermille, asymmetryPermille, opennessPermille, bottleneckCount]
   * の平坦な配列（sim-wasm の battleSites() と同じレイアウト）。
   */
  battleSites: number[];
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
      troopType: number;
      seedSalt: number;
    }
  | { type: "setFactionGoal"; faction: number; xM: number; yM: number }
  | {
      type: "addLineUnit";
      faction: number;
      firstId: number;
      count: number;
      ranks: number;
      formation: number;
    }
  | {
      type: "issueMoveTo";
      node: number;
      xM: number;
      yM: number;
      facingBrad: number;
      formation: number;
    }
  | { type: "setRunning"; running: boolean }
  | { type: "setSpeed"; speed: number }
  | { type: "recycleBuffer"; buffer: ArrayBuffer };

/**
 * M3/M4 の集計データ。毎ティックではなく間引いて送る（仕様 01 章 4 節：
 * 境界を跨ぐ呼び出しはフレームあたり定数回に抑える）。
 * レイアウトは `sim-wasm` の `combatStats`/`commandNodes`/`commandEvents`/
 * `messengers` と同じフラット配列。
 */
export interface StatsPayload {
  combat: number[];
  nodes: number[];
  commandEvents: number[];
  messengers: number[];
}

export type FromWorker =
  | {
      type: "ready";
      simVersion: number;
      snapshotVersion: number;
      soldierStride: number;
      terrain: TerrainPayload;
    }
  | {
      type: "snapshot";
      tick: number;
      count: number;
      buffer: ArrayBuffer;
      stats?: StatsPayload;
    };
