/**
 * 事前生成したビジュアルアセットの読み込み。
 *
 * 地形画像は `web/public/assets` に置かれている。素材が見つからない場合は
 * 読み込みを失敗させず、その場で手続き生成した地形へフォールバックする。
 * 人物はRust/Wasm生成ポリゴンへ統一されており、画像アセットを使用しない。
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
