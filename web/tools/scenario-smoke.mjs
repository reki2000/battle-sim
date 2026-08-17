/**
 * 会戦プリセットの疎通確認。
 *
 * UI からプリセットを選ぶと、地形（森・泥濘・湿地）も陣容も指揮官も入れ替わり、
 * 命令を一切与えていないのに指揮官 AI の判断で会戦が始まる——という経路を
 * ブラウザ越しに確認する。登録されているプリセットを**すべて**順に選び、
 * 最後に対称デモ配置へ戻す。スクリーンショットは web/smoke-out/ に出す。
 *
 *   npm run build && npx vite preview --port 4173 &
 *   node tools/scenario-smoke.mjs
 */
import { chromium } from "playwright";
import { mkdirSync } from "node:fs";

const URL = process.env.SMOKE_URL ?? "http://localhost:4173/";
const OUT = process.env.SMOKE_OUT ?? "smoke-out";
const EXEC = process.env.PLAYWRIGHT_CHROMIUM ?? undefined;
/** 起動時に URL から指定するプリセット（`?scenario=` の経路も確認する）。 */
const FIRST = process.env.SMOKE_SCENARIO ?? "agincourt_1415";

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

const hud = async () => (await page.textContent("#hud")) ?? "";
const tick = async () => Number(/tick (\d+)/.exec(await hud())?.[1] ?? -1);
const soldiers = async () => Number(/兵士 (\d+)/.exec(await hud())?.[1] ?? -1);
const waitForDeploy = () =>
  page.waitForFunction(
    () => /兵士 [1-9]/.test(document.getElementById("hud")?.textContent ?? ""),
    { timeout: 60_000 },
  );

// 1. URL からプリセットを指定して起動できる
await page.goto(`${URL}?scenario=${FIRST}`, { waitUntil: "networkidle" });
await waitForDeploy();

const options = await page.$$eval("#scenario-panel select option", (els) =>
  els.map((e) => ({ value: e.value, label: e.textContent ?? "" })),
);
console.log("選択肢:", options.map((o) => o.label).join(" / "));
const presets = options.filter((o) => Number(o.value) >= 0);
if (presets.length < 2) errors.push("会戦プリセットが 2 つ以上出ていない");

await page.keyboard.press("4"); // 8x

// 2. どのプリセットも、選ぶと陣容が入れ替わり、命令なしで会戦が進む
for (const preset of presets) {
  await page.selectOption("#scenario-panel select", preset.value);
  await waitForDeploy();
  const deployed = await soldiers();

  const panel = ((await page.textContent("#scenario-panel")) ?? "").replace(/\s+/g, " ");
  const armies = (panel.match(/■/g) ?? []).length;
  if (armies < 2) errors.push(`${preset.label}: 陣容パネルに軍が 2 つ出ていない`);
  if (!panel.includes("会戦プラン")) errors.push(`${preset.label}: 会戦プランが出ていない`);

  const before = await tick();
  await page.waitForTimeout(10_000);
  const after = await tick();
  console.log(
    `${preset.label.padEnd(22)} 兵 ${deployed} tick ${before} → ${after}`,
  );
  if (after <= before) errors.push(`${preset.label}: シミュレーションが進んでいない`);
  if (deployed < 100) errors.push(`${preset.label}: 兵が配置されていない (${deployed})`);

  await page.screenshot({ path: `${OUT}/scenario-${preset.value}.png` });
}

// 3. 対称デモ配置へ戻せる（ワールドを組み直しても壊れない）
await page.selectOption("#scenario-panel select", "-1");
await waitForDeploy();
await page.waitForTimeout(4000);
console.log("デモへ復帰:", (await hud()).replace(/\n/g, " | "));
await page.screenshot({ path: `${OUT}/scenario-sandbox.png` });

await browser.close();

if (errors.length) {
  console.error("\nNG:\n" + errors.join("\n"));
  process.exit(1);
}
console.log("\nOK: すべての会戦プリセットの選択と再構築が通っている");
