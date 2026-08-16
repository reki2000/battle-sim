/**
 * 兵士の描画。
 *
 * M2 の描画経路は、兵士専用の WebGL2 キャンバスへインスタンス描画する。
 * クアッドは 1 枚だけを共有し、位置・標高・向き・スプライト番号・陣営・状態を
 * 16 バイトのインスタンス属性へ詰めて 1 ドローコールで送る。
 *
 * 人物画像は ImageGen で事前生成した PNG をテクスチャとして読み込む。現在の
 * アセットは 1 枚絵だが、packed の facing / sprite フィールドとシェーダの
 * アトラス分岐は将来の「役職 × 行動 × 8方向 × フレーム」スプライトシートへ
 * 差し替えられるようにしてある。WebGL2 が使えない環境では従来の Canvas2D
 * 簡易描画へフォールバックする。
 */

import { createContext, createProgram } from "./gl";
import { Camera, Lod, PX_PER_M, TILE_H, TILE_M, Z_SCALE } from "./iso";
import type { InterpolatedPositions, SnapshotView } from "../sim/snapshot";
import { SoldierState } from "../sim/snapshot";
import { loadSoldierSprite } from "./generated-assets";

/** 陣営色。色覚多様性に配慮し、赤/青ではなく青/橙にする（仕様 09 章 9 節）。 */
const FACTION_COLORS = ["#3d7ab8", "#d98032"];

const INSTANCE_STRIDE = 16;

const VERT_SRC = `#version 300 es
in vec2 a_quad;
in vec2 i_pos;
in float i_z;
in uint i_packed;

uniform vec2 u_viewSize;
uniform float u_zoom;
uniform vec2 u_camOffset;
uniform float u_pxPerM;
uniform float u_vScale;
uniform float u_zScale;
uniform float u_worldSize;
uniform float u_spriteWidth;
uniform float u_spriteHeight;

out vec2 v_uv;
flat out uint v_packed;

void main() {
  float facing = float(i_packed & 255u);
  float sprite = float((i_packed >> 8u) & 4095u);
  // 現在の 1 枚絵アセットでは未使用だが、値を読むことでアトラスの
  // facing/sprite インデックスを後から同じバッファのまま利用できる。
  float atlasOffset = facing * 0.0 + sprite * 0.0;

  float sx = (i_pos.x - i_pos.y) * u_pxPerM * u_zoom - u_camOffset.x
    + u_viewSize.x * 0.5;
  float sy = (i_pos.x + i_pos.y) * u_vScale * u_zoom
    - i_z * u_zScale * u_zoom - u_camOffset.y + u_viewSize.y * 0.5;
  float px = sx + (a_quad.x - 0.5) * u_spriteWidth;
  float py = sy - a_quad.y * u_spriteHeight;
  vec2 ndc = vec2((px / u_viewSize.x) * 2.0 - 1.0,
                  1.0 - (py / u_viewSize.y) * 2.0);

  // x+y の昇順を GPU 深度へ写す。兵士キャンバス内の CPU ソートを不要にし、
  // 地形と同じワールド→スクリーン式で座標を揃える。
  float depth = clamp((i_pos.x + i_pos.y) / (2.0 * u_worldSize), 0.0, 1.0);
  gl_Position = vec4(ndc, depth * 2.0 - 1.0 + atlasOffset, 1.0);
  v_uv = vec2(a_quad.x, 1.0 - a_quad.y);
  v_packed = i_packed;
}
`;

