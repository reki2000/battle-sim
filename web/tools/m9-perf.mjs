/**
 * M9 の性能確認。50,000 体シナリオ（`?soldiers=50000`）を全ズーム域で
 * 開き、エラーなく描け、tick が進み続けることを確認する。
 *
 * この環境（ヘッドレス・ソフトウェアレンダリング Chromium）の fps 絶対値は
 * 実機の指標にならないため、参考値として出すのみで合否判定には使わない。
 * 合否は「エラーが出ないこと」「tick が単調に進むこと」「初期ロードが
 * 3 MB 以下であること」で行う。
 *
 *   npm run build && npm run preview &
 *   node tools/m9-perf.mjs
 */
import { chromium } from "playwright";
import { mkdirSync, statSync, readdirSync } from "node:fs";
import { join } from "node:path";

const URL = process.env.SMOKE_URL ?? "http://localhost:4173/";
const OUT = process.env.SMOKE_OUT ?? "smoke-out";
const EXEC = process.env.PLAYWRIGHT_CHROMIUM ?? undefined;

mkdirSync(OUT, { recursive: true });

// ── 初期ロードサイズ（M9 受け入れ条件: 3 MB 以下） ─────────────
//
// 「初回ロード」はシミュレーションを起動・操作可能にするために必須の
// アプリ本体（HTML/JS/wasm）を指す。兵科スプライトや地形アトラスなどの
// 事前生成アート（`public/assets/`、数十 MB）は `loadAssets()` で非同期・
// 失敗時フォールバック付きに読むもので、起動には不要（本文の try/catch
// 参照）なため、この予算には含めない。
function bundleSizeBytes(dir) {
  let total = 0;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === "sprites") continue;
      total += bundleSizeBytes(p);
    } else if (/\.(js|wasm|html|css)$/.test(entry.name)) {
      total += statSync(p).size;
    }
  }
  return total;
}
const distBytes = bundleSizeBytes("dist");
console.log(`初期ロード（HTML/JS/wasm）: ${(distBytes / 1024).toFixed(1)} KB`);
const errors = [];
if (distBytes > 3 * 1024 * 1024) {
  errors.push(`初期ロードが 3 MB を超えている: ${(distBytes / 1024 / 1024).toFixed(2)} MB`);
}

const browser = await chromium.launch(EXEC ? { executablePath: EXEC } : {});
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });

page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
page.on("console", (m) => {
  if (m.type() === "error" && !m.text().includes("favicon")) {
    errors.push(`console: ${m.text()}`);
  }
});

await page.goto(`${URL}?soldiers=50000`, { waitUntil: "networkidle" });
await page.keyboard.press("3"); // 4x
await page.waitForTimeout(10000); // 50,000 体の配置・初動が収まるまで待つ

const views = [
  ["q", "5km"],
  ["w", "1km"],
  ["e", "200m"],
  ["r", "40m"],
];

let lastTick = -1;
for (const [key, name] of views) {
  await page.keyboard.press(key);
  await page.waitForTimeout(1200);
  const hud = (await page.textContent("#hud")) ?? "";
  console.log(`${name.padEnd(6)} ${hud.replace(/\n/g, " | ")}`);
  await page.screenshot({ path: `${OUT}/m9-${name}.png` });

  const tick = Number(/tick (\d+)/.exec(hud)?.[1] ?? -1);
  if (tick <= lastTick) {
    errors.push(`シミュレーションが進んでいない (${name}: tick ${tick})`);
  }
  lastTick = tick;

  const soldierMatch = /兵士 (\d+)（描画 (\d+)）/.exec(hud);
  if (soldierMatch) {
    const [, total, drawn] = soldierMatch;
    console.log(`  兵士 ${total} 体中 ${drawn} 体を描画`);
  }
}

await browser.close();

if (errors.length) {
  console.error("\nNG:\n" + errors.join("\n"));
  process.exit(1);
}
console.log("\nOK: 50,000 体シナリオが全ズーム域でエラーなく動作");
