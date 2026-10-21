import { svelte } from "@sveltejs/vite-plugin-svelte";
import { readFileSync } from "node:fs";
import { defineConfig } from "vitest/config";

// About shows the app's version (section 11), read from the Tauri config at build time.
const tauriConf = JSON.parse(readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8")) as {
  version: string;
};

export default defineConfig({
  plugins: [svelte()],
  define: {
    __APP_VERSION__: JSON.stringify(tauriConf.version),
  },
  clearScreen: false,
  server: {
    host: "localhost",
    port: 5173,
    strictPort: true,
  },
  build: {
    // OS webviews: WebView2 (Chromium), WKWebView (Safari 16+), WebKitGTK 4.1.
    target: ["es2022", "chrome120", "safari16"],
    // Never inline assets as data: URIs; the CSP allows only 'self' (section 13.3).
    assetsInlineLimit: 0,
    sourcemap: false,
  },
  test: {
    include: ["src/**/*.test.ts", "scripts/**/*.test.ts"],
    environment: "node",
  },
});