const FRAG_SRC = `#version 300 es
precision mediump float;
precision highp int;

uniform sampler2D u_spriteTex;
uniform int u_renderMode;
uniform vec4 u_faction0;
uniform vec4 u_faction1;

in vec2 v_uv;
flat in uint v_packed;
out vec4 outColor;

void main() {
  uint faction = (v_packed >> 20u) & 15u;
  uint state = (v_packed >> 24u) & 255u;
  vec4 factionColor = faction == 1u ? u_faction1 : u_faction0;

  // Downed / Dead はスプライトを残しつつ、横倒しの暗い個体として表示する。
  // シミュレーション上の個体を描画から間引かないという M2 の原則を守る。
  if (state >= 12u) {
    float edge = smoothstep(0.0, 0.18, v_uv.y) * smoothstep(1.0, 0.82, v_uv.y);
    outColor = vec4(factionColor.rgb * 0.48, 0.68 * edge);
    return;
  }

  if (u_renderMode == 0) {
    vec4 sprite = texture(u_spriteTex, v_uv);
    if (sprite.a <= 0.01) discard;
    // 2陣営目だけをシェーダ内でパレット寄せする。将来の本格的な
    // パレット置換でも、このインスタンス属性と描画経路はそのまま使える。
    if (faction == 1u) sprite.rgb = mix(sprite.rgb, factionColor.rgb, 0.28);
    outColor = sprite;
    return;
  }

  // L2/L3/L4 は小さなクアッド／点に落とす。状態は色の明度に反映し、
  // 画面を遠ざけても状態と陣営の違いを失わないようにする。
  vec3 color = factionColor.rgb;
  if (state == 8u || state == 9u) color = mix(color, vec3(0.92, 0.48, 0.20), 0.35);
  if (state == 10u) color = mix(color, vec3(0.35, 0.86, 0.52), 0.28);
  outColor = vec4(color, 0.96);
}
`;

export class SoldierRenderer {
  /** 直前のフレームで実際に描いた数（デバッグ表示用）。 */
  drawn = 0;

  private readonly canvas: HTMLCanvasElement;
  private readonly gl: WebGL2RenderingContext | null;
  private readonly program: WebGLProgram | null;
  private readonly vao: WebGLVertexArrayObject | null;
  private readonly instanceBuffer: WebGLBuffer | null;
  private readonly spriteTexture: WebGLTexture | null;
  private readonly uniform: {
    viewSize: WebGLUniformLocation;
    zoom: WebGLUniformLocation;
    camOffset: WebGLUniformLocation;
    pxPerM: WebGLUniformLocation;
    vScale: WebGLUniformLocation;
    zScale: WebGLUniformLocation;
    worldSize: WebGLUniformLocation;
    spriteWidth: WebGLUniformLocation;
    spriteHeight: WebGLUniformLocation;
    renderMode: WebGLUniformLocation;
    spriteTex: WebGLUniformLocation;
    faction0: WebGLUniformLocation;
    faction1: WebGLUniformLocation;
  } | null;
  private instanceBytes = new ArrayBuffer(0);
  private assetsReady = false;

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
    let gl: WebGL2RenderingContext | null = null;
    try {
      gl = createContext(canvas, { alpha: true });
    } catch {
      // Playwright の一部環境や古いブラウザでは Canvas2D フォールバックを使う。
    }
    this.gl = gl;

    if (!gl) {
      this.program = null;
      this.vao = null;
      this.instanceBuffer = null;
      this.spriteTexture = null;
      this.uniform = null;
      return;
    }

    this.program = createProgram(gl, VERT_SRC, FRAG_SRC);
    this.vao = gl.createVertexArray();
    this.instanceBuffer = gl.createBuffer();
    this.spriteTexture = gl.createTexture();
    if (!this.vao || !this.instanceBuffer || !this.spriteTexture) {
      throw new Error("兵士インスタンス描画用の GPU リソースを作成できません");
    }

    const location = (name: string): WebGLUniformLocation => {
      const result = gl.getUniformLocation(this.program!, name);
      if (!result) throw new Error(`兵士描画 uniform ${name} が見つかりません`);
      return result;
    };
    this.uniform = {
      viewSize: location("u_viewSize"),
      zoom: location("u_zoom"),
      camOffset: location("u_camOffset"),
      pxPerM: location("u_pxPerM"),
      vScale: location("u_vScale"),
      zScale: location("u_zScale"),
      worldSize: location("u_worldSize"),
      spriteWidth: location("u_spriteWidth"),
      spriteHeight: location("u_spriteHeight"),
      renderMode: location("u_renderMode"),
      spriteTex: location("u_spriteTex"),
      faction0: location("u_faction0"),
      faction1: location("u_faction1"),
    };

    const quad = new Float32Array([0, 0, 1, 0, 0, 1, 1, 1]);
    const aQuad = gl.getAttribLocation(this.program, "a_quad");
    const iPos = gl.getAttribLocation(this.program, "i_pos");
    const iZ = gl.getAttribLocation(this.program, "i_z");
    const iPacked = gl.getAttribLocation(this.program, "i_packed");

