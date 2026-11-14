import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// /api is proxied to the Combox API, so a single port serves the dashboard and
// its API in previews and judge mode (VITE_AXOM_API_URL=/api).
const apiTarget = process.env.AXOM_API_PROXY_TARGET || "http://127.0.0.1:8080";
const proxy = { "/api": { target: apiTarget, changeOrigin: true, rewrite: (p) => p.replace(/^\/api/, "") } };

export default defineConfig({ plugins: [react(), tailwindcss()], server: { proxy }, preview: { proxy } });
