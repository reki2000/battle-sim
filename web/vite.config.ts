import { defineConfig } from "vite";

// ビルドごとに変わる短い識別子。単体 HTML は配布のたびに送り直すファイル名が
// 同じになりがちで、「これは最新のファイルか」を見た目だけでは判別できない
// （実際に配布先で「バージョン番号がないのでわからない」という声があった）。
// HUD に出す（main.ts）ための、ビルド時刻ベースの値をここで注入する。
const BUILD_ID = new Date()
  .toISOString()
  .replace("T", " ")
  .replace(/:\d{2}\.\d{3}Z$/, "");

export default defineConfig({
  define: {
    __BUILD_ID__: JSON.stringify(BUILD_ID),
  },
  server: {
    // SharedArrayBuffer を使う場合（仕様 01 章 3.2 / 11 章 4 節）は
    // これらのヘッダが必要になる。M9 のマルチスレッド化で有効にする。
    headers: {
      "Cross-Origin-Opener-Policy": "same-origin",
      "Cross-Origin-Embedder-Policy": "require-corp",
    },
  },
  build: {
    target: "es2022",
    // wasm も含めてすべて JS へ畳み込む。`?worker&inline` の Worker は
    // blob から動くので `import.meta.url` が blob: になり、そこからの
    // 相対パスで `sim_bg.wasm` を取りに行けない。Worker を畳むなら
    // wasm も畳むしかない、という組み合わせで決まっている。
    //
    // 代償は base64 のぶん（約 4/3）JS が太ることだが、これで
    // 出力が HTML と JS の 2 本になり、`tools/bundle-single-file.mjs` が
    // 1 枚の HTML にできる。
    assetsInlineLimit: 100_000_000,
  },
  worker: {
    // **classic（IIFE）でなければならない。** `file://` のオリジンは `null` で、
    // そこから module worker を作ることはブラウザに拒否される（classic は
    // 通る）。ここを "es" に戻すと、サーバー越しでは動くのに単一 HTML を
    // ファイルとして開いたときだけ無言で止まる。
    // `web/tools/single-file-smoke.mjs` が file:// で実際に起動して見張る。
    format: "iife",
  },
});
