/**
 * 低スペック端末向けの品質プリセット（M9）。
 *
 * `i18n.ts` と同じ形（モジュール内シングルトン + 変更時コールバック）を踏襲する。
 * 選択は `localStorage` に永続化し、次回起動時も同じ設定で立ち上がる。
 */

export type Quality = "low" | "medium" | "high";

export interface QualitySettings {
  /** デバイスピクセル比の上限。高いほど鮮明だがフィルレートを食う。 */
  dprCap: number;
  /** 会戦・戦域 LOD で間引く目標インスタンス数（`render/soldiers.ts`）。 */
  thinTargetInstances: number;
  /** ミニマップで間引く目標点数（`render/minimap.ts`）。 */
  thinTargetMinimapDots: number;
  /** true なら等身大スプライトを描かず、常に点/矩形表現に落とす
   * （テクスチャサンプリングとアニメーション計算を丸ごと省く）。 */
  forceDotsOnly: boolean;
}

const PRESETS: Record<Quality, QualitySettings> = {
  low: { dprCap: 1, thinTargetInstances: 2000, thinTargetMinimapDots: 1500, forceDotsOnly: true },
  medium: { dprCap: 1.5, thinTargetInstances: 6000, thinTargetMinimapDots: 4000, forceDotsOnly: false },
  high: { dprCap: 2, thinTargetInstances: 15000, thinTargetMinimapDots: 8000, forceDotsOnly: false },
};

const STORAGE_KEY = "battle-sim.quality";
const ORDER: Quality[] = ["low", "medium", "high"];

function loadInitial(): Quality {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored === "low" || stored === "medium" || stored === "high") return stored;
  } catch {
    // localStorage が使えない環境（プライベートモード等）では既定値を使う
  }
  return "medium";
}

let current: Quality = loadInitial();
const listeners: (() => void)[] = [];

export function getQualityName(): Quality {
  return current;
}

export function getQuality(): QualitySettings {
  return PRESETS[current];
}

export function setQuality(q: Quality): void {
  current = q;
  try {
    localStorage.setItem(STORAGE_KEY, q);
  } catch {
    // 保存できなくても実行時の設定切替は続行する
  }
  for (const fn of listeners) fn();
}

/** low → medium → high → low… と巡回させる。 */
export function cycleQuality(): void {
  const idx = ORDER.indexOf(current);
  setQuality(ORDER[(idx + 1) % ORDER.length]!);
}

export function onQualityChange(fn: () => void): void {
  listeners.push(fn);
}
