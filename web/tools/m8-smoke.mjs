/**
 * M8（UI と体験）の疎通確認。
 *
 * 憑依 → 命令発行（伝令の可視化）、兵士の選択 → 追従、観戦モード、
 * リプレイの保存・読み込み → hash 一致、をヘッドレスブラウザで一通り操作する。
 *
 *   npm run build && npm run preview &
 *   node tools/m8-smoke.mjs
 */
import { chromium } from "playwright";
import { mkdirSync } from "node:fs";

const URL = process.env.SMOKE_URL ?? "http://localhost:4173/";
const OUT = process.env.SMOKE_OUT ?? "smoke-out";
const EXEC = process.env.PLAYWRIGHT_CHROMIUM ?? undefined;

mkdirSync(OUT, { recursive: true });

const browser = await chromium.launch(EXEC ? { executablePath: EXEC } : {});
const page = await browser.newPage({ viewport: { width: 1400, height: 900 } });

const errors = [];
page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
page.on("console", (m) => {
  if (m.type() === "error" && !m.text().includes("favicon")) {
    errors.push(`console: ${m.text()}`);
  }
});

function fail(msg) {
  errors.push(msg);
}

await page.goto(URL, { waitUntil: "networkidle" });
await page.keyboard.press("2"); // 2x（伝令の移動をゆっくり観察するため）
await page.waitForTimeout(3000);

// ── 憑依して命令を出す ──────────────────────────────────
await page.click("#command-panel .node-row");
let hasOrderControls = await page.locator("#command-panel .order-controls").count();
if (hasOrderControls === 0) fail("憑依しても命令パネルが出ない");

await page.screenshot({ path: `${OUT}/possessed.png` });

// 「移動」を押して地図をクリックし、目標地点を指定する。
// #view-stack は子要素が全て position:absolute のため自身の内包サイズを
// 持たない（boundingBox() は使えない）。ビューポート座標で直接クリックする。
await page.getByRole("button", { name: "移動" }).click();
const vp = page.viewportSize();
const centerX = vp.width / 2;
const centerY = vp.height / 2;
await page.mouse.click(centerX - 260, centerY + 40);
await page.waitForTimeout(200);

const pendingLeft = await page.locator("#command-panel .pick-hint").count();
if (pendingLeft !== 0) fail("命令発行後も地点指定待ちのままになっている");

// 伝令が動く様子を可視化できているか（messengers があれば overlay に描かれる）
await page.waitForTimeout(1500);
await page.screenshot({ path: `${OUT}/order-issued.png` });

// ── 兵士を選択して追従する ──────────────────────────────
// 陣営の密集地点付近をクリックして最寄りの兵士を拾う
await page.mouse.click(centerX - 80, centerY - 30);
await page.waitForTimeout(300);
const soldierDetailShown = await page.locator("#detail-panel").textContent();
if (!soldierDetailShown || !soldierDetailShown.includes("兵士詳細")) {
  fail(`兵士クリックで詳細パネルが更新されない: ${soldierDetailShown}`);
} else {
  await page.getByRole("button", { name: "追従" }).click();
  await page.waitForTimeout(1500);
  const hud = (await page.textContent("#hud")) ?? "";
  if (!/視野 8\d m/.test(hud)) {
    fail(`追従カメラの視野幅が期待どおりでない: ${hud}`);
  }
  await page.screenshot({ path: `${OUT}/follow.png` });
  await page.getByRole("button", { name: "追従解除" }).click();
}

// ── 観戦モード ───────────────────────────────────────────
await page.getByRole("button", { name: /観戦モード/ }).click();
await page.waitForTimeout(4500);
await page.screenshot({ path: `${OUT}/spectator.png` });
await page.getByRole("button", { name: /観戦モード/ }).click();

// ── リプレイの保存・読み込み（hash 一致） ───────────────────
const [download] = await Promise.all([
  page.waitForEvent("download"),
  page.getByRole("button", { name: "記録を保存" }).click(),
]);
const replayPath = `${OUT}/replay.json`;
await download.saveAs(replayPath);

const [chooser] = await Promise.all([
  page.waitForEvent("filechooser"),
  page.getByRole("button", { name: "記録を読み込む" }).click(),
]);
await chooser.setFiles(replayPath);
await page.waitForTimeout(1500);

const statusText = await page.locator("#session-panel .status-line").textContent();
if (!statusText || !statusText.includes("一致") || statusText.includes("不一致")) {
  fail(`リプレイが一致しなかった: ${statusText}`);
}
await page.screenshot({ path: `${OUT}/replay-result.png` });

await browser.close();

if (errors.length) {
  console.error("\nNG:\n" + errors.join("\n"));
  process.exit(1);
}
console.log("\nOK: M8 の憑依・追従・観戦・リプレイが一通り動作");
