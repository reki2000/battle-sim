/**
 * 決定論的なハッシュとノイズ場。
 *
 * シードは 64 bit。SplitMix64 でドメインごとの 32 bit 鍵に混ぜてから、
 * 座標アドレス方式の 32 bit ハッシュで密なノイズ場を作る。64 bit すべての
 * ビットがどのドメイン鍵にも影響するので、無関係な機能を 1 つ足しても
 * 共有の乱数列を消費せず、既存の地形はずれない。
 */

/** ドメイン分離用の定数。新しい用途を足すときは新しい値を使う。 */
export const Domain = {
  HEIGHT: 0x1111111111111111n,
  MOISTURE: 0x2222222222222222n,
  VEGETATION: 0x3333333333333333n,
  OBJECTS: 0x4444444444444444n,
  ROADS: 0x5555555555555555n,
} as const;

const U64 = (v: bigint): bigint => BigInt.asUintN(64, v);

export function mix64(x: bigint): bigint {
  let v = U64(x + 0x9e3779b97f4a7c15n);
  v = U64((v ^ (v >> 30n)) * 0xbf58476d1ce4e5b9n);
  v = U64((v ^ (v >> 27n)) * 0x94d049bb133111ebn);
  return U64(v ^ (v >> 31n));
}

/** シードを 64 bit に正規化する。10/16 進の文字列、数値、BigInt を受ける。 */
export function parseSeed(s: bigint | string | number): bigint {
  if (typeof s === "bigint") return U64(s);
  if (typeof s === "number") return U64(BigInt(Math.trunc(s)));
  const text = String(s).trim();
  try {
    return U64(BigInt(text.replace(/_/g, "") || "0"));
  } catch {
    // 数値として読めない文字列は、文字コードを順に混ぜてシードにする
    let x = 0n;
    for (const c of text) x = mix64(x ^ BigInt(c.codePointAt(0) ?? 0));
    return x;
  }
}

/** 64 bit シードとドメインから 32 bit の鍵を導く。 */
export function derive32(seed: bigint, domain: bigint, k = 0): number {
  const v = mix64(U64(seed) ^ domain ^ U64(BigInt(k) * 0xd6e8feb86659fd93n));
  return Number((v ^ (v >> 32n)) & 0xffffffffn) >>> 0;
}

/** 座標アドレス方式の 32 bit ハッシュ。ホットループ用。 */
export function hash32(seed32: number, x: number, z: number, k = 0): number {
  let h =
    (seed32 ^
      Math.imul(x | 0, 0x632be5ab) ^
      Math.imul(z | 0, 0x85157af5) ^
      Math.imul(k | 0, 0x9e3779b1)) >>>
    0;
  h ^= h >>> 16;
  h = Math.imul(h, 0x7feb352d) >>> 0;
  h ^= h >>> 15;
  h = Math.imul(h, 0x846ca68b) >>> 0;
  h ^= h >>> 16;
  return h >>> 0;
}

export function rand32(seed32: number, x: number, z: number, k = 0): number {
  return hash32(seed32, x, z, k) / 4294967296;
}

/** 決定論的な整数乱数列。道路の端点選びのような逐次の抽選に使う。 */
export class Stream {
  private state: number;

  constructor(seed32: number) {
    this.state = seed32 >>> 0;
  }

  next(): number {
    this.state = hash32(this.state, 1, 1, 0);
    return this.state;
  }

  /** `[lo, hi)` の整数。 */
  range(lo: number, hi: number): number {
    if (hi <= lo) return lo;
    return lo + (this.next() % (hi - lo));
  }
}

function fade(t: number): number {
  return t * t * t * (t * (t * 6 - 15) + 10);
}

function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t;
}

/**
 * 決まった波長の value noise 場。格子点を事前に埋めておき、
 * サンプルは 5 次の fade カーブによる双線形補間だけで済ませる。
 *
 * 格子は m 単位で定義されるので、同じ波長を与えればマップの大きさが
 * 変わっても特徴の大きさは変わらない。
 */
export class NoiseField {
  readonly wavelength: number;
  private readonly cols: number;
  private readonly rows: number;
  private readonly values: Float32Array;

  constructor(seed32: number, widthM: number, heightM: number, wavelength: number, octave = 0) {
    this.wavelength = wavelength;
    this.cols = Math.ceil(widthM / wavelength) + 2;
    this.rows = Math.ceil(heightM / wavelength) + 2;
    this.values = new Float32Array(this.cols * this.rows);
    for (let z = 0; z < this.rows; z++) {
      for (let x = 0; x < this.cols; x++) {
        this.values[z * this.cols + x] = rand32(seed32, x, z, octave) * 2 - 1;
      }
    }
  }

  /** ワールド座標（m）でサンプルする。戻り値はおおよそ -1..1。 */
  sample(x: number, z: number): number {
    const gx = x / this.wavelength;
    const gz = z / this.wavelength;
    let x0 = Math.floor(gx);
    let z0 = Math.floor(gz);
    const tx = fade(gx - x0);
    const tz = fade(gz - z0);
    // 端の外を読まないよう格子内へ丸める（格子は +2 の余裕を持たせてある）
    x0 = Math.max(0, Math.min(this.cols - 2, x0));
    z0 = Math.max(0, Math.min(this.rows - 2, z0));
    const c = this.cols;
    const v = this.values;
    const a = v[z0 * c + x0]!;
    const b = v[z0 * c + x0 + 1]!;
    const d = v[(z0 + 1) * c + x0]!;
    const e = v[(z0 + 1) * c + x0 + 1]!;
    return lerp(lerp(a, b, tx), lerp(d, e, tx), tz);
  }
}

/** `radiusM` の縁でちょうど 0 に減衰するコンパクト台の動径核。 */
export function wendland(q: number): number {
  if (q >= 1) return 0;
  const t = 1 - q;
  return t * t * t * t * (4 * q + 1);
}

export function clamp(v: number, a: number, b: number): number {
  return Math.max(a, Math.min(b, v));
}
