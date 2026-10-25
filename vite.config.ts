import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// In browser dev mode, API calls go to a locally running ccm-server.
const backend = process.env.CCM_BACKEND ?? "http://127.0.0.1:8787";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    proxy: {
      "/healthz": backend,
      "/admin/api": backend,
    },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    outDir: "dist",
    target: "es2021",
  },
});
