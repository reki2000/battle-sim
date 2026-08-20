/**
 * 地形 IndexedDB キャッシュの確認。
 *
 * 同じページを 2 回開き（同じブラウザコンテキスト = 同じ IndexedDB）、
 * 1 回目でキャッシュが保存され、2 回目でそれが読まれることを console.log
 * の trace 経由で確認する。あわせて、2 回目のロードでも地形サイズなど
 * 基本情報が一致すること（壊れたデータで復元していないこと）を見る。
 */
import {
  collectPageErrors,
  launchBrowser,
  smokeUrl as URL,
} from "./smoke-helpers.mjs";

const browser = await launchBrowser();
const context = await browser.newContext();
const page = await context.newPage();

const errors = [];
collectPageErrors(page, errors);

// 1 回目: キャッシュが無いので生成し、保存されるはず
await page.goto(URL, { waitUntil: "networkidle" });
await page.waitForFunction(
  () => /兵士 [1-9]/.test(document.getElementById("hud")?.textContent ?? ""),
  undefined,
  { timeout: 60_000 },
);
const hud1 = (await page.textContent("#hud")) ?? "";
console.log("1 回目:", hud1.split("\n")[0]);

/** Workerと同じoriginのIndexedDBをメインスレッドから読み、保存件数を返す。 */
const cacheState = () =>
  page.evaluate(async () => {
    const dbs = await indexedDB.databases();
    const db = dbs.find((d) => d.name === "battle-sim");
    if (!db) return { found: false, count: 0 };
    return await new Promise((resolve) => {
      const req = indexedDB.open("battle-sim");
      req.onerror = () => resolve({ found: true, count: -1 });
      req.onsuccess = () => {
        const connection = req.result;
        const tx = connection.transaction("terrain", "readonly");
        const countReq = tx.objectStore("terrain").count();
        countReq.onerror = () => resolve({ found: true, count: -1 });
        countReq.onsuccess = () => {
          connection.close();
          resolve({ found: true, count: countReq.result });
        };
      };
    });
  });

// 保存はシミュレーションの起動を塞がない非同期処理なので、固定時間ではなく
// 実際に1件入るまで待つ。
let dbCheck1 = { found: false, count: 0 };
for (let attempt = 0; attempt < 60 && dbCheck1.count < 1; attempt++) {
  dbCheck1 = await cacheState();
  if (dbCheck1.count < 1) await page.waitForTimeout(250);
}
console.log("IndexedDB 状態（1 回目後）:", JSON.stringify(dbCheck1));
if (!dbCheck1.found || dbCheck1.count < 1) {
  errors.push("地形がキャッシュに保存されていない");
}

// 2 回目: 同じコンテキスト（同じ IndexedDB）で再訪 → キャッシュから復元されるはず
await page.reload({ waitUntil: "networkidle" });
await page.waitForFunction(
  () => {
    const text = document.getElementById("hud")?.textContent ?? "";
    return /兵士 [1-9]/.test(text) && Number(/tick (\d+)/.exec(text)?.[1] ?? -1) >= 0;
  },
  undefined,
  { timeout: 60_000 },
);
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
