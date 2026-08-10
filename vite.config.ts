import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { configDefaults } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  build: {
    minify: false,
  },
  server: {
    port: 1420,
    strictPort: true,
    hmr: {
      port: 1421
    }
  },
  clearScreen: false,
  test: {
    exclude: [
      ...configDefaults.exclude,
      ".agents/**",
      ".codex/**",
      "mobile/**",
      "relay/**"
    ]
  }
});
