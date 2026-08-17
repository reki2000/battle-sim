/**
 * スキルが生成した v2 manifest とシミュレーター視覚プロファイルを読み込む。
 *
 * シミュレーターは兵科・状態→行動の意味を所有し、生成スキルは各行動の
 * 8方向×8位相画像と物理パスだけを供給する。
 */

export const SPRITE_DIRECTIONS = [
  "south",
  "southeast",
  "east",
  "northeast",
  "north",
  "northwest",
  "west",
  "southwest",
] as const;

export const SOLDIER_STATE_IDS = [
  "idle",
  "marching",
  "repositioning",
  "advancing",
  "charging",
  "engaged",
  "shooting",
  "reloading",
  "wavering",
  "broken",
  "rallying",
  "working",
  "downed",
  "dead",
] as const;

export type SoldierStateId = (typeof SOLDIER_STATE_IDS)[number];

export interface ActionPlayback {
  mode: "cycle" | "variant-loop" | "static-variants";
  variantCount: number;
  framesPerVariant: number;
  framesPerSecond: number;
  loop: boolean;
}

export interface RuntimeSpriteRole {
  id: string;
  troopTypeId: string;
  troopTypeIndex: number;
  name: string;
  description: string;
  actions: string[];
  stateActions: Record<SoldierStateId, string>;
}

export interface SpriteRuntimeProfile {
  schemaVersion: 1;
  id: string;
  assetSetId: string;
  historicalContext: {
    period: { id: string; label: string };
    region: { id: string; label: string };
  };
  styleId: string;
  actionPlayback: Record<string, ActionPlayback>;
  roles: RuntimeSpriteRole[];
}

interface ManifestRole {
  actions: Record<string, string>;
}

interface SpriteManifestV2 {
  version: 2;
  cell: { width: number; height: number };
  columns: { kind: "direction"; values: string[] };
  rows: { kind: "frame"; values: number[] };
  sets: Record<
    string,
    {
      declaredRoles: string[];
      roles: Record<string, ManifestRole>;
    }
  >;
}

export interface SpriteSheet {
  image: HTMLCanvasElement;
  columns: 8;
  rows: 8;
  cellWidth: number;
  cellHeight: number;
  url: string;
}

export interface ResolvedSprite {
  role: RuntimeSpriteRole;
  action: string;
  playback: ActionPlayback;
  sheet: SpriteSheet;
}

export interface SpriteRuntime {
  profile: SpriteRuntimeProfile;
  resolve(troopTypeIndex: number, state: number): ResolvedSprite | null;
  sheets: readonly SpriteSheet[];
}

async function loadImage(url: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.decoding = "async";
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error(`スプライトシートを読み込めません: ${url}`));
    image.src = url;
  });
}

async function fetchJson<T>(url: string, optional = false): Promise<T | null> {
  const response = await fetch(url);
  if (optional && response.status === 404) return null;
  if (!response.ok) throw new Error(`${url} の読み込みに失敗しました: HTTP ${response.status}`);
  return (await response.json()) as T;
}

function validateProfile(profile: SpriteRuntimeProfile): void {
  if (profile.schemaVersion !== 1 || !profile.id || !profile.assetSetId || !profile.roles.length) {
    throw new Error("スプライト実行時プロファイルの基本フィールドが不正です");
  }
  const indices = new Set<number>();
  for (const [action, playback] of Object.entries(profile.actionPlayback)) {
    const shape = `${playback.mode}:${playback.variantCount}x${playback.framesPerVariant}:${playback.loop}`;
    const valid =
      shape === "cycle:1x8:true" ||
      shape === "variant-loop:4x2:true" ||
      shape === "static-variants:8x1:false";
    const validRate = Number.isInteger(playback.framesPerSecond) && playback.framesPerSecond >= 0 &&
      (playback.mode === "static-variants") === (playback.framesPerSecond === 0);
    if (!valid || !validRate) throw new Error(`${action}: actionPlayback が不正です`);
  }
  for (const role of profile.roles) {
    if (indices.has(role.troopTypeIndex) || role.troopTypeIndex < 0) {
      throw new Error(`兵科indexが重複または不正です: ${role.troopTypeIndex}`);
    }
    indices.add(role.troopTypeIndex);
    for (const state of SOLDIER_STATE_IDS) {
      const action = role.stateActions[state];
      if (!action || !role.actions.includes(action)) {
        throw new Error(`${role.id}: stateActions.${state} が未定義の行動を参照しています`);
      }
      if (!profile.actionPlayback[action]) {
        throw new Error(`${role.id}: ${action} のactionPlaybackがありません`);
      }
    }
  }
}

