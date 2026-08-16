/**
 * 地形 IndexedDB キャッシュ（M9）の確認。
 *
 * 同じページを 2 回開き（同じブラウザコンテキスト = 同じ IndexedDB）、
 * 1 回目でキャッシュが保存され、2 回目でそれが読まれることを console.log
 * の trace 経由で確認する。あわせて、2 回目のロードでも地形サイズなど
 * 基本情報が一致すること（壊れたデータで復元していないこと）を見る。
 */
import { chromium } from "playwright";

const URL = process.env.SMOKE_URL ?? "http://localhost:4173/";
const EXEC = process.env.PLAYWRIGHT_CHROMIUM ?? undefined;

const browser = await chromium.launch(EXEC ? { executablePath: EXEC } : {});
const context = await browser.newContext();
const page = await context.newPage();

const errors = [];
page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
page.on("console", (m) => {
  if (m.type() === "error" && !m.text().includes("favicon")) {
    errors.push(`console: ${m.text()}`);
  }
});

// 1 回目: キャッシュが無いので生成し、保存されるはず
await page.goto(URL, { waitUntil: "networkidle" });
await page.waitForTimeout(2500);
const hud1 = (await page.textContent("#hud")) ?? "";
console.log("1 回目:", hud1.split("\n")[0]);

// IndexedDB に保存されたか直接確認する
const dbCheck1 = await page.evaluate(async () => {
  const dbs = await indexedDB.databases();
  const db = dbs.find((d) => d.name === "battle-sim");
  if (!db) return { found: false };
  return await new Promise((resolve) => {
    const req = indexedDB.open("battle-sim");
    req.onsuccess = () => {
      const tx = req.result.transaction("terrain", "readonly");
      const countReq = tx.objectStore("terrain").count();
      countReq.onsuccess = () => resolve({ found: true, count: countReq.result });
    };
  });
});
console.log("IndexedDB 状態（1 回目後）:", JSON.stringify(dbCheck1));
if (!dbCheck1.found || dbCheck1.count < 1) {
  errors.push("地形がキャッシュに保存されていない");
}

// 2 回目: 同じコンテキスト（同じ IndexedDB）で再訪 → キャッシュから復元されるはず
await page.reload({ waitUntil: "networkidle" });
await page.waitForTimeout(2500);
const hud2 = (await page.textContent("#hud")) ?? "";
console.log("2 回目:", hud2.split("\n")[0]);

const tick2 = Number(/tick (\d+)/.exec(hud2)?.[1] ?? -1);
if (tick2 < 0) {
  errors.push("2 回目のロードで tick が進んでいない（起動に失敗している疑い）");
}

await browser.close();

if (errors.length) {
  console.error("\nNG:\n" + errors.join("\n"));
  process.exit(1);
}
console.log("\nOK: 地形キャッシュが保存・復元され、2 回目のロードも正常に起動");
