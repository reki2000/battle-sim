#!/usr/bin/env node
/**
 * `dist/` を 1 枚の HTML にまとめる。
 *
 *   npm run build && node tools/bundle-single-file.mjs
 *   → dist/battle-sim.html
 *
 * ビルド設定（`vite.config.ts`）の側で、Worker は classic（IIFE）の
 * blob として、wasm は data URI として、すでに JS へ畳み込んである。
 * ここに残る仕事は「その 1 本の JS を HTML へ入れる」ことだけ。
 *
 * **Worker が classic なのは file:// で開けるようにするため。** `file://` の
 * オリジンは `null` で、そこから module worker を作ることはブラウザに拒否
 * される（classic worker は通る）。ES module のまま畳むと、サーバー越しでは
 * 動くのにファイルを直接開くと無言で止まる、という形でしか現れない。
 */

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const webRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const dist = join(webRoot, "dist");
const outPath = join(dist, "battle-sim.html");

const html = readFileSync(join(dist, "index.html"), "utf8");

const scripts = [...html.matchAll(/<script\b[^>]*\bsrc="([^"]+)"[^>]*><\/script>/g)];
if (scripts.length === 0) {
  throw new Error("dist/index.html に外部 JS の script タグが見つかりません");
}

let out = html;
for (const [tag, src] of scripts) {
  const file = join(dist, src.replace(/^\//, ""));
  const code = readFileSync(file, "utf8");
  // `</script>` がコード中に現れると HTML パーサがそこで閉じてしまう。
  // JS としては同じ意味の文字列に割って逃がす。
  const safe = code.replace(/<\/script>/gi, "<\\/script>");
  out = out.replace(tag, `<script type="module">\n${safe}\n</script>`);
}

const leftovers = [...out.matchAll(/(?:src|href)="(\.?\/[^"]+)"/g)]
  .map((m) => m[1])
  .filter((u) => !u.startsWith("data:"));
if (leftovers.length > 0) {
  throw new Error(`外部参照が残っています: ${leftovers.join(", ")}`);
}

writeFileSync(outPath, out);

console.log(`書き出し ${outPath} (${(Buffer.byteLength(out) / 1024).toFixed(0)} KB)`);
console.log(`畳み込んだ JS: ${scripts.length} 本`);
// `dist/index.html` と `dist/assets/*.js` はそのまま残す。ホスティング用の
// 通常のビルド成果物であって、単一 HTML はその隣に増えるだけ。
