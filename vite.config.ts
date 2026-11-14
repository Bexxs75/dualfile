/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  // Relative Pfade: die Oberfläche lädt später aus dem Tauri-Paket, nie aus dem Netz.
  base: "./",
  test: {
    environment: "jsdom",
    css: { include: /(?:app|ssh)\.css/ },
    globals: true,
    setupFiles: ["./src/test-setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