    gl.bindVertexArray(this.vao);
    const quadBuffer = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, quadBuffer);
    gl.bufferData(gl.ARRAY_BUFFER, quad, gl.STATIC_DRAW);
    gl.enableVertexAttribArray(aQuad);
    gl.vertexAttribPointer(aQuad, 2, gl.FLOAT, false, 8, 0);

    gl.bindBuffer(gl.ARRAY_BUFFER, this.instanceBuffer);
    gl.bufferData(gl.ARRAY_BUFFER, 16, gl.DYNAMIC_DRAW);
    gl.enableVertexAttribArray(iPos);
    gl.vertexAttribPointer(iPos, 2, gl.FLOAT, false, INSTANCE_STRIDE, 0);
    gl.vertexAttribDivisor(iPos, 1);
    gl.enableVertexAttribArray(iZ);
    gl.vertexAttribPointer(iZ, 1, gl.FLOAT, false, INSTANCE_STRIDE, 8);
    gl.vertexAttribDivisor(iZ, 1);
    gl.enableVertexAttribArray(iPacked);
    gl.vertexAttribIPointer(iPacked, 1, gl.UNSIGNED_INT, INSTANCE_STRIDE, 12);
    gl.vertexAttribDivisor(iPacked, 1);
    gl.bindVertexArray(null);

    gl.bindTexture(gl.TEXTURE_2D, this.spriteTexture);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.texImage2D(
      gl.TEXTURE_2D,
      0,
      gl.RGBA,
      1,
      1,
      0,
      gl.RGBA,
      gl.UNSIGNED_BYTE,
      new Uint8Array([61, 122, 184, 255]),
    );
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
    gl.enable(gl.DEPTH_TEST);
    gl.depthFunc(gl.LESS);
  }

  async loadAssets(): Promise<void> {
    if (!this.gl || !this.spriteTexture) return;
    const sprite = await loadSoldierSprite();
    const gl = this.gl;
    gl.bindTexture(gl.TEXTURE_2D, this.spriteTexture);
    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, sprite);
    this.assetsReady = true;
  }

  draw(
    fallbackCtx: CanvasRenderingContext2D,
    cam: Camera,
    snap: SnapshotView,
    interp: InterpolatedPositions,
    alpha: number,
    groundHeight: (x: number, y: number) => number,
  ): void {
    if (!this.gl || !this.program || !this.vao || !this.instanceBuffer || !this.uniform) {
      this.drawFallback(fallbackCtx, cam, snap, interp, alpha, groundHeight);
      return;
    }
    this.drawInstanced(cam, snap, interp, alpha, groundHeight);
  }

  private drawInstanced(
    cam: Camera,
    snap: SnapshotView,
    interp: InterpolatedPositions,
    alpha: number,
    groundHeight: (x: number, y: number) => number,
  ): void {
    const gl = this.gl!;
    const program = this.program!;
    const vao = this.vao!;
    const buffer = this.instanceBuffer!;
    const u = this.uniform!;
    const n = snap.count;
    const zoom = cam.zoom;
    const camOffsetX = (cam.centerX - cam.centerY) * PX_PER_M * zoom;
    const camOffsetY = (cam.centerX + cam.centerY) * (TILE_H / TILE_M / 2) * zoom;
    const pxPerM = cam.pxPerM;
    const close = cam.lod === Lod.Close || cam.lod === Lod.Tactical;
    const spriteHeight = close ? 1.7 * pxPerM : cam.lod === Lod.Unit ? 0.7 * pxPerM : 1;
    const spriteWidth = close ? spriteHeight * (200 / 300) : spriteHeight;

    if (n === 0) {
      this.drawn = 0;
      gl.viewport(0, 0, this.canvas.width, this.canvas.height);
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
      return;
    }

    if (this.instanceBytes.byteLength < n * INSTANCE_STRIDE) {
      this.instanceBytes = new ArrayBuffer(n * INSTANCE_STRIDE);
    }
    const data = new DataView(this.instanceBytes);
    const margin = Math.max(spriteWidth, spriteHeight) * 1.5;
    // 少数個体では毎個体の射影判定の方が高くつく。GPU は画面外のクアッドを
    // そのままクリップできるため、5,000 体規模でだけ CPU カリングを有効にする。
    const shouldCull = n > 3000;
    let visibleCount = 0;
    for (let i = 0; i < n; i++) {
      const x = interp.x(i, alpha);
      const y = interp.y(i, alpha);
      const z = groundHeight(x, y);
      if (shouldCull) {
        const screen = cam.worldToScreen(x, y, z);
        if (
          screen.sx < -margin ||
          screen.sy < -margin ||
          screen.sx > cam.viewW + margin ||
          screen.sy > cam.viewH + margin
        ) {
          continue;
        }
      }

      const offset = visibleCount * INSTANCE_STRIDE;
      data.setFloat32(offset, x, true);
      data.setFloat32(offset + 4, y, true);
      data.setFloat32(offset + 8, z, true);
      const facing = Math.round((snap.facing(i) / 65536) * 255) & 0xff;
      const spriteIndex = 0;
      const faction = snap.unitId(i) % 2;
      const state = snap.state(i) & 0xff;
      const packed = facing | (spriteIndex << 8) | ((faction & 0x0f) << 20) | (state << 24);
      data.setUint32(offset + 12, packed >>> 0, true);
      visibleCount++;
    }

    if (visibleCount === 0) {
      this.drawn = 0;
      gl.viewport(0, 0, this.canvas.width, this.canvas.height);
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
      return;
    }

    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.bufferData(gl.ARRAY_BUFFER, new Uint8Array(this.instanceBytes, 0, visibleCount * INSTANCE_STRIDE), gl.DYNAMIC_DRAW);
    gl.bindVertexArray(vao);
    gl.useProgram(program);
    gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);

    gl.uniform2f(u.viewSize, cam.viewW, cam.viewH);
    gl.uniform1f(u.zoom, zoom);
    gl.uniform2f(u.camOffset, camOffsetX, camOffsetY);
    gl.uniform1f(u.pxPerM, PX_PER_M);
    gl.uniform1f(u.vScale, TILE_H / TILE_M / 2);
    gl.uniform1f(u.zScale, Z_SCALE);
    gl.uniform1f(u.worldSize, cam.worldSizeM);
    gl.uniform1f(u.spriteWidth, Math.max(1, spriteWidth));
    gl.uniform1f(u.spriteHeight, Math.max(1, spriteHeight));
    gl.uniform1i(u.renderMode, close && this.assetsReady ? 0 : 1);
    gl.uniform1i(u.spriteTex, 0);
    gl.uniform4f(u.faction0, 0.239, 0.478, 0.722, 1);
    gl.uniform4f(u.faction1, 0.851, 0.502, 0.196, 1);

    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.spriteTexture);
    gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, visibleCount);
    gl.bindVertexArray(null);
    this.drawn = visibleCount;
  }

  private drawFallback(
    ctx: CanvasRenderingContext2D,
    cam: Camera,
    snap: SnapshotView,
    interp: InterpolatedPositions,
    alpha: number,
    groundHeight: (x: number, y: number) => number,
  ): void {
    const n = snap.count;
    const pxPerM = cam.pxPerM;
    const size = Math.max(1, 0.7 * pxPerM);
    const margin = size * 4;
    let drawn = 0;
    ctx.save();
    for (let i = 0; i < n; i++) {
      const x = interp.x(i, alpha);
      const y = interp.y(i, alpha);
      const p = cam.worldToScreen(x, y, groundHeight(x, y));
      if (p.sx < -margin || p.sy < -margin || p.sx > cam.viewW + margin || p.sy > cam.viewH + margin) continue;
      const faction = snap.unitId(i) % FACTION_COLORS.length;
      const state = snap.state(i);
      ctx.fillStyle = state === SoldierState.Dead || state === SoldierState.Downed
        ? "rgba(60,40,40,0.65)"
        : FACTION_COLORS[faction]!;
      const h = cam.lod <= Lod.Tactical ? Math.max(2, 1.7 * pxPerM) : size;
      ctx.fillRect(p.sx - size / 2, p.sy - h, size, h);
      drawn++;
    }
    ctx.restore();
    this.drawn = drawn;
  }
}

export { FACTION_COLORS };
