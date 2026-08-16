/**
 * 地形の IndexedDB キャッシュ（M9: スケールと仕上げ）。
 *
 * 地形はシード・サイズ・起伏から決定論的に生成されるが、生成そのものに
 * CPU 時間がかかる（2000 m 四方で 250 ms 前後）。同じ (seed, sizeM, relief)
 * を再訪したときは、生成をスキップして `sim-wasm` の `fromCachedTerrain`
 * へそのまま渡せるよう、グリッドを丸ごと保存しておく。
 *
 * ワーカー内で完結する（`indexedDB` は Web Worker からも使える）。
 * 使えない環境（プライベートモード等）ではキャッシュ無しで動くだけなので、
 * 失敗はすべて握りつぶして null 相当を返す。
 */

/** キャッシュの中身が変わったら上げる（`sim-core::SIM_VERSION` とは独立）。 */
const TERRAIN_CACHE_VERSION = 1;
const DB_NAME = "battle-sim";
const STORE_NAME = "terrain";
const DB_VERSION = 1;

export interface CachedTerrain {
  dim: number;
  cellM: number;
  height: Int16Array;
  surface: Uint8Array;
  passability: Uint8Array;
  water: Uint8Array;
  waterKind: Uint8Array;
  cliff: Uint8Array;
  battleSitesFlat: number[];
}

export function terrainCacheKey(seed: number, sizeM: number, relief: number): string {
  return `${seed}:${sizeM}:${relief}:v${TERRAIN_CACHE_VERSION}`;
}

function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, DB_VERSION);
    req.onupgradeneeded = () => {
      if (!req.result.objectStoreNames.contains(STORE_NAME)) {
        req.result.createObjectStore(STORE_NAME);
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

export async function loadCachedTerrain(key: string): Promise<CachedTerrain | null> {
  try {
    const db = await openDb();
    return await new Promise((resolve, reject) => {
      const tx = db.transaction(STORE_NAME, "readonly");
      const req = tx.objectStore(STORE_NAME).get(key);
      req.onsuccess = () => resolve((req.result as CachedTerrain | undefined) ?? null);
      req.onerror = () => reject(req.error);
    });
  } catch {
    return null;
  }
}

export async function saveCachedTerrain(key: string, data: CachedTerrain): Promise<void> {
  try {
    const db = await openDb();
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction(STORE_NAME, "readwrite");
      tx.objectStore(STORE_NAME).put(data, key);
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error);
    });
  } catch {
    // 保存できなくても致命的ではない（次回また生成するだけ）
  }
}
