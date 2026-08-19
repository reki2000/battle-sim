/**
 * 事前生成したビジュアルアセットの読み込み。
 *
 * 画像は `web/public/assets` に置かれており、ビルド時には生成しない
 * （仕様 08 章 4.2 節）。素材が見つからない場合（404 や manifest 未掲載）は
 * 読み込みを失敗させず、その場で手続き生成した簡易プレースホルダに
 * フォールバックする。シミュレーションと描画自体は常に成立させるための
 * 保険であり、本番品質の見た目を代替するものではない。
 *
 * 人物画像だけは ImageGen の出力に残ったチェッカーボード風の背景を、
 * 起動時に外周からの連結領域としてアルファ化する。これは生成処理ではなく、
 * 既存 PNG を Canvas へ読み込む際のマスク処理である。
 */

import { TerrainTile, TILE_COLORS } from "./terrain-tile";

export interface TerrainAtlas {
  width: number;
  height: number;
  tileWidth: number;
  tileHeight: number;
  pixels: Uint8ClampedArray;
}

function loadImage(url: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.decoding = "async";
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error(`画像アセットを読み込めません: ${url}`));
    image.src = url;
  });
}

/**
 * 座標から決定論的な疑似乱数を作る（0..1）。`Math.random` は使わない
 * （同じ入力で毎回同じ絵になる必要がある。下記 `proceduralTerrainAtlas` 参照）。
 *
 * 単純な線形結合（`(px*a + py*b) % n`）だと周期がそのまま画面に縞模様として
 * 出てしまう（水面のような広い単色面で特に目立つ）。ビット拡散を挟んで
 * 粒状のディザに見えるようにする。
 */
function hash2(x: number, y: number, salt: number): number {
  let h = (x * 374761393 + y * 668265263 + salt * 2246822519) | 0;
  h = Math.imul(h ^ (h >>> 13), 1274126177);
  h ^= h >>> 16;
  return (h >>> 0) / 4294967296;
}

/**
 * 地形タイルアトラスが見つからないときの手続き生成フォールバック。
 *
 * 16 枚のタイルを、それぞれの基準色（`TILE_COLORS`）で塗り分ける。
 * ミニマップと同じ色を引くので、アトラスが無くても地図の読み方は変わらない
 * ——草地は緑、水は青、耕地は黄土色のまま。ここで別の色表を持つと、
 * アトラスの有無で水と森が入れ替わって見える。
 *
 * 単色のままだと 2 m タイルが一様でのっぺりするので、タイル内に決定論的な
 * 微弱な明暗を入れて地面らしい粒状感だけ足す。
 *
 * ただし水面は除く。1 セル＝1 タイルとして同じ 64×64 画像をそのまま並べて
 * 貼るので、湖のように何十セルも連続する場所では「同じ粒状パターンの
 * 繰り返し」がそのまま縞・モザイクとして見えてしまう（陸は地質や標高陰影が
 * セルごとに変わるので目立たないが、水面は単色に近く継ぎ目が丸見えになる）。
 * 水は最初から質感の主張が無いほうが自然なので、水タイルだけは単色にする。
 */
const WATER_TILES = new Set<number>([
  TerrainTile.DEEP_WATER,
  TerrainTile.SHALLOW_WATER,
  TerrainTile.FORD,
]);

function proceduralTerrainAtlas(tileSize = 64): TerrainAtlas {
  const cols = 4;
  const rows = 4;
  const width = tileSize * cols;
  const height = tileSize * rows;
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) throw new Error("地形アトラス用 Canvas を作成できません");

  const image = ctx.createImageData(width, height);
  for (let ty = 0; ty < rows; ty++) {
    for (let tx = 0; tx < cols; tx++) {
      const tile = ty * cols + tx;
      const [r, g, b] = TILE_COLORS[tile] ?? TILE_COLORS[TerrainTile.GRASS]!;
      const isWater = WATER_TILES.has(tile);
      for (let py = 0; py < tileSize; py++) {
        for (let px = 0; px < tileSize; px++) {
          // 座標から決まる ±6% の明暗（粒状のディザ、縞にはならない）。
          // 水タイルは繰り返しが目立つので単色のまま（上のコメント参照）
          const n = isWater ? 0 : hash2(px, py, tile) - 0.5;
          const k = 1 + n * 0.12;
          const o = ((ty * tileSize + py) * width + tx * tileSize + px) * 4;
          image.data[o] = Math.min(255, Math.round(r * k));
          image.data[o + 1] = Math.min(255, Math.round(g * k));
          image.data[o + 2] = Math.min(255, Math.round(b * k));
          image.data[o + 3] = 255;
        }
      }
    }
  }
  ctx.putImageData(image, 0, 0);

  return {
    width,
    height,
    tileWidth: tileSize,
    tileHeight: tileSize,
    pixels: image.data,
  };
}

