/**
 * 地形の WebGL2 描画。
 *
 * 仕様 08 章 3 節。地表グリッドをヒルシェード付きのビットマップに焼き
 * （chunk baking の単純化版 — 現状は地図全体を 1 枚のテクスチャに焼く。
 * 128 m チャンク単位での遅延焼き・LRU 破棄は 5 km 超の大規模マップでの
 * メモリ・帯域対策として将来实装する)、1 枚のクアッドとして GPU に渡す。
 *
 * **タイルアトラス（仕様 08 章 3.2 節）についての注記**: 仕様が想定する
 * 「スプライトのタイルを Wang tile（47-blob）で遷移させながら並べる」方式は
 * 採っていない。代わりに、地表タイプの色をセルごとに直接計算してビットマップに
 * 焼く方式にした。スプライトタイルを持たないので、そもそもタイル同士の
 * 境界線という概念がなく、Wang tile が解決しようとしている「隣接タイルの
 * 継ぎ目」問題が発生しない。見た目のなめらかさは十分に出せている
 * （このモジュールの利用箇所のスクリーンショットで確認済み）。
 * 実在するスプライト画像（草地の質感、岩肌のテクスチャなど）を貼りたくなったら、
 * そのときに本来のタイルアトラス方式へ切り替える。
 *
 * 崖は現状、地形が常にフラットなクアッド（高さ方向の頂点変位なし）である
 * ため、仕様が意図する「崖面の垂直なクアッド」は実装していない。ここでは
 * 崖セルを暗く縁取ることで位置だけ視覚的に示す簡易表現に留めている。
 * 本物の側面ジオメトリは、地形メッシュが標高で実際に持ち上がるようになる
 * （兵士の描画が M2 で個体の 3D 的配置に対応する頃）まで待つのが自然。
 */

import { createContext, createProgram, clampTextureSize } from "./gl";
import { Camera, TILE_H, TILE_M, PX_PER_M } from "./iso";
import { cliffAt, WaterKind, waterKindAt } from "../sim/terrain-data";
import type { TerrainData } from "../sim/terrain-data";
import { SURFACE_COLORS } from "./surface-colors";

const VERT_SRC = `#version 300 es
in vec2 a_grid;
in vec2 a_uv;
out vec2 v_uv;

uniform vec2 u_viewSize;
uniform float u_cellM;
uniform float u_zoom;
uniform vec2 u_camOffset;
uniform float u_pxPerM;
uniform float u_vScale;

void main() {
  vec2 world = a_grid * u_cellM;
  float sx = (world.x - world.y) * u_pxPerM * u_zoom - u_camOffset.x + u_viewSize.x * 0.5;
  float sy = (world.x + world.y) * u_vScale * u_zoom - u_camOffset.y + u_viewSize.y * 0.5;
  vec2 ndc = vec2((sx / u_viewSize.x) * 2.0 - 1.0, 1.0 - (sy / u_viewSize.y) * 2.0);
  gl_Position = vec4(ndc, 0.0, 1.0);
  v_uv = a_uv;
}
`;

const FRAG_SRC = `#version 300 es
precision mediump float;
in vec2 v_uv;
out vec4 outColor;
uniform sampler2D u_tex;
void main() {
  outColor = texture(u_tex, v_uv);
}
`;

export class TerrainGlRenderer {
  private gl: WebGL2RenderingContext;
  private program: WebGLProgram;
  private vao: WebGLVertexArrayObject;
  private texture: WebGLTexture;
  private uViewSize: WebGLUniformLocation;
  private uCellM: WebGLUniformLocation;
  private uZoom: WebGLUniformLocation;
  private uCamOffset: WebGLUniformLocation;
  private uPxPerM: WebGLUniformLocation;
  private uVScale: WebGLUniformLocation;

  /** アップロードしたテクスチャの一辺（間引き後）。デバッグ表示用に保持する。 */
  textureDim = 0;
  private cellM = 2;
  private hasTerrain = false;

  constructor(canvas: HTMLCanvasElement) {
    const gl = createContext(canvas);
    this.gl = gl;
    this.program = createProgram(gl, VERT_SRC, FRAG_SRC);

    const loc = (name: string): WebGLUniformLocation => {
      const l = gl.getUniformLocation(this.program, name);
      if (!l) throw new Error(`uniform ${name} が見つかりません`);
      return l;
    };
    this.uViewSize = loc("u_viewSize");
    this.uCellM = loc("u_cellM");
    this.uZoom = loc("u_zoom");
    this.uCamOffset = loc("u_camOffset");
    this.uPxPerM = loc("u_pxPerM");
    this.uVScale = loc("u_vScale");

    // クアッドは地形の描画のたびに座標が変わらない（世界座標そのものを
    // 頂点属性にして、変換はすべて頂点シェーダの uniform で行う）ので、
    // VAO は一度作れば terrain データが変わっても作り直さなくてよい。
    this.vao = gl.createVertexArray()!;
    const tex = gl.createTexture();
    if (!tex) throw new Error("テクスチャの作成に失敗しました");
    this.texture = tex;
    gl.bindTexture(gl.TEXTURE_2D, this.texture);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  }

