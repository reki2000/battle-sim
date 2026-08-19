/**
 * Rust/Wasm が生成した人物ポリゴンを WebGL2 へ転送する汎用レンダラ。
 *
 * 人物のボーン、モーション、体格、騎乗、LOD、カリングは `sim-render` が
 * 担当する。このファイルは `[clipX, clipY, clipZ, clipW, r, g, b, a]` の
 * 三角形列を受け取り、そのまま描くだけである。人物スプライトやテクスチャは
 * どのLODでも使用しない。
 */

import init, { ArmyRenderer as WasmArmyRenderer } from "../wasm/sim.js";
import { createContext, createProgram } from "./gl";
import type { Camera } from "./iso";
import type { InterpolatedPositions, SnapshotView } from "../sim/snapshot";

const VERTEX_FLOATS = 8;
const VERTEX_STRIDE = VERTEX_FLOATS * Float32Array.BYTES_PER_ELEMENT;

/** UI・ミニマップと共有する陣営色。人物ポリゴンにも同系色を使う。 */
export const FACTION_COLORS = ["#3d7ab8", "#d98032"];

const VERT_SRC = `#version 300 es
in vec4 a_clip;
in vec4 a_color;
out vec4 v_color;

void main() {
  gl_Position = a_clip;
  v_color = a_color;
}
`;

const FRAG_SRC = `#version 300 es
precision mediump float;
in vec4 v_color;
out vec4 outColor;

void main() {
  outColor = v_color;
}
`;

interface PendingSnapshot {
  bytes: Uint8Array;
  tick: number;
}

export class SoldierRenderer {
  /** 直前のフレームで実際にポリゴン化した兵士数。 */
  drawn = 0;
  /** カリング前に画面内と判定された兵士数。 */
  visible = 0;
  /** Rust側で画面外として除外した兵士数。 */
  culled = 0;

