/** Playwright 疎通確認で共有する、起動と既定エラー収集だけの薄いヘルパー。 */
import { mkdirSync } from "node:fs";
import { chromium } from "playwright";

export const smokeUrl = process.env.SMOKE_URL ?? "http://localhost:4173/";
export const smokeOut = process.env.SMOKE_OUT ?? "smoke-out";

export function ensureSmokeOut() {
  mkdirSync(smokeOut, { recursive: true });
}

/** CI の Playwright と、事前配置した Chromium のどちらでも起動する。 */
export function launchBrowser() {
  const executablePath = process.env.PLAYWRIGHT_CHROMIUM;
  return chromium.launch(executablePath ? { executablePath } : {});
}

/** favicon 以外のページ例外・console error を共通形式で収集する。 */
export function collectPageErrors(page, errors) {
  page.on("pageerror", (error) => errors.push(`pageerror: ${error.message}`));
  page.on("console", (message) => {
    if (message.type() === "error" && !message.text().includes("favicon")) {
      errors.push(`console: ${message.text()}`);
    }
  });
}