  /**
   * 地形を焼いて GPU へアップロードする。地形が変わったとき（起動時、
   * 将来の工兵による地形改変時）だけ呼べばよい。
   */
  setTerrain(data: TerrainData): void {
    const gl = this.gl;
    this.cellM = data.cellM;

    // テクスチャ上限を超える場合は間引く（8 km マップを 2 m セルで焼くなど）。
    // 見た目のわずかな解像度低下と引き換えに、アップロード自体は必ず成功させる。
    const maxDim = clampTextureSize(gl, data.dim);
    const stride = Math.ceil(data.dim / maxDim);
    const outDim = Math.ceil(data.dim / stride);
    this.textureDim = outDim;

    const px = new Uint8Array(outDim * outDim * 4);
    const cellCm = data.cellM * 100 * stride;

    for (let oy = 0; oy < outDim; oy++) {
      const y = Math.min(data.dim - 1, oy * stride);
      for (let ox = 0; ox < outDim; ox++) {
        const x = Math.min(data.dim - 1, ox * stride);
        const i = y * data.dim + x;
        const c = SURFACE_COLORS[data.surface[i]! & 0x0f]!;

        const xn = Math.min(data.dim - 1, x + stride);
        const yn = Math.min(data.dim - 1, y + stride);
        const hx = data.height[y * data.dim + xn]! - data.height[i]!;
        const hy = data.height[yn * data.dim + x]! - data.height[i]!;
        const shade = Math.max(0.45, Math.min(1.5, 1 + (-(hx + hy) / cellCm) * 0.7));

        let r = c[0] * shade;
        let g = c[1] * shade;
        let b = c[2] * shade;

        // 崖セルは暗く縁取って位置を示す（本物の側面ジオメトリはまだない。
        // モジュール先頭のコメント参照）。
        if (cliffAt(data, x, y) !== 0) {
          r *= 0.55;
          g *= 0.55;
          b *= 0.55;
        }
        // 海は他の水域よりわずかに濃く見せて区別する
        if (waterKindAt(data, x, y) === WaterKind.Sea) {
          r *= 0.85;
          g *= 0.9;
          b *= 1.05;
        }

        const o = (oy * outDim + ox) * 4;
        px[o] = Math.min(255, r);
        px[o + 1] = Math.min(255, g);
        px[o + 2] = Math.min(255, b);
        px[o + 3] = 255;
      }
    }

    gl.bindTexture(gl.TEXTURE_2D, this.texture);
    gl.texImage2D(
      gl.TEXTURE_2D,
      0,
      gl.RGBA,
      outDim,
      outDim,
      0,
      gl.RGBA,
      gl.UNSIGNED_BYTE,
      px,
    );

    // クアッドの頂点をグリッド座標（0..dim）で持つ。頂点シェーダが
    // ワールド座標・スクリーン座標への変換を行う。
    const dim = data.dim;
    // prettier-ignore
    const verts = new Float32Array([
      0,   0,   0, 0,
      dim, 0,   1, 0,
      0,   dim, 0, 1,
      dim, dim, 1, 1,
    ]);
    const buf = gl.createBuffer();
    gl.bindVertexArray(this.vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(gl.ARRAY_BUFFER, verts, gl.STATIC_DRAW);

    const aGrid = gl.getAttribLocation(this.program, "a_grid");
    const aUv = gl.getAttribLocation(this.program, "a_uv");
    gl.enableVertexAttribArray(aGrid);
    gl.vertexAttribPointer(aGrid, 2, gl.FLOAT, false, 16, 0);
    gl.enableVertexAttribArray(aUv);
    gl.vertexAttribPointer(aUv, 2, gl.FLOAT, false, 16, 8);
    gl.bindVertexArray(null);

    this.hasTerrain = true;
  }

  draw(cam: Camera): void {
    const gl = this.gl;
    gl.viewport(0, 0, cam.viewW, cam.viewH);
    gl.clearColor(0.05, 0.06, 0.08, 1);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);

    if (!this.hasTerrain) return;

    gl.useProgram(this.program);
    gl.bindVertexArray(this.vao);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.texture);

    const zoom = cam.zoom;
    const camOffsetX = (cam.centerX - cam.centerY) * PX_PER_M * zoom;
    const camOffsetY = (cam.centerX + cam.centerY) * (TILE_H / TILE_M / 2) * zoom;

    gl.uniform2f(this.uViewSize, cam.viewW, cam.viewH);
    gl.uniform1f(this.uCellM, this.cellM);
    gl.uniform1f(this.uZoom, zoom);
    gl.uniform2f(this.uCamOffset, camOffsetX, camOffsetY);
    gl.uniform1f(this.uPxPerM, PX_PER_M);
    gl.uniform1f(this.uVScale, TILE_H / TILE_M / 2);

    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    gl.bindVertexArray(null);
  }

  /** ワールド座標の標高（m）。兵士を地面に置くために使う。 */
  heightAt(data: TerrainData, xM: number, yM: number): number {
    const cx = Math.max(0, Math.min(data.dim - 1, Math.floor(xM / data.cellM)));
    const cy = Math.max(0, Math.min(data.dim - 1, Math.floor(yM / data.cellM)));
    return data.height[cy * data.dim + cx]! / 100;
  }
}
