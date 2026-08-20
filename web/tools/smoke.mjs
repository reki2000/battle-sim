/**
 * ブラウザでの疎通確認。
 *
 * Rust → wasm → Worker → 描画の経路が通っていること、
 * 5 km 〜 10 m の全ズーム域でエラーなく描けることを確認する。
 * 各 LOD のスクリーンショットを web/smoke-out/ に出す。
 *
 *   npm run build && npm run preview &
 *   node tools/smoke.mjs
 */
import {
  collectPageErrors,
  ensureSmokeOut,
  launchBrowser,
  smokeOut as OUT,
  smokeUrl as URL,
} from "./smoke-helpers.mjs";

ensureSmokeOut();

const browser = await launchBrowser();
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });

const errors = [];
collectPageErrors(page, errors);

await page.goto(URL, { waitUntil: "networkidle" });
await page.keyboard.press("3"); // 4x
await page.waitForTimeout(8000);

const views = [
  ["q", "5km"],
  ["w", "1km"],
  ["e", "200m"],
  ["r", "40m"],
  ["t", "10m"],
];

let lastTick = -1;
for (const [key, name] of views) {
  await page.keyboard.press(key);
  await page.waitForTimeout(900);
  const hud = (await page.textContent("#hud")) ?? "";
  console.log(`${name.padEnd(6)} ${hud.replace(/\n/g, " | ")}`);
  await page.screenshot({ path: `${OUT}/${name}.png` });

  const tick = Number(/tick (\d+)/.exec(hud)?.[1] ?? -1);
  if (tick <= lastTick) {
    errors.push(`シミュレーションが進んでいない (${name}: tick ${tick})`);
  }
  lastTick = tick;
}

await browser.close();

if (errors.length) {
  console.error("\nNG:\n" + errors.join("\n"));
  process.exit(1);
}
console.log("\nOK: 全ズーム域でエラーなし");
