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
 * 地形タイルアトラスが見つからないときの手続き生成フォールバック。
 * 4x4 のタイルそれぞれを、地表らしき単色で塗り分けるだけの最小限のもの。
 */
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
  const colors = [
    "#5b7a4a",
    "#7fa15c",
    "#8f9a5a",
    "#6b6247",
    "#9c8b6a",
    "#8a8a8a",
    "#3d5a80",
    "#c9b896",
  ];
  for (let ty = 0; ty < rows; ty++) {
    for (let tx = 0; tx < cols; tx++) {
      ctx.fillStyle = colors[(ty * cols + tx) % colors.length]!;
      ctx.fillRect(tx * tileSize, ty * tileSize, tileSize, tileSize);
    }
  }
  return {
    width,
    height,
    tileWidth: tileSize,
    tileHeight: tileSize,
    pixels: ctx.getImageData(0, 0, width, height).data,
  };
}

export async function loadTerrainAtlas(): Promise<TerrainAtlas> {
  let image: HTMLImageElement;
  try {
    image = await loadImage("/assets/terrain-atlas.png");
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
  const image = await loadImage("/assets/soldier-blue.png");
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
