import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [svelte()],
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
