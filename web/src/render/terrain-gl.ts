/**
 * 地形の WebGL2 描画。
 *
 * 地形の見た目は ImageGen で事前生成した 4x4 タイルアトラスを使う。
 * 実行時は、シミュレーションの地表グリッドをアトラスの 16 タイルへ
 * 参照して GPU 用の地図テクスチャへ焼くだけで、画像生成は行わない。
 *
 * 崖は現状、地形が常にフラットなクアッド（高さ方向の頂点変位なし）である
 * ため、仕様が意図する「崖面の垂直なクアッド」は実装していない。ここでは
 * 崖セルを暗く縁取ることで位置だけ視覚的に示す簡易表現に留めている。
 * 本物の側面ジオメトリは、地形メッシュが標高で実際に持ち上がるようになる
 * （兵士の描画が M2 で個体の 3D 的配置に対応する頃）まで待つのが自然。
 */

import { createContext, createProgram, clampTextureSize } from "./gl";
import { Camera, TILE_H, TILE_M, PX_PER_M } from "./iso";
import { cliffAt, tileAtIndex, WaterKind, waterKindAt } from "../sim/terrain-data";
import type { TerrainData } from "../sim/terrain-data";
import { TILE_COLORS } from "./terrain-tile";
import { loadTerrainAtlas, type TerrainAtlas } from "./generated-assets";

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
uniform sampler2D u_surfaceTex;
uniform sampler2D u_atlasTex;
uniform vec2 u_mapSize;
uniform vec2 u_atlasSize;
uniform vec2 u_atlasTileSize;
uniform bool u_useAtlas;
void main() {
  if (!u_useAtlas) {
    outColor = vec4(texture(u_tex, v_uv).rgb, 1.0);
    return;
  }

  vec2 cell = floor(v_uv * u_mapSize);
  vec2 cellUv = (cell + vec2(0.5)) / u_mapSize;
  float surface = texture(u_surfaceTex, cellUv).r * 255.0 + 0.5;
  float tileX = mod(surface, 4.0);
  float tileY = floor(surface / 4.0);
  vec2 localUv = fract(v_uv * u_mapSize);
  vec2 atlasPx = vec2(tileX, tileY) * u_atlasTileSize
    + localUv * (u_atlasTileSize - vec2(1.0)) + vec2(0.5);
  vec3 color = texture(u_atlasTex, atlasPx / u_atlasSize).rgb;
  float shade = texture(u_tex, cellUv).a * 1.5;
  outColor = vec4(color * shade, 1.0);
}
`;

export class TerrainGlRenderer {
  private gl: WebGL2RenderingContext;
  private program: WebGLProgram;
  private vao: WebGLVertexArrayObject;
  private texture: WebGLTexture;
  private surfaceTexture: WebGLTexture;
  private atlasTexture: WebGLTexture;
  private uViewSize: WebGLUniformLocation;
  private uCellM: WebGLUniformLocation;
  private uZoom: WebGLUniformLocation;
  private uCamOffset: WebGLUniformLocation;
  private uPxPerM: WebGLUniformLocation;
  private uVScale: WebGLUniformLocation;
  private uSurfaceTex: WebGLUniformLocation;
  private uAtlasTex: WebGLUniformLocation;
  private uMapSize: WebGLUniformLocation;
  private uAtlasSize: WebGLUniformLocation;
  private uAtlasTileSize: WebGLUniformLocation;
  private uUseAtlas: WebGLUniformLocation;

  /** アップロードしたテクスチャの一辺（間引き後）。デバッグ表示用に保持する。 */
  textureDim = 0;
  private cellM = 2;
  private hasTerrain = false;
  private atlas: TerrainAtlas | null = null;

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
    this.uSurfaceTex = loc("u_surfaceTex");
    this.uAtlasTex = loc("u_atlasTex");
    this.uMapSize = loc("u_mapSize");
    this.uAtlasSize = loc("u_atlasSize");
    this.uAtlasTileSize = loc("u_atlasTileSize");
    this.uUseAtlas = loc("u_useAtlas");

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

    const surfaceTexture = gl.createTexture();
    const atlasTexture = gl.createTexture();
    if (!surfaceTexture || !atlasTexture) {
      throw new Error("地形アトラステクスチャの作成に失敗しました");
    }
    this.surfaceTexture = surfaceTexture;
    this.atlasTexture = atlasTexture;
    for (const texture of [this.surfaceTexture, this.atlasTexture]) {
      gl.bindTexture(gl.TEXTURE_2D, texture);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    }
    gl.bindTexture(gl.TEXTURE_2D, this.surfaceTexture);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.bindTexture(gl.TEXTURE_2D, this.atlasTexture);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
  }

  async loadAssets(): Promise<void> {
    this.atlas = await loadTerrainAtlas();
    const gl = this.gl;
    gl.bindTexture(gl.TEXTURE_2D, this.atlasTexture);
    gl.texImage2D(
      gl.TEXTURE_2D,
      0,
      gl.RGBA,
      this.atlas.width,
      this.atlas.height,
      0,
      gl.RGBA,
      gl.UNSIGNED_BYTE,
      this.atlas.pixels,
    );
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
    const stride = Math.max(1, Math.ceil(data.dim / maxDim));
    const outDim = Math.ceil(data.dim / stride);
    this.textureDim = outDim;

    // RGB はアトラス未読込時のフォールバック色、A はヒルシェード係数
    // （0..1.5 を 0..255 に符号化）。セル種別は別の R8 テクスチャへ置く。
    const px = new Uint8Array(outDim * outDim * 4);
    const surfacePx = new Uint8Array(outDim * outDim);
    const cellCm = data.cellM * 100 * stride;

    for (let oy = 0; oy < outDim; oy++) {
      const y = Math.min(data.dim - 1, oy * stride);
      for (let ox = 0; ox < outDim; ox++) {
        const x = Math.min(data.dim - 1, ox * stride);
        const i = y * data.dim + x;
        const tile = tileAtIndex(data, i);
        const c = TILE_COLORS[tile]!;
        surfacePx[oy * outDim + ox] = tile;

        // 水面は平らなので、湖底・川底の標高勾配をそのまま陰影に使うと
        // 水が地形のように凹凸して見える（水中の地形はヒルシェードから除く）。
        let shade: number;
        if (data.water[i]! > 0) {
          shade = 1;
        } else {
          const xn = Math.min(data.dim - 1, x + stride);
          const yn = Math.min(data.dim - 1, y + stride);
          const hx = data.height[y * data.dim + xn]! - data.height[i]!;
          const hy = data.height[yn * data.dim + x]! - data.height[i]!;
          shade = Math.max(0.45, Math.min(1.5, 1 + (-(hx + hy) / cellCm) * 0.7));
        }

        // 崖セルは暗く縁取って位置を示す（本物の側面ジオメトリはまだない。
        // モジュール先頭のコメント参照）。
        if (cliffAt(data, x, y) !== 0) {
          shade *= 0.55;
        }
        // 海は他の水域よりわずかに濃く見せて区別する
        if (waterKindAt(data, x, y) === WaterKind.SEA) {
          shade *= 0.9;
        }

        const o = (oy * outDim + ox) * 4;
        px[o] = Math.min(255, c[0] * shade);
        px[o + 1] = Math.min(255, c[1] * shade);
        px[o + 2] = Math.min(255, c[2] * shade);
        px[o + 3] = Math.min(255, (shade / 1.5) * 255);
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
    gl.bindTexture(gl.TEXTURE_2D, this.surfaceTexture);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    gl.texImage2D(
      gl.TEXTURE_2D,
      0,
      gl.R8,
      outDim,
      outDim,
      0,
      gl.RED,
      gl.UNSIGNED_BYTE,
      surfacePx,
    );
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);

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
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, this.surfaceTexture);
    gl.activeTexture(gl.TEXTURE2);
    gl.bindTexture(gl.TEXTURE_2D, this.atlasTexture);

    const zoom = cam.zoom;
    const camOffsetX = (cam.centerX - cam.centerY) * PX_PER_M * zoom;
    const camOffsetY = (cam.centerX + cam.centerY) * (TILE_H / TILE_M / 2) * zoom;

    gl.uniform2f(this.uViewSize, cam.viewW, cam.viewH);
    gl.uniform1f(this.uCellM, this.cellM);
    gl.uniform1f(this.uZoom, zoom);
    gl.uniform2f(this.uCamOffset, camOffsetX, camOffsetY);
    gl.uniform1f(this.uPxPerM, PX_PER_M);
    gl.uniform1f(this.uVScale, TILE_H / TILE_M / 2);
    gl.uniform1i(this.uSurfaceTex, 1);
    gl.uniform1i(this.uAtlasTex, 2);
    gl.uniform2f(this.uMapSize, this.textureDim, this.textureDim);
    gl.uniform2f(
      this.uAtlasSize,
      this.atlas?.width ?? 1,
      this.atlas?.height ?? 1,
    );
    gl.uniform2f(
      this.uAtlasTileSize,
      this.atlas?.tileWidth ?? 1,
      this.atlas?.tileHeight ?? 1,
    );
    gl.uniform1i(this.uUseAtlas, this.atlas ? 1 : 0);

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
