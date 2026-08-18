#!/usr/bin/env node
/**
 * 地形効果テーブルが JS と Rust で一致しているか検査する。
 *
 * 地形の生成は JS（`web/src/terrain/effects.ts`）、シミュレーションは Rust
 * （`crates/sim-terrain/src/effects.rs`）で、同じ数字を別々に持っている。
 * 食い違うと、生成時に道路が選んだ経路とシミュレーション中の移動速度が
 * 噛み合わなくなり、「地形は見た目どおりなのに兵が動かない」という形でしか
 * 現れない。数字の並びを両方から取り出して突き合わせる。
 *
 *   node tools/check_terrain_effects.mjs
 */

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const tsPath = join(repoRoot, "web", "src", "terrain", "effects.ts");
const rsPath = join(repoRoot, "crates", "sim-terrain", "src", "effects.rs");

/** `eff(a, b, c, d, e, f)` の呼び出しを、現れる順に 6 つ組で拾う。 */
function extractEffCalls(source, startMarker, endMarker) {
  const start = source.indexOf(startMarker);
  if (start < 0) throw new Error(`目印が見つからない: ${startMarker}`);
  const end = endMarker ? source.indexOf(endMarker, start) : source.length;
  const body = source.slice(start, end < 0 ? source.length : end);
  const rows = [];
  const re = /\beff\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*\)/g;
  let m;
  while ((m = re.exec(body)) !== null) rows.push(m.slice(1, 7).map(Number));
  return rows;
}

const ts = readFileSync(tsPath, "utf8");
const rs = readFileSync(rsPath, "utf8");

/** 突き合わせる区間。開始の目印から次の目印までを 1 つの表として読む。 */
const tables = [
  {
    name: "GROUND_EFFECTS",
    ts: ["export const GROUND_EFFECTS", "export const VEGETATION_EFFECTS"],
    rs: ["pub static GROUND_EFFECTS", "pub static VEGETATION_EFFECTS"],
  },
  {
    name: "VEGETATION_EFFECTS",
    ts: ["export const VEGETATION_EFFECTS", "export const OVERLAY_EFFECTS"],
    rs: ["pub static VEGETATION_EFFECTS", "pub static OVERLAY_EFFECTS"],
  },
  {
    name: "OVERLAY_EFFECTS",
    ts: ["export const OVERLAY_EFFECTS", "export function waterEffect"],
    rs: ["pub static OVERLAY_EFFECTS", "pub fn water_effect"],
  },
  {
    name: "waterEffect",
    ts: ["export function waterEffect", "function mul("],
    rs: ["pub fn water_effect", "fn mul3("],
  },
];

let failed = 0;
for (const table of tables) {
  const a = extractEffCalls(ts, table.ts[0], table.ts[1]);
  const b = extractEffCalls(rs, table.rs[0], table.rs[1]);
  if (a.length === 0) {
    console.error(`${table.name}: JS 側から 1 行も読めなかった`);
    failed++;
    continue;
  }
  if (a.length !== b.length) {
    console.error(`${table.name}: 行数が違う（JS ${a.length} / Rust ${b.length}）`);
    failed++;
    continue;
  }
  let mismatched = 0;
  for (let i = 0; i < a.length; i++) {
    if (a[i].join(",") !== b[i].join(",")) {
      console.error(
        `${table.name}[${i}]: JS [${a[i].join(", ")}] / Rust [${b[i].join(", ")}]`,
      );
      mismatched++;
    }
  }
  failed += mismatched;
  if (mismatched === 0) console.log(`OK ${table.name}: ${a.length} 行が一致`);
}

if (failed > 0) {
  console.error(
    "\n地形効果テーブルが JS と Rust で食い違っています。" +
      "\nweb/src/terrain/effects.ts と crates/sim-terrain/src/effects.rs の両方を直してください。",
  );
  process.exit(1);
}
console.log("OK: 地形効果テーブルは JS と Rust で一致しています");
