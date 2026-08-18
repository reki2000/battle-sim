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
/**
 * いま画面に載っているワールドの素性（`main.ts` が `ready` のたびに書く）。
 * `scenario` は選択中のプリセット index、`seq` は組み直しの通し番号。
 */
const worldState = () =>
  page.evaluate(() => ({
    scenario: Number(document.body.dataset.scenario ?? "-2"),
    seq: Number(document.body.dataset.worldSeq ?? "0"),
  }));

/**
 * 指定したプリセットのワールドが載り終わるまで待ち、兵数を返す。
 *
 * HUD の見かけから推測してはいけない。切り替えた直後はまだ前のワールドが
 * 動いていて、前の会戦の兵数と進んだ tick を読んでしまう——しかも地形の
 * 生成がワーカーへ移ってから組み直しに 1 秒近くかかることがあり、その間
 * ずっと前のワールドの HUD が「落ち着いて」見える。`ready` のたびに更新
 * される `data-scenario` / `data-world-seq` を直接見て、目当てのワールドが
 * 載ったことを確かめる。
 *
 * `?scenario=` 起動では、URL による選択とループ先頭の再選択で組み直しが
 * 2 回続けて走ることがあるので、`seq` が進み終わるのも待つ。
 */
const waitForWorld = async (scenarioIndex, sinceSeq) => {
  const matches = ([want, since]) => {
    const d = document.body.dataset;
    return (
      Number(d.scenario ?? "-2") === want && Number(d.worldSeq ?? "0") > since
    );
  };
  await page.waitForFunction(matches, [scenarioIndex, sinceSeq], { timeout: 60_000 });

  // 組み直しが連続することがあるので、`seq` が止まるまで見届ける
  let previous = -1;
  for (let i = 0; i < 20; i++) {
    await page.waitForTimeout(400);
    const { seq } = await worldState();
    if (seq === previous) break;
    previous = seq;
  }

  // 目当てのワールドが載っていることと兵数を**同じ評価の中で**読む。
  // `data-scenario` は `ready` を受けた時点で立つが、HUD の兵数が新しい
  // ワールドのものになるのは次の描画フレームからで、その隙間では
  // 「兵士 0」を通る。2 回に分けて読むと、その隙間を挟んでしまう。
  const handle = await page.waitForFunction(
    ([want, since]) => {
      const d = document.body.dataset;
      if (Number(d.scenario ?? "-2") !== want) return false;
      if (Number(d.worldSeq ?? "0") <= since) return false;
      const m = /兵士 (\d+)/.exec(document.getElementById("hud")?.textContent ?? "");
      const n = m ? Number(m[1]) : 0;
      return n > 0 ? n : false;
    },
    [scenarioIndex, sinceSeq],
    { timeout: 60_000 },
  );
  return await handle.jsonValue();
};

// 1. URL からプリセットを指定して起動できる
await page.goto(`${URL}?scenario=${FIRST}`, { waitUntil: "networkidle" });
// `?scenario=` は id 指定なので、どの index になるかは載ってから分かる。
// プリセット（index 0 以上）が載り、兵が出るまで待つ。
await page.waitForFunction(
  () =>
    Number(document.body.dataset.scenario ?? "-2") >= 0 &&
    /兵士 [1-9]/.test(document.getElementById("hud")?.textContent ?? ""),
  undefined,
  { timeout: 60_000 },
);
await waitForWorld((await worldState()).scenario, 0);

const options = await page.$$eval("#scenario-panel select option", (els) =>
  els.map((e) => ({ value: e.value, label: e.textContent ?? "" })),
);
console.log("選択肢:", options.map((o) => o.label).join(" / "));
const presets = options.filter((o) => Number(o.value) >= 0);
if (presets.length < 2) errors.push("会戦プリセットが 2 つ以上出ていない");

await page.keyboard.press("4"); // 8x

// 2. どのプリセットも、選ぶと陣容が入れ替わり、命令なしで会戦が進む
for (const preset of presets) {
  const { seq } = await worldState();
  await page.selectOption("#scenario-panel select", preset.value);
  const deployed = await waitForWorld(Number(preset.value), seq);

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
const { seq: seqBeforeSandbox } = await worldState();
await page.selectOption("#scenario-panel select", "-1");
await waitForWorld(-1, seqBeforeSandbox);
await page.waitForTimeout(4000);
console.log("デモへ復帰:", (await hud()).replace(/\n/g, " | "));
await page.screenshot({ path: `${OUT}/scenario-sandbox.png` });

await browser.close();

if (errors.length) {
  console.error("\nNG:\n" + errors.join("\n"));
  process.exit(1);
}
console.log("\nOK: すべての会戦プリセットの選択と再構築が通っている");
