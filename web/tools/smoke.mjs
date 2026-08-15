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
import { chromium } from "playwright";
import { mkdirSync } from "node:fs";

const URL = process.env.SMOKE_URL ?? "http://localhost:4173/";
const OUT = process.env.SMOKE_OUT ?? "smoke-out";
// CI（Playwright が自前で入れる）とサンドボックス（事前配置）の両方に対応する
const EXEC = process.env.PLAYWRIGHT_CHROMIUM ?? undefined;

mkdirSync(OUT, { recursive: true });

const browser = await chromium.launch(EXEC ? { executablePath: EXEC } : {});
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });

const errors = [];
page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
page.on("console", (m) => {
  if (m.type() === "error" && !m.text().includes("favicon")) {
    errors.push(`console: ${m.text()}`);
  }
});

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
