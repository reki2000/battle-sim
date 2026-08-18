#!/usr/bin/env node
/**
 * シナリオの地形を生成し、`data/terrain/*.bin` へ固定する。
 *
 * 生成器（`web/src/terrain`）は f64 で動くので、走らせる環境によって
 * 1 cm ずれることがありうる。Rust のテストと `sim-headless` が毎回同じ
 * 地形を見るように、生成は**ここで 1 度だけ**行い、結果の整数グリッドを
 * リポジトリに入れる。生成器を直したらこれを回して差分をコミットすること。
 *
 *   node tools/gen_terrain_fixtures.mjs [--check]
 *
 * `--check` は書き出さず、既存のフィクスチャと一致するかだけを見る（CI 用）。
 */

import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { pathToFileURL, fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..");
const outDir = join(repoRoot, "data", "terrain");

// 生成器は web と同じ TypeScript のソースをそのまま使う。Node は拡張子なしの
// import を解決できないので、esbuild で 1 ファイルへ束ねてから読み込む。
// 別実装を Node 用に持つと、ブラウザで見える地形とフィクスチャがずれる。
const bundleDir = mkdtempSync(join(tmpdir(), "terrain-bundle-"));
const bundlePath = join(bundleDir, "terrain.mjs");
execFileSync(
  join(repoRoot, "web", "node_modules", ".bin", "esbuild"),
  [
    join(repoRoot, "web", "src", "terrain", "index.ts"),
    "--bundle",
    "--format=esm",
    "--platform=node",
    "--target=node20",
    `--outfile=${bundlePath}`,
  ],
  { stdio: ["ignore", "ignore", "inherit"] },
);

const terrainModule = await import(pathToFileURL(bundlePath).href);
const { SCENARIO_TERRAINS, generateScenarioTerrain, encodeTerrain } = terrainModule;

const checkOnly = process.argv.includes("--check");
mkdirSync(outDir, { recursive: true });

let failed = 0;
for (const scenario of SCENARIO_TERRAINS) {
  const started = Date.now();
  const terrain = generateScenarioTerrain(scenario);
  const bytes = Buffer.from(encodeTerrain(terrain));
  const path = join(outDir, `${scenario.id}.bin`);
  const elapsed = Date.now() - started;

  if (checkOnly) {
    if (!existsSync(path)) {
      console.error(`欠けている: ${path}`);
      failed++;
      continue;
    }
    const existing = readFileSync(path);
    if (!existing.equals(bytes)) {
      console.error(`一致しない: ${path}（生成器を直したら再生成してコミットすること）`);
      failed++;
      continue;
    }
    console.log(`OK ${scenario.id} (${bytes.length} bytes, ${elapsed} ms)`);
  } else {
    writeFileSync(path, bytes);
    console.log(
      `書き出し ${scenario.id}: ${terrain.dim}x${terrain.dim} cells, ` +
        `${bytes.length} bytes, ${terrain.battleSites.length} sites, ${elapsed} ms`,
    );
  }
}

rmSync(bundleDir, { recursive: true, force: true });
if (failed > 0) process.exit(1);
