/**
 * 単一 HTML（`dist/battle-sim.html`）の疎通確認。
 *
 * **`file://` で開く。** これが本題で、サーバー越しに動くことは
 * `npm run smoke` がすでに見ている。単一 HTML の価値は「配って、
 * ダブルクリックで動く」ことなので、そこが壊れていないかを直接見る。
 *
 * `file://` のオリジンは `null` なので、module worker の生成も
 * `fetch()` も拒否される。Worker が classic で、wasm が data URI に
 * 畳まれていて初めて動く。どれか一つでも崩れるとここで落ちる。
 *
 *   npm run build:single
 *   npm run smoke:single
 */
import { chromium } from "playwright";
import { readFileSync, existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const webRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const htmlPath = process.env.SINGLE_FILE ?? join(webRoot, "dist", "battle-sim.html");
const EXEC = process.env.PLAYWRIGHT_CHROMIUM ?? undefined;

if (!existsSync(htmlPath)) {
  console.error(`${htmlPath} がありません。先に npm run build:single を実行してください`);
  process.exit(1);
}

const errors = [];

// 1. ファイルそのものが自己完結しているか（外部を取りに行く参照が無いか）
const html = readFileSync(htmlPath, "utf8");
const external = [...html.matchAll(/(?:src|href)\s*=\s*"([^"]+)"/g)]
  .map((m) => m[1])
  .filter((u) => !u.startsWith("data:"));
if (external.length > 0) {
  errors.push(`外部参照が残っている: ${external.join(", ")}`);
}
const sizeKb = Buffer.byteLength(html) / 1024;

const browser = await chromium.launch(EXEC ? { executablePath: EXEC } : {});
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });

page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
page.on("console", (m) => {
  if (m.type() !== "error") return;
  const text = m.text();
  // 画像アセット（PNG）は単一 HTML には入らない。読み込み失敗は想定内で、
  // 描画は手続き生成のフォールバックへ落ちる。それ以外は本物の異常。
  const isMissingAsset =
    /assets\/.*\.(png|json)/.test(text) ||
    text.includes("net::ERR_FILE_NOT_FOUND") ||
    text.includes("net::ERR_FAILED") ||
    text.includes("has been blocked by CORS policy");
  if (!isMissingAsset) errors.push(`console: ${text}`);
});

const hud = async () => (await page.textContent("#hud")) ?? "";
const tick = async () => Number(/tick (\d+)/.exec(await hud())?.[1] ?? -1);

await page.goto("file://" + resolve(htmlPath));

/** 失敗を素の例外ではなく、読める 1 行として溜める。 */
const step = async (what, fn) => {
  if (errors.length > 0) return; // 起動に失敗しているなら後続は意味がない
  try {
    await fn();
  } catch (e) {
    errors.push(`${what}: ${String(e).split("\n")[0]}`);
  }
};

// 2. wasm が起動し、Worker が地形を生成し、兵が並ぶ
await step("兵士が配置されない（wasm か Worker が起動していない）", () =>
  page.waitForFunction(
    () => /兵士 [1-9]/.test(document.getElementById("hud")?.textContent ?? ""),
    undefined,
    { timeout: 60_000 },
  ),
);

// 3. シミュレーションが進む
await step("シミュレーションが進まない", async () => {
  await page.keyboard.press("3"); // 4x
  const before = await tick();
  await page.waitForTimeout(6000);
  const after = await tick();
  if (!(after > before)) throw new Error(`tick ${before} → ${after}`);
});

// 4. 会戦プリセットへ組み替えられる（Worker との往復と地形生成が通る）
await step("会戦プリセットへ組み替えられない", async () => {
  const since = await page.evaluate(() => Number(document.body.dataset.worldSeq ?? "0"));
  await page.selectOption("#scenario-panel select", "0");
  await page.waitForFunction(
    (seq) =>
      Number(document.body.dataset.scenario ?? "-2") === 0 &&
      Number(document.body.dataset.worldSeq ?? "0") > seq,
    since,
    { timeout: 60_000 },
  );
  // HUD の兵数が新しいワールドのものになるのは次の描画フレームから
  await page.waitForFunction(
    () => /兵士 [1-9]/.test(document.getElementById("hud")?.textContent ?? ""),
    undefined,
    { timeout: 60_000 },
  );
});

console.log(`${htmlPath} (${sizeKb.toFixed(0)} KB)`);
if (errors.length === 0) console.log("HUD:", (await hud()).replace(/\n/g, " | "));

await browser.close();

if (errors.length) {
  console.error("\nNG:\n" + errors.join("\n"));
  process.exit(1);
}
console.log("\nOK: 単一 HTML が file:// で動いている");
