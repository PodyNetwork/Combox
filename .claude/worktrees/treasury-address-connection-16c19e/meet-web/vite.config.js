import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// /api is proxied to the Combox API, so one port serves the jobs board, the
// meeting page, and their API in previews (VITE_AXOM_API_URL=/api).
const apiTarget = process.env.AXOM_API_PROXY_TARGET || "http://127.0.0.1:8080";
const proxy = { "/api": { target: apiTarget, changeOrigin: true, rewrite: (p) => p.replace(/^\/api/, "") } };

// The interview room chunk is LiveKit itself (~160 kB gzipped); it loads in
// the background while the lobby shows.
export default defineConfig({ plugins: [react()], server: { proxy }, preview: { proxy }, build: { chunkSizeWarningLimit: 700 } });
