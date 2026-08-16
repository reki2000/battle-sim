/**
 * WebGL2 のコンテキスト管理とシェーダのコンパイル・リンク。
 *
 * 仕様 08 章 3〜4 節。地形と兵士はどちらも「1 枚のテクスチャ／1 回の
 * インスタンス描画」を GPU に投げる構成にする。ここには汎用のヘルパだけを置き、
 * 具体的な頂点レイアウトは呼び出し側（terrain.ts, soldiers.ts）が持つ。
 */

export function createContext(
  canvas: HTMLCanvasElement,
  options: { alpha?: boolean } = {},
): WebGL2RenderingContext {
  const gl = canvas.getContext("webgl2", {
    alpha: options.alpha ?? false,
    antialias: false,
    depth: true,
    // 地形は 1 枚のクアッドなので深度は使わないが、M2 で兵士のインスタンス
    // 描画を同じコンテキストに乗せるときに深度バッファ（x+y をエンコードした
    // もの、仕様 08 章 1.1 節）を使う。今のうちに有効化しておく。
    stencil: false,
    preserveDrawingBuffer: false,
    powerPreference: "high-performance",
  });
  if (!gl) {
    throw new Error("WebGL2 が利用できません");
  }
  return gl;
}

function compileShader(gl: WebGL2RenderingContext, type: number, source: string): WebGLShader {
  const shader = gl.createShader(type);
  if (!shader) throw new Error("シェーダの作成に失敗しました");
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    const log = gl.getShaderInfoLog(shader);
    gl.deleteShader(shader);
    throw new Error(`シェーダのコンパイルに失敗しました: ${log}`);
  }
  return shader;
}

export function createProgram(
  gl: WebGL2RenderingContext,
  vertexSrc: string,
  fragmentSrc: string,
): WebGLProgram {
  const vs = compileShader(gl, gl.VERTEX_SHADER, vertexSrc);
  const fs = compileShader(gl, gl.FRAGMENT_SHADER, fragmentSrc);
  const program = gl.createProgram();
  if (!program) throw new Error("プログラムの作成に失敗しました");
  gl.attachShader(program, vs);
  gl.attachShader(program, fs);
  gl.linkProgram(program);
  gl.deleteShader(vs);
  gl.deleteShader(fs);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    const log = gl.getProgramInfoLog(program);
    gl.deleteProgram(program);
    throw new Error(`プログラムのリンクに失敗しました: ${log}`);
  }
  return program;
}

/** デバイスの上限を超えないテクスチャ幅を返す。 */
export function clampTextureSize(gl: WebGL2RenderingContext, size: number): number {
  const max = gl.getParameter(gl.MAX_TEXTURE_SIZE) as number;
  return Math.min(size, max);
}
