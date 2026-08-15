import { defineConfig } from "vite";

export default defineConfig({
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
    // wasm はそのまま assets として出す
    assetsInlineLimit: 0,
  },
  worker: {
    format: "es",
  },
});