function validateManifest(manifest: SpriteManifestV2): void {
  if (
    manifest.version !== 2 ||
    manifest.cell.width !== 200 ||
    manifest.cell.height !== 300 ||
    manifest.columns.values.join(",") !== SPRITE_DIRECTIONS.join(",") ||
    manifest.rows.values.join(",") !== "0,1,2,3,4,5,6,7"
  ) {
    throw new Error("v2 sprite manifest が 200x300・8方向・8位相契約と一致しません");
  }
}

function assetUrl(path: string): string {
  if (!path.startsWith("sprites/v2/sets/") || path.includes("..") || path.startsWith("/")) {
    throw new Error(`manifest に非正規パスがあります: ${path}`);
  }
  return `${import.meta.env.BASE_URL}assets/${path}`;
}

async function loadSpriteSheet(url: string): Promise<SpriteSheet> {
  const image = await loadImage(url);
  if (image.naturalWidth !== 1600 || image.naturalHeight !== 2400) {
    throw new Error(`${url}: ${image.naturalWidth}x${image.naturalHeight}; 1600x2400 が必要です`);
  }
  const canvas = document.createElement("canvas");
  canvas.width = image.naturalWidth;
  canvas.height = image.naturalHeight;
  const ctx = canvas.getContext("2d", { willReadFrequently: false });
  if (!ctx) throw new Error("スプライトシート用 Canvas を作成できません");
  ctx.drawImage(image, 0, 0);
  return {
    image: canvas,
    columns: 8,
    rows: 8,
    cellWidth: 200,
    cellHeight: 300,
    url,
  };
}

/**
 * プロファイルは必須、manifest は生成前でも起動できるよう任意とする。
 * manifestがまだ無い、または該当シートが未生成なら resolve は null を返す。
 */
export async function loadSpriteRuntime(profileId = "medieval_western"): Promise<SpriteRuntime> {
  const profile = await fetchJson<SpriteRuntimeProfile>(
    `${import.meta.env.BASE_URL}assets/sprites/v2/profiles/${profileId}.json`,
  );
  if (!profile) throw new Error(`スプライト実行時プロファイル ${profileId} がありません`);
  validateProfile(profile);

  const manifest = await fetchJson<SpriteManifestV2>(
    `${import.meta.env.BASE_URL}assets/sprites/v2/manifest.json`,
    true,
  );
  const loaded = new Map<string, SpriteSheet>();
  if (manifest) {
    validateManifest(manifest);
    const set = manifest.sets[profile.assetSetId];
    if (set) {
      const urls = new Set<string>();
      for (const role of profile.roles) {
        const manifestRole = set.roles[role.id];
        if (!manifestRole) continue;
        for (const action of role.actions) {
          const path = manifestRole.actions[action];
          if (path) urls.add(assetUrl(path));
        }
      }
      const sheets = await Promise.all([...urls].map(loadSpriteSheet));
      for (const sheet of sheets) loaded.set(sheet.url, sheet);
    }
  }

  const roleByTroopType = new Map(profile.roles.map((role) => [role.troopTypeIndex, role]));
  const manifestSet = manifest?.sets[profile.assetSetId];
  return {
    profile,
    sheets: [...loaded.values()],
    resolve(troopTypeIndex: number, state: number): ResolvedSprite | null {
      const role = roleByTroopType.get(troopTypeIndex);
      const stateId = SOLDIER_STATE_IDS[state];
      if (!role || !stateId || !manifestSet) return null;
      const action = role.stateActions[stateId];
      const path = manifestSet.roles[role.id]?.actions[action];
      if (!path) return null;
      const sheet = loaded.get(assetUrl(path));
      const playback = profile.actionPlayback[action];
      return sheet && playback ? { role, action, playback, sheet } : null;
    },
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

/** brad の0方向を south 列へ割り当てる。 */
export function directionFromFacing(facingRad: number): number {
  return Math.round((facingRad / (Math.PI * 2)) * 8) & 7;
}

/** 均等な8位相を返す。終端の複製は格納しない。 */
export function animationFrame(elapsedSeconds: number, framesPerSecond = 8): number {
  return Math.floor(Math.max(0, elapsedSeconds) * framesPerSecond) % 8;
}
