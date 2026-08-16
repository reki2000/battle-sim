/**
 * 役職・行動・方向・フレームで参照する ImageGen スプライトシート。
 *
 * 各シートは 1536x1024 程度の 8x4 グリッドで、
 *   列 = south, southeast, east, northeast, north, northwest, west, southwest
 *   行 = 4つの連続したアニメーション位相
 * を表す。最後の行は最初の行の複製ではない。
 */

export const SPRITE_ROLES = [
  "heavy-infantry",
  "spearman",
  "pikeman",
  "longbowman",
  "crossbowman",
  "heavy-cavalry",
  "light-cavalry",
  "engineer",
] as const;

export type SpriteRole = (typeof SPRITE_ROLES)[number];

export const SPRITE_ACTIONS = ["idle", "walk", "run", "combat", "work"] as const;
export type SpriteAction = (typeof SPRITE_ACTIONS)[number];

export interface SpriteSheet {
  image: HTMLCanvasElement;
  columns: 8;
  rows: 4;
  cellWidth: number;
  cellHeight: number;
}

const AVAILABLE_SHEETS: Partial<Record<SpriteRole, Partial<Record<SpriteAction, string>>>> = {
  "heavy-infantry": {
    idle: "/assets/sprites/heavy-infantry/idle.png",
    walk: "/assets/sprites/heavy-infantry/walk.png",
    run: "/assets/sprites/heavy-infantry/run.png",
    combat: "/assets/sprites/heavy-infantry/combat.png",
    work: "/assets/sprites/heavy-infantry/work.png",
  },
  spearman: {
    idle: "/assets/sprites/spearman/idle.png",
    walk: "/assets/sprites/spearman/walk.png",
    run: "/assets/sprites/spearman/run.png",
    combat: "/assets/sprites/spearman/combat.png",
    work: "/assets/sprites/spearman/work.png",
  },
};

function loadImage(url: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.decoding = "async";
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error(`スプライトシートを読み込めません: ${url}`));
    image.src = url;
  });
}

function isCheckerboardPixel(r: number, g: number, b: number): boolean {
  const min = Math.min(r, g, b);
  const max = Math.max(r, g, b);
  return max - min <= 18 && (r + g + b) / 3 >= 160;
}

function removeCheckerboard(canvas: HTMLCanvasElement): void {
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) throw new Error("スプライトシート用 Canvas を作成できません");
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
}

export async function loadSpriteSheet(
  role: SpriteRole,
  action: SpriteAction,
): Promise<SpriteSheet | null> {
  const url = AVAILABLE_SHEETS[role]?.[action];
  if (!url) return null;

  const image = await loadImage(url);
  const canvas = document.createElement("canvas");
  canvas.width = image.naturalWidth;
  canvas.height = image.naturalHeight;
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) throw new Error("スプライトシート用 Canvas を作成できません");
  ctx.drawImage(image, 0, 0);

  const pixels = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
  let hasTransparency = false;
  for (let i = 3; i < pixels.length; i += 4) {
    if (pixels[i] !== 255) {
      hasTransparency = true;
      break;
    }
  }
  if (!hasTransparency) removeCheckerboard(canvas);

  return {
    image: canvas,
    columns: 8,
    rows: 4,
    cellWidth: canvas.width / 8,
    cellHeight: canvas.height / 4,
  };
}

export function spriteFrameRect(
  sheet: SpriteSheet,
  direction: number,
  frame: number,
): { sx: number; sy: number; sw: number; sh: number } {
  const column = ((direction % sheet.columns) + sheet.columns) % sheet.columns;
  const row = ((frame % sheet.rows) + sheet.rows) % sheet.rows;
  return {
    sx: column * sheet.cellWidth,
    sy: row * sheet.cellHeight,
    sw: sheet.cellWidth,
    sh: sheet.cellHeight,
  };
}

/** brad の0方向を南列に割り当てる簡易マッピング。 */
export function directionFromFacing(facingRad: number): number {
  return Math.round((facingRad / (Math.PI * 2)) * 8) & 7;
}

/** 描画経過時間から4段階の位相を返す。最終行を始点の複製にはしない。 */
export function animationFrame(elapsedSeconds: number, framesPerSecond = 8): number {
  return Math.floor(Math.max(0, elapsedSeconds) * framesPerSecond) % 4;
}