  private readonly canvas: HTMLCanvasElement;
  private gl: WebGL2RenderingContext | null = null;
  private program: WebGLProgram | null = null;
  private vao: WebGLVertexArrayObject | null = null;
  private vertexBuffer: WebGLBuffer | null = null;
  private memory: WebAssembly.Memory | null = null;
  private army: WasmArmyRenderer | null = null;
  private snapshotStride = 20;
  private pendingSnapshot: PendingSnapshot | null = null;
  private cameraKey = "";

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
    try {
      const gl = createContext(canvas, { alpha: true });
      const program = createProgram(gl, VERT_SRC, FRAG_SRC);
      const vao = gl.createVertexArray();
      const vertexBuffer = gl.createBuffer();
      if (!vao || !vertexBuffer) throw new Error("人物ポリゴン用WebGLリソースを作成できません");

      const clip = gl.getAttribLocation(program, "a_clip");
      const color = gl.getAttribLocation(program, "a_color");
      gl.bindVertexArray(vao);
      gl.bindBuffer(gl.ARRAY_BUFFER, vertexBuffer);
      gl.bufferData(gl.ARRAY_BUFFER, 0, gl.DYNAMIC_DRAW);
      gl.enableVertexAttribArray(clip);
      gl.vertexAttribPointer(clip, 4, gl.FLOAT, false, VERTEX_STRIDE, 0);
      gl.enableVertexAttribArray(color);
      gl.vertexAttribPointer(
        color,
        4,
        gl.FLOAT,
        false,
        VERTEX_STRIDE,
        4 * Float32Array.BYTES_PER_ELEMENT,
      );
      gl.bindVertexArray(null);

      gl.enable(gl.BLEND);
      gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
      gl.enable(gl.DEPTH_TEST);
      gl.depthFunc(gl.LESS);
      gl.disable(gl.CULL_FACE);
      gl.clearColor(0, 0, 0, 0);

      this.gl = gl;
      this.program = program;
      this.vao = vao;
      this.vertexBuffer = vertexBuffer;
    } catch (error) {
      // WebGL2がない環境では、同じスナップショットからCanvas2Dの三角形を描く。
      // ここでも画像・スプライトへのフォールバックは行わない。
      console.warn("人物ポリゴンのWebGL2初期化に失敗しました", error);
    }
  }

  setSnapshotStride(stride: number): void {
    if (stride !== 16 && stride !== 20) {
      throw new Error(`人物描画Wasmが未対応のsnapshot strideです: ${stride}`);
    }
    this.snapshotStride = stride;
  }

  /** Wasmを初期化し、シナリオ切替時は人物の内部状態を新しくする。 */
  async initialize(): Promise<void> {
    const wasm = await init();
    this.memory = wasm.memory;
    this.army?.free();
    this.army = new WasmArmyRenderer();
    this.cameraKey = "";
    if (this.pendingSnapshot) {
      const pending = this.pendingSnapshot;
      this.pendingSnapshot = null;
      this.army.applySnapshot(pending.bytes, this.snapshotStride, pending.tick);
    }
  }

  /** 20 HzスナップショットをRustへ一括適用する。エンティティごとの呼び出しはしない。 */
  applySnapshot(buffer: ArrayBuffer, tick: number): void {
    const bytes = new Uint8Array(buffer);
    if (!this.army) {
      // 初期WasmロードとWorkerの最初のsnapshotが競合した場合だけコピーして保留する。
      this.pendingSnapshot = { bytes: bytes.slice(), tick };
      return;
    }
    this.army.applySnapshot(bytes, this.snapshotStride, tick);
  }

  draw(
    fallbackCtx: CanvasRenderingContext2D,
    cam: Camera,
    snap: SnapshotView,
    interp: InterpolatedPositions,
    alpha: number,
    simulationDt: number,
  ): void {
    if (!this.gl || !this.program || !this.vao || !this.vertexBuffer || !this.army || !this.memory) {
      this.drawFallback(fallbackCtx, cam, snap, interp, alpha);
      return;
    }

    const cameraKey = [
      cam.centerX,
      cam.centerY,
      cam.pxPerM,
      cam.viewW,
      cam.viewH,
      cam.worldSizeM,
    ].join("/");
    if (cameraKey !== this.cameraKey) {
      this.army.setCamera(
        cam.centerX,
        cam.centerY,
        cam.pxPerM,
        cam.viewW,
        cam.viewH,
        cam.worldSizeM,
      );
      this.cameraKey = cameraKey;
    }

    this.army.step(simulationDt, alpha, cam.lod);
    const floatLen = this.army.verticesFloatLen();
    const vertexCount = this.army.vertexCount();
    const ptr = this.army.verticesPtr();
    const vertices = floatLen === 0
      ? new Float32Array(0)
      : new Float32Array(this.memory.buffer, ptr, floatLen);

    const gl = this.gl;
    gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    gl.useProgram(this.program);
    gl.bindVertexArray(this.vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.vertexBuffer);
    gl.bufferData(gl.ARRAY_BUFFER, vertices, gl.DYNAMIC_DRAW);
    gl.drawArrays(gl.TRIANGLES, 0, vertexCount);
    gl.bindVertexArray(null);

    this.drawn = this.army.drawnCount();
    this.visible = this.army.visibleCount();
    this.culled = this.army.culledCount();
  }

  /** WebGL2が使えない場合の画像非依存フォールバック。 */
  private drawFallback(
    ctx: CanvasRenderingContext2D,
    cam: Camera,
    snap: SnapshotView,
    interp: InterpolatedPositions,
    alpha: number,
  ): void {
    const cap = cam.lod >= 3 ? 5_000 : 15_000;
    const stride = Math.max(1, Math.ceil(snap.count / cap));
    let drawn = 0;
    ctx.save();
    for (let i = 0; i < snap.count; i += stride) {
      const x = interp.x(i, alpha);
      const y = interp.y(i, alpha);
      const p = cam.worldToScreen(x, y, snap.z(i));
      if (p.sx < -12 || p.sy < -24 || p.sx > cam.viewW + 12 || p.sy > cam.viewH + 24) continue;
      const dead = snap.state(i) >= 12;
      const faction = snap.faction(i) & 1;
      const base = faction === 0 ? [61, 122, 184] : [217, 128, 50];
      const shade = dead ? 0.45 : 1;
      ctx.fillStyle = `rgba(${base[0]! * shade},${base[1]! * shade},${base[2]! * shade},0.92)`;
      const size = Math.max(1.5, Math.min(12, cam.pxPerM * (dead ? 0.32 : 0.5)));
      ctx.beginPath();
      if (dead || snap.isStumbling(i)) {
        ctx.moveTo(p.sx - size, p.sy);
        ctx.lineTo(p.sx + size, p.sy + size * 0.25);
        ctx.lineTo(p.sx - size * 0.65, p.sy + size * 0.55);
      } else {
        ctx.moveTo(p.sx, p.sy - size * 2);
        ctx.lineTo(p.sx + size, p.sy);
        ctx.lineTo(p.sx - size, p.sy);
      }
      ctx.closePath();
      ctx.fill();
      drawn++;
    }
    ctx.restore();
    this.drawn = drawn;
    this.visible = drawn;
    this.culled = snap.count - drawn;
  }
}
