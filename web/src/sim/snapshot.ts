/**
 * wasm のリニアメモリに置かれた描画スナップショットを読む。
 *
 * レイアウトは crates/sim-core/src/snapshot.rs と一致させること。
 * ずれるとサイレントに壊れるので、SOLDIER_STRIDE を wasm 側から取って検証する。
 */

/** 兵士 1 体あたりのバイト数。 */
export const SOLDIER_STRIDE = 20;

/** State enum。crates/sim-core/src/soldiers.rs と一致させること。 */
/** `soldiers::flags` のビット（Rust 側と一致させること）。 */
export enum SoldierFlag {
  Mounted = 1 << 0,
  Commander = 1 << 1,
  BannerBearer = 1 << 2,
  Engineer = 1 << 3,
  MissileTroop = 1 << 4,
  Sleeping = 1 << 5,
  /** 転倒中。倒れていて動けず攻撃もできない */
  Stumbling = 1 << 6,
  /** 盾を構えて突撃を受け止める姿勢 */
  Braced = 1 << 7,
}

export enum SoldierState {
  Idle = 0,
  Marching = 1,
  Repositioning = 2,
  Advancing = 3,
  Charging = 4,
  Engaged = 5,
  Shooting = 6,
  Reloading = 7,
  Wavering = 8,
  Broken = 9,
  Rallying = 10,
  Working = 11,
  Downed = 12,
  Dead = 13,
}

/**
 * スナップショットのビュー。
 *
 * wasm のメモリが grow するとバッファが detach するので、
 * 毎フレーム {@link SnapshotView.rebind} で貼り直す。
 */
export class SnapshotView {
  private view = new DataView(new ArrayBuffer(0));
  private stride = SOLDIER_STRIDE;
  count = 0;

  /** v1(16 B) を読みながら v2(20 B) Wasm を再生成できる移行用。 */
  setStride(stride: number): void {
    if (stride !== 16 && stride !== SOLDIER_STRIDE) {
      throw new Error(`未対応の兵士スナップショットstrideです: ${stride}`);
    }
    this.stride = stride;
  }

  /**
   * バッファに貼り直す。
   *
   * ワーカーから transfer されてきた ArrayBuffer にも、
   * SharedArrayBuffer 経由の wasm メモリにも使える。
   */
  bind(buffer: ArrayBufferLike, byteOffset = 0, byteLen?: number): void {
    const len = byteLen ?? buffer.byteLength - byteOffset;
    this.view = new DataView(buffer as ArrayBuffer, byteOffset, len);
    this.count = (len / this.stride) | 0;
  }

  /**
   * wasm のリニアメモリを直接見る（同一スレッドで動かす場合）。
   *
   * メモリが grow するとビューが detach するので毎回貼り直すこと。
   */
  rebind(memory: WebAssembly.Memory, ptr: number, byteLen: number): void {
    this.bind(memory.buffer, ptr, byteLen);
  }

  /** X 座標（m）。 */
  x(i: number): number {
    return this.view.getFloat32(i * this.stride, true);
  }

  /** Y 座標（m）。 */
  y(i: number): number {
    return this.view.getFloat32(i * this.stride + 4, true);
  }

  /** 標高（m）。 */
  z(i: number): number {
    return this.view.getInt16(i * this.stride + 8, true) / 100;
  }

  /** 向き（brad、0..65535）。 */
  facing(i: number): number {
    return this.view.getUint16(i * this.stride + 10, true);
  }

  /** 向き（ラジアン）。 */
  facingRad(i: number): number {
    return (this.facing(i) / 65536) * Math.PI * 2;
  }

  unitId(i: number): number {
    return this.view.getUint16(i * this.stride + 12, true);
  }

  troopType(i: number): number {
    return this.stride === 16 ? this.unitId(i) : this.view.getUint16(i * this.stride + 14, true);
  }

  state(i: number): SoldierState {
    return this.view.getUint8(i * this.stride + (this.stride === 16 ? 14 : 16)) as SoldierState;
  }

  flags(i: number): number {
    return this.view.getUint8(i * this.stride + (this.stride === 16 ? 15 : 17));
  }

  /** 転倒中か（倒れて動けない）。 */
  isStumbling(i: number): boolean {
    return (this.flags(i) & SoldierFlag.Stumbling) !== 0;
  }

  /** 盾を構えて踏ん張っているか。 */
  isBraced(i: number): boolean {
    return (this.flags(i) & SoldierFlag.Braced) !== 0;
  }

  faction(i: number): number {
    return this.stride === 16
      ? this.unitId(i) & 1
      : this.view.getUint8(i * this.stride + 18);
  }

  /** 生存しているか（Downed / Dead でない）。 */
  isAlive(i: number): boolean {
    const s = this.state(i);
    return s !== SoldierState.Downed && s !== SoldierState.Dead;
  }
}

/**
 * 2 つのスナップショット間を補間するための位置バッファ。
 *
 * シミュレーションは 20 Hz、描画は 60 fps なので、
 * 3 フレームに 1 回しか新しい状態が来ない（仕様 08 章 9 節）。
 * 補間はレンダラ内で完結し、シミュレーションには戻さない。
 */
export class InterpolatedPositions {
  private prevX = new Float32Array(0);
  private prevY = new Float32Array(0);
  private currX = new Float32Array(0);
  private currY = new Float32Array(0);
  count = 0;

  /** 新しいスナップショットを取り込む。前回ぶんは prev に落ちる。 */
  push(snap: SnapshotView): void {
    const n = snap.count;
    if (this.currX.length < n) {
      this.prevX = new Float32Array(n);
      this.prevY = new Float32Array(n);
      this.currX = new Float32Array(n);
      this.currY = new Float32Array(n);
    } else {
      this.prevX.set(this.currX.subarray(0, this.count));
      this.prevY.set(this.currY.subarray(0, this.count));
    }
    for (let i = 0; i < n; i++) {
      this.currX[i] = snap.x(i);
      this.currY[i] = snap.y(i);
    }
    // 新規に現れた兵士は補間せず即座にその位置へ
    for (let i = this.count; i < n; i++) {
      this.prevX[i] = this.currX[i]!;
      this.prevY[i] = this.currY[i]!;
    }
    this.count = n;
  }

  /** alpha は 0..1。 */
  x(i: number, alpha: number): number {
    const a = this.prevX[i]!;
    const b = this.currX[i]!;
    return a + (b - a) * alpha;
  }

  y(i: number, alpha: number): number {
    const a = this.prevY[i]!;
    const b = this.currY[i]!;
    return a + (b - a) * alpha;
  }
}
