/**
 * 会戦プリセットの疎通確認。
 *
 * UI からプリセットを選ぶと、地形（森・泥濘）も陣容も指揮官も入れ替わり、
 * 命令を一切与えていないのに指揮官 AI の判断で会戦が始まる——という経路を
 * ブラウザ越しに確認する。スクリーンショットは web/smoke-out/ に出す。
 *
 *   npm run build && npx vite preview --port 4173 &
 *   node tools/scenario-smoke.mjs
 */
import { chromium } from "playwright";
import { mkdirSync } from "node:fs";

const URL = process.env.SMOKE_URL ?? "http://localhost:4173/";
const OUT = process.env.SMOKE_OUT ?? "smoke-out";
const EXEC = process.env.PLAYWRIGHT_CHROMIUM ?? undefined;
const SCENARIO = process.env.SMOKE_SCENARIO ?? "agincourt_1415";

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

const hudTick = async () => {
  const hud = (await page.textContent("#hud")) ?? "";
  return { hud, tick: Number(/tick (\d+)/.exec(hud)?.[1] ?? -1) };
};

// 1. URL からプリセットを指定して起動できる
await page.goto(`${URL}?scenario=${SCENARIO}`, { waitUntil: "networkidle" });
await page.waitForFunction(
  () => /兵士 [1-9]/.test(document.getElementById("hud")?.textContent ?? ""),
  { timeout: 60_000 },
);

const panel = (await page.textContent("#scenario-panel")) ?? "";
console.log(panel.replace(/\s+/g, " ").slice(0, 300));
for (const expected of ["アジンクール", "イングランド軍", "フランス軍"]) {
  if (!panel.includes(expected)) errors.push(`会戦パネルに「${expected}」が無い`);
}

// 2. 選択肢に会戦プリセットが並んでいる
const options = await page.$$eval("#scenario-panel select option", (els) =>
  els.map((e) => e.textContent),
);
console.log("選択肢:", options.join(" / "));
if (options.length < 2) errors.push("会戦プリセットが選択肢に出ていない");

// 3. 命令を出していないのに会戦が進む（指揮官 AI が動かしている）
await page.keyboard.press("4"); // 8x
const before = await hudTick();
console.log("開始:", before.hud.replace(/\n/g, " | "));
await page.waitForTimeout(15_000);
const after = await hudTick();
console.log("15 秒後:", after.hud.replace(/\n/g, " | "));
if (after.tick <= before.tick) errors.push("シミュレーションが進んでいない");

await page.screenshot({ path: `${OUT}/scenario-${SCENARIO}.png` });

// 4. 対称デモ配置へ戻せる（ワールドを組み直しても壊れない）
await page.selectOption("#scenario-panel select", "-1");
await page.waitForFunction(
  () => /tick [0-9]/.test(document.getElementById("hud")?.textContent ?? ""),
  { timeout: 60_000 },
);
await page.waitForTimeout(6000);
const sandbox = await hudTick();
console.log("デモへ復帰:", sandbox.hud.replace(/\n/g, " | "));
if (sandbox.tick < 0) errors.push("デモ配置へ戻せない");
await page.screenshot({ path: `${OUT}/scenario-sandbox.png` });

await browser.close();

if (errors.length) {
  console.error("\nNG:\n" + errors.join("\n"));
  process.exit(1);
}
console.log("\nOK: 会戦プリセットの選択と再構築が通っている");