export async function loadTerrainAtlas(): Promise<TerrainAtlas> {
  let image: HTMLImageElement;
  try {
    image = await loadImage(`${import.meta.env.BASE_URL}assets/terrain-atlas.png`);
  } catch (err) {
    console.warn("地形タイルアトラスが見つからないため、手続き生成のプレースホルダを使う", err);
    return proceduralTerrainAtlas();
  }
  const canvas = document.createElement("canvas");
  canvas.width = image.naturalWidth;
  canvas.height = image.naturalHeight;
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) throw new Error("地形アトラス用 Canvas を作成できません");
  ctx.drawImage(image, 0, 0);

  return {
    width: canvas.width,
    height: canvas.height,
    tileWidth: Math.floor(canvas.width / 4),
    tileHeight: Math.floor(canvas.height / 4),
    pixels: ctx.getImageData(0, 0, canvas.width, canvas.height).data,
  };
}

function isCheckerboardPixel(r: number, g: number, b: number): boolean {
  const min = Math.min(r, g, b);
  const max = Math.max(r, g, b);
  const average = (r + g + b) / 3;
  // ImageGen の背景は白〜薄灰色の無彩色チェッカーボード。兵士の
  // 金属や布地にある同系色は、外周から連結していない限り残す。
  return max - min <= 18 && average >= 160;
}

/**
 * ImageGen の人物 PNG から、外周に連結したチェッカーボード背景だけを除く。
 * シールドの白い紋章など、人物内部の明るい部分は連結していないので保たれる。
 */
export async function loadSoldierSprite(): Promise<HTMLCanvasElement> {
  const image = await loadImage(`${import.meta.env.BASE_URL}assets/soldier-blue.png`);
  const canvas = document.createElement("canvas");
  canvas.width = image.naturalWidth;
  canvas.height = image.naturalHeight;
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) throw new Error("人物画像用 Canvas を作成できません");
  ctx.drawImage(image, 0, 0);

  const pixels = ctx.getImageData(0, 0, canvas.width, canvas.height);
  const { data } = pixels;
  const visited = new Uint8Array(canvas.width * canvas.height);
  const queue = new Int32Array(canvas.width * canvas.height);
  let head = 0;
  let tail = 0;

  const enqueue = (x: number, y: number): void => {
    const i = y * canvas.width + x;
    if (visited[i]) return;
    const p = i * 4;
    if (!isCheckerboardPixel(data[p]!, data[p + 1]!, data[p + 2]!)) return;
    visited[i] = 1;
    queue[tail++] = i;
  };

  for (let x = 0; x < canvas.width; x++) {
    enqueue(x, 0);
    enqueue(x, canvas.height - 1);
  }
  for (let y = 1; y < canvas.height - 1; y++) {
    enqueue(0, y);
    enqueue(canvas.width - 1, y);
  }

  while (head < tail) {
    const i = queue[head++]!;
    const x = i % canvas.width;
    const y = Math.floor(i / canvas.width);
    if (x > 0) enqueue(x - 1, y);
    if (x + 1 < canvas.width) enqueue(x + 1, y);
    if (y > 0) enqueue(x, y - 1);
    if (y + 1 < canvas.height) enqueue(x, y + 1);
  }

  for (let i = 0; i < visited.length; i++) {
    if (visited[i]) data[i * 4 + 3] = 0;
  }
  ctx.putImageData(pixels, 0, 0);
  return canvas;
}
