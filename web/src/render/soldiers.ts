/**
 * 兵士の描画。
 *
 * M2 の描画経路は、兵士専用の WebGL2 キャンバスへインスタンス描画する。
 * クアッドは 1 枚だけを共有し、位置・標高・向き・スプライト番号・陣営・状態を
 * 16 バイトのインスタンス属性へ詰めて 1 ドローコールで送る。
 *
 * 人物画像は ImageGen で事前生成した PNG をテクスチャとして読み込む。近距離は
 * v2 runtime profile と manifest により「兵科 × 状態→行動 × 8方向 × 8スロット」
 * を選び、可視なシート単位で描画する。v2シートが未生成なら既存の1枚絵へ、
 * WebGL2が使えなければ Canvas2D の簡易描画へフォールバックする。
 */

import { createContext, createProgram } from "./gl";
import { Camera, Lod, PX_PER_M, TILE_H, TILE_M, Z_SCALE } from "./iso";
import type { InterpolatedPositions, SnapshotView } from "../sim/snapshot";
import { SoldierState } from "../sim/snapshot";
import { loadSoldierSprite } from "./generated-assets";
import { loadSpriteRuntime } from "./sprite-atlas";
import type { SpriteRuntime } from "./sprite-atlas";
import { getQuality } from "../quality";

/** 陣営色。色覚多様性に配慮し、赤/青ではなく青/橙にする（仕様 09 章 9 節）。 */
const FACTION_COLORS = ["#3d7ab8", "#d98032"];

const INSTANCE_STRIDE = 16;

/**
 * 会戦・戦域 LOD（個体が数 px 以下の点になる距離）での間引き目標。
 *
 * この距離では 1 体ずつ描いても見た目はほぼ変わらないが、フレームごとに
 * インスタンスバッファへ書き込む JS 側のループ（GPU 側のインスタンス
 * 描画そのものではない）が兵士数に比例して重くなる。M9 の 50,000 体規模で
 * 60 fps を保つため、視覚的に意味のある密度まで間引く（仕様 12 章 M9
 * 「描画の最適化（視錐台カリング、L3/L4 の間引き）」）。目標値は品質
 * プリセット（`quality.ts`）で変わる。
 */
/**
 * スプライトを選ぶときの「姿勢」。
 *
 * 転倒中の兵士は倒れているので、`Downed` の寝姿を借りる。状態そのものは
 * `Engaged` などのまま（シミュレーションでは生きて戦っている）なので、
 * 絵の選択だけを差し替える。
 */
function poseState(snap: SnapshotView, i: number): SoldierState {
  return snap.isStumbling(i) ? SoldierState.Downed : snap.state(i);
}

function thinningStride(n: number, lod: Lod): number {
  if (lod < Lod.Battle) return 1;
  return Math.max(1, Math.floor(n / getQuality().thinTargetInstances));
}

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
uniform float u_sheetColumns;
uniform float u_sheetRows;

out vec2 v_uv;
flat out uint v_packed;

void main() {
  float direction = float(i_packed & 255u);
  float frame = float((i_packed >> 8u) & 4095u);

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
  gl_Position = vec4(ndc, depth * 2.0 - 1.0, 1.0);
  v_uv = vec2(
    (direction + a_quad.x) / u_sheetColumns,
    (frame + 1.0 - a_quad.y) / u_sheetRows
  );
  v_packed = i_packed;
}
`;

const FRAG_SRC = `#version 300 es
precision mediump float;
precision highp int;

uniform sampler2D u_spriteTex;
uniform float u_dotBlend;
uniform vec4 u_faction0;
uniform vec4 u_faction1;

in vec2 v_uv;
flat in uint v_packed;
out vec4 outColor;

