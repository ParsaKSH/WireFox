import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The bundle is embedded into the Go binary, so it is written straight into
// the embed directory. Relative asset paths keep it working behind a prefix.
export default defineConfig({
  plugins: [react()],
  base: "./",
  build: {
    outDir: "../internal/web/dist",
    emptyOutDir: true,
  },
  server: {
    proxy: {
      "/api": "http://127.0.0.1:8080",
    },
  },
});
