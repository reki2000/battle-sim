/**
 * 地形グリッドのバイナリ表現。
 *
 * 生成は f64 で行うので、環境が違えば 1 cm ずれることがありうる。
 * **決定論はこの形式が担保する**——シナリオの地形は Node で 1 度だけ生成して
 * `data/terrain/*.bin` に固定し、Rust のテストと `sim-headless` はそれを読む。
 * ブラウザは同じ生成器をその場で回すが、シミュレーション本体（Rust）は
 * 受け取った整数グリッドだけを見るので、生成側の丸め差は結果に伝播しない。
 *
 * 形式は `crates/sim-terrain/src/fixture.rs` のデコーダと 1 対 1。
 * 並びを変えるときは `VERSION` を上げ、両方を直すこと。
 */

import type { BattleSiteCandidate, TerrainGrids } from "./types";

/** "BSTR"（little endian の u32）。 */
const MAGIC = 0x52545342;
export const VERSION = 1;

/** ヘッダの長さ（バイト）。16 bit グリッドが偶数境界に来るよう詰めてある。 */
const HEADER_BYTES = 28;
/** 会戦地候補 1 件のバイト数。 */
const SITE_BYTES = 20;

/**
 * グリッドをバイナリへ書き出す。
 *
 * 16 bit のグリッド（標高・水深）を先頭にまとめ、8 bit のグリッドを後ろに
 * 置く。こうしておけば、どの 16 bit 配列も偶数オフセットから始まるので、
 * 読み出し側はコピーなしで `Int16Array` のビューを張れる。
 */
export function encodeTerrain(t: TerrainGrids): ArrayBuffer {
  const n = t.dim * t.dim;
  const bytes = HEADER_BYTES + n * 4 + n * 5 + t.battleSites.length * SITE_BYTES;
  const buf = new ArrayBuffer(bytes);
  const view = new DataView(buf);

  view.setUint32(0, MAGIC, true);
  view.setUint16(4, VERSION, true);
  view.setUint16(6, 0, true); // 予約
  view.setUint32(8, t.dim, true);
  view.setUint32(12, t.cellM, true);
  view.setBigUint64(16, BigInt.asUintN(64, t.seed), true);
  view.setUint32(24, t.battleSites.length, true);

  let off = HEADER_BYTES;
  new Int16Array(buf, off, n).set(t.height);
  off += n * 2;
  new Uint16Array(buf, off, n).set(t.water);
  off += n * 2;
  new Uint8Array(buf, off, n).set(t.ground);
  off += n;
  new Uint8Array(buf, off, n).set(t.vegetation);
  off += n;
  new Uint8Array(buf, off, n).set(t.overlay);
  off += n;
  new Uint8Array(buf, off, n).set(t.waterKind);
  off += n;
  new Uint8Array(buf, off, n).set(t.moisture);
  off += n;

  for (const s of t.battleSites) {
    view.setInt32(off, s.xM, true);
    view.setInt32(off + 4, s.yM, true);
    view.setInt32(off + 8, s.score, true);
    view.setUint16(off + 12, s.passablePermille, true);
    view.setUint16(off + 14, s.asymmetryPermille, true);
    view.setUint16(off + 16, s.opennessPermille, true);
    view.setUint16(off + 18, s.bottleneckCount, true);
    off += SITE_BYTES;
  }

  return buf;
}

/**
 * バイナリからグリッドを復元する。
 *
 * 崖グリッドは標高から取り直す（保存しても標高と食い違いうるだけで、
 * 導出はセル 1 つあたり 4 回の比較しかかからない）。
 */
export function decodeTerrain(buf: ArrayBuffer, computeCliff: (h: Int16Array, dim: number) => Uint8Array): TerrainGrids {
  const view = new DataView(buf);
  if (view.getUint32(0, true) !== MAGIC) throw new Error("地形バイナリのマジックが違う");
  const version = view.getUint16(4, true);
  if (version !== VERSION) {
    throw new Error(`地形バイナリの版が違う: ${version}（期待は ${VERSION}）`);
  }
  const dim = view.getUint32(8, true);
  const cellM = view.getUint32(12, true);
  const seed = view.getBigUint64(16, true);
  const siteCount = view.getUint32(24, true);
  const n = dim * dim;

  const expected = HEADER_BYTES + n * 9 + siteCount * SITE_BYTES;
  if (buf.byteLength !== expected) {
    throw new Error(`地形バイナリの長さが合わない: ${buf.byteLength}（期待は ${expected}）`);
  }

  let off = HEADER_BYTES;
  const height = new Int16Array(buf.slice(off, off + n * 2));
  off += n * 2;
  const water = new Uint16Array(buf.slice(off, off + n * 2));
  off += n * 2;
  const take = (): Uint8Array => {
    const a = new Uint8Array(buf.slice(off, off + n));
    off += n;
    return a;
  };
  const ground = take();
  const vegetation = take();
  const overlay = take();
  const waterKind = take();
  const moisture = take();

  const battleSites: BattleSiteCandidate[] = [];
  for (let i = 0; i < siteCount; i++) {
    battleSites.push({
      xM: view.getInt32(off, true),
      yM: view.getInt32(off + 4, true),
      score: view.getInt32(off + 8, true),
      passablePermille: view.getUint16(off + 12, true),
      asymmetryPermille: view.getUint16(off + 14, true),
      opennessPermille: view.getUint16(off + 16, true),
      bottleneckCount: view.getUint16(off + 18, true),
    });
    off += SITE_BYTES;
  }

  return {
    dim,
    cellM,
    seed,
    height,
    ground,
    vegetation,
    overlay,
    water,
    waterKind,
    moisture,
    cliff: computeCliff(height, dim),
    battleSites,
  };
}