void main() {
  uint faction = (v_packed >> 20u) & 15u;
  uint state = (v_packed >> 24u) & 255u;
  bool fallen = ((v_packed >> 16u) & 1u) == 1u;
  bool braced = ((v_packed >> 17u) & 1u) == 1u;
  vec4 factionColor = faction == 1u ? u_faction1 : u_faction0;

  // L2/L3/L4 は小さなクアッド／点に落とす。状態は色の明度に反映し、
  // 画面を遠ざけても状態と陣営の違いを失わないようにする。
  vec3 dotColor = factionColor.rgb;
  if (state == 8u || state == 9u) dotColor = mix(dotColor, vec3(0.92, 0.48, 0.20), 0.35);
  if (state == 10u) dotColor = mix(dotColor, vec3(0.35, 0.86, 0.52), 0.28);
  float dotAlpha = 0.96;
  if (fallen) {
    // 転倒中は横倒しだが生きている。死体（下）より明るく残す
    float edge = smoothstep(0.0, 0.18, v_uv.y) * smoothstep(1.0, 0.82, v_uv.y);
    dotColor = mix(factionColor.rgb, vec3(0.95, 0.75, 0.35), 0.30);
    dotAlpha = 0.90 * edge;
  }
  if (braced) dotColor = mix(dotColor, vec3(0.72, 0.84, 1.00), 0.30);
  if (state >= 12u) {
    // 遠距離では Downed / Dead を横倒しの暗い個体として残す。近距離では
    // manifest の dead 静止バリエーションへ滑らかにクロスフェードする。
    float edge = smoothstep(0.0, 0.18, v_uv.y) * smoothstep(1.0, 0.82, v_uv.y);
    dotColor = factionColor.rgb * 0.48;
    dotAlpha = 0.68 * edge;
  }
  vec4 dot = vec4(dotColor, dotAlpha);

  if (u_dotBlend >= 0.999) {
    outColor = dot;
    return;
  }

  vec4 sprite = texture(u_spriteTex, v_uv);
  if (sprite.a <= 0.01) {
    if (u_dotBlend <= 0.001) discard;
    outColor = vec4(dot.rgb, dot.a * u_dotBlend);
    return;
  }
  // 2陣営目だけをシェーダ内でパレット寄せする。将来の本格的な
  // パレット置換でも、このインスタンス属性と描画経路はそのまま使える。
  if (faction == 1u) sprite.rgb = mix(sprite.rgb, factionColor.rgb, 0.28);
  if (state >= 12u) sprite.rgb *= 0.62;
  // 転倒中は倒れた絵のまま、死体より明るく（まだ生きている）
  if (fallen) sprite.rgb = mix(sprite.rgb * 0.88, vec3(0.95, 0.75, 0.35), 0.12);
  // 受け止める姿勢は盾のきらめきとして薄く乗せる
  if (braced) sprite.rgb = mix(sprite.rgb, vec3(0.80, 0.88, 1.00), 0.14);

  // 隣接 LOD 間はクロスフェード帯（px/m で ±15%）を設け、両方を描いて
  // アルファでブレンドする（仕様 08 章 2.3 節）。
  outColor = mix(sprite, dot, u_dotBlend);
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
    dotBlend: WebGLUniformLocation;
    sheetColumns: WebGLUniformLocation;
    sheetRows: WebGLUniformLocation;
    spriteTex: WebGLUniformLocation;
    faction0: WebGLUniformLocation;
    faction1: WebGLUniformLocation;
  } | null;
  private instanceBytes = new ArrayBuffer(0);
  private assetsReady = false;
  private spriteRuntime: SpriteRuntime | null = null;
  private readonly v2Textures = new Map<string, WebGLTexture>();

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
      dotBlend: location("u_dotBlend"),
      sheetColumns: location("u_sheetColumns"),
      sheetRows: location("u_sheetRows"),
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
    const [sprite, runtime] = await Promise.all([
      loadSoldierSprite(),
      loadSpriteRuntime().catch((error) => {
        console.warn("v2スプライト設定を読み込めないため従来画像を使います", error);
        return null;
      }),
    ]);
    const gl = this.gl;
    gl.bindTexture(gl.TEXTURE_2D, this.spriteTexture);
    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, sprite);
    this.spriteRuntime = runtime;
    for (const sheet of runtime?.sheets ?? []) {
      const texture = gl.createTexture();
      if (!texture) throw new Error(`v2スプライト用テクスチャを作成できません: ${sheet.url}`);
      gl.bindTexture(gl.TEXTURE_2D, texture);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, sheet.image);
      this.v2Textures.set(sheet.url, texture);
    }
    this.assetsReady = true;
  }

  draw(
    fallbackCtx: CanvasRenderingContext2D,
    cam: Camera,
    snap: SnapshotView,
    interp: InterpolatedPositions,
    alpha: number,
    elapsedSeconds: number,
    groundHeight: (x: number, y: number) => number,
  ): void {
    if (!this.gl || !this.program || !this.vao || !this.instanceBuffer || !this.uniform) {
      this.drawFallback(fallbackCtx, cam, snap, interp, alpha, groundHeight);
      return;
    }
    this.drawInstanced(cam, snap, interp, alpha, elapsedSeconds, groundHeight);
  }

  private drawInstanced(
    cam: Camera,
    snap: SnapshotView,
    interp: InterpolatedPositions,
    alpha: number,
    elapsedSeconds: number,
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
    // 隣接 LOD 間はクロスフェード帯（px/m で ±15%）を設け、両方を描いて
    // アルファでブレンドする（仕様 08 章 2.3 節）。スプライト（等身大の絵）と
    // 点描画の切り替わりは Tactical/Unit 境界（12 px/m）、点のサイズが
    // 固定になる切り替わりは Unit/Battle 境界（3 px/m）で起きる。
    const crossfadeT = (scale: number, threshold: number): number => {
      const band = threshold * 0.15;
      return Math.min(1, Math.max(0, (scale - (threshold - band)) / (2 * band)));
    };
    const tSpriteVsDot = crossfadeT(pxPerM, 12);
    let spriteHeight: number;
    let dotBlend: number;
    if (tSpriteVsDot > 0) {
      const spriteSideHeight = 1.7 * pxPerM;
      const dotSideHeight = 0.7 * pxPerM;
      spriteHeight = dotSideHeight + (spriteSideHeight - dotSideHeight) * tSpriteVsDot;
      dotBlend = 1 - tSpriteVsDot;
    } else {
      const tUnitVsBattle = crossfadeT(pxPerM, 3);
      const unitSideHeight = 0.7 * pxPerM;
      const battleSideHeight = 1;
      spriteHeight = battleSideHeight + (unitSideHeight - battleSideHeight) * tUnitVsBattle;
      dotBlend = 1;
    }
    // 低品質プリセットではスプライトのテクスチャサンプリング・アニメーション
    // 計算を丸ごと省き、常に点/矩形表現にする（`quality.ts`）。
    if (getQuality().forceDotsOnly) dotBlend = 1;
    const spriteWidth = dotBlend < 1 ? spriteHeight * (200 / 300) : spriteHeight;

    if (n === 0) {
      this.drawn = 0;
      gl.viewport(0, 0, this.canvas.width, this.canvas.height);
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
      return;
    }

    const margin = Math.max(spriteWidth, spriteHeight) * 1.5;
    // 少数個体では毎個体の射影判定の方が高くつく。GPU は画面外のクアッドを
    // そのままクリップできるため、5,000 体規模でだけ CPU カリングを有効にする。
    const shouldCull = n > 3000;
    const stride = thinningStride(n, cam.lod);
    type DrawGroup = {
      texture: WebGLTexture;
      columns: number;
      rows: number;
      indices: number[];
    };
    const groups = new Map<string, DrawGroup>();
    const fallbackGroup = (): DrawGroup => {
      const key = "__fallback__";
      let group = groups.get(key);
      if (!group) {
        group = { texture: this.spriteTexture!, columns: 1, rows: 1, indices: [] };
        groups.set(key, group);
      }
      return group;
    };
    const useSprites = this.assetsReady && dotBlend < 0.999;
    let visibleCount = 0;
    for (let i = 0; i < n; i += stride) {
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

      if (!useSprites) {
        fallbackGroup().indices.push(i);
      } else {
        // 転倒中は「倒れている」姿勢のスプライトを使う。死体と同じ絵だが、
        // シェーダ側で明度を分けて区別する（死体より明るい）。
        const resolved = this.spriteRuntime?.resolve(snap.troopType(i), poseState(snap, i));
        const texture = resolved ? this.v2Textures.get(resolved.sheet.url) : undefined;
        if (!resolved || !texture) {
          fallbackGroup().indices.push(i);
        } else {
          let group = groups.get(resolved.sheet.url);
          if (!group) {
            group = { texture, columns: 8, rows: 8, indices: [] };
            groups.set(resolved.sheet.url, group);
          }
          group.indices.push(i);
        }
      }
      visibleCount++;
    }

    if (visibleCount === 0) {
      this.drawn = 0;
      gl.viewport(0, 0, this.canvas.width, this.canvas.height);
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
      return;
    }

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
    gl.uniform1f(u.dotBlend, this.assetsReady ? dotBlend : 1);
    gl.uniform1i(u.spriteTex, 0);
    gl.uniform4f(u.faction0, 0.239, 0.478, 0.722, 1);
    gl.uniform4f(u.faction1, 0.851, 0.502, 0.196, 1);

    gl.activeTexture(gl.TEXTURE0);
    for (const group of groups.values()) {
      const count = group.indices.length;
      if (this.instanceBytes.byteLength < count * INSTANCE_STRIDE) {
        this.instanceBytes = new ArrayBuffer(count * INSTANCE_STRIDE);
      }
      const data = new DataView(this.instanceBytes);
      for (let slot = 0; slot < count; slot++) {
        const i = group.indices[slot]!;
        const offset = slot * INSTANCE_STRIDE;
        const x = interp.x(i, alpha);
        const y = interp.y(i, alpha);
        data.setFloat32(offset, x, true);
        data.setFloat32(offset + 4, y, true);
        data.setFloat32(offset + 8, groundHeight(x, y), true);

        let direction = 0;
        let frame = 0;
        const resolved = group.columns === 8
          ? this.spriteRuntime?.resolve(snap.troopType(i), poseState(snap, i))
          : null;
        if (resolved) {
          direction = Math.round((snap.facing(i) / 65536) * 8) & 7;
          const stable = Math.imul(i + 1, 0x9e3779b1) >>> 0;
          if (resolved.playback.mode === "variant-loop") {
            const variant = stable % resolved.playback.variantCount;
            const phase = Math.floor(elapsedSeconds * resolved.playback.framesPerSecond) %
              resolved.playback.framesPerVariant;
            frame = variant * resolved.playback.framesPerVariant + phase;
          } else if (resolved.playback.mode === "static-variants") {
            frame = stable % resolved.playback.variantCount;
          } else {
            frame = (Math.floor(elapsedSeconds * resolved.playback.framesPerSecond) + (stable & 7)) & 7;
          }
        }
        const faction = snap.faction(i) & 0x0f;
        const state = snap.state(i) & 0xff;
        const fallen = snap.isStumbling(i) ? 1 : 0;
        const braced = snap.isBraced(i) ? 1 : 0;
        const packed = direction | (frame << 8) | (fallen << 16) | (braced << 17) |
          (faction << 20) | (state << 24);
        data.setUint32(offset + 12, packed >>> 0, true);
      }
      gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
      gl.bufferData(
        gl.ARRAY_BUFFER,
        new Uint8Array(this.instanceBytes, 0, count * INSTANCE_STRIDE),
        gl.DYNAMIC_DRAW,
      );
      gl.uniform1f(u.sheetColumns, group.columns);
      gl.uniform1f(u.sheetRows, group.rows);
      gl.bindTexture(gl.TEXTURE_2D, group.texture);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, count);
    }
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
    const stride = thinningStride(n, cam.lod);
    let drawn = 0;
    ctx.save();
    for (let i = 0; i < n; i += stride) {
      const x = interp.x(i, alpha);
      const y = interp.y(i, alpha);
      const p = cam.worldToScreen(x, y, groundHeight(x, y));
      if (p.sx < -margin || p.sy < -margin || p.sx > cam.viewW + margin || p.sy > cam.viewH + margin) continue;
      const faction = snap.faction(i) % FACTION_COLORS.length;
      const state = snap.state(i);
      ctx.fillStyle = state === SoldierState.Dead || state === SoldierState.Downed
        ? "rgba(60,40,40,0.65)"
        : FACTION_COLORS[faction]!;
      if (snap.isStumbling(i)) {
        // 転倒中は横倒しの平たい矩形にする（倒れているのが一目で分かる）
        const w = Math.max(2, 1.4 * pxPerM);
        const h = Math.max(1, 0.4 * pxPerM);
        ctx.fillRect(p.sx - w / 2, p.sy - h, w, h);
        drawn++;
        continue;
      }
      const h = cam.lod <= Lod.Tactical ? Math.max(2, 1.7 * pxPerM) : size;
      ctx.fillRect(p.sx - size / 2, p.sy - h, size, h);
      drawn++;
    }
    ctx.restore();
    this.drawn = drawn;
  }
}

export { FACTION_COLORS };
