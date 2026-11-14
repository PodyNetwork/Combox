import { resolve } from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// A multi-page site: every page is its own HTML file (real URLs that work on
// any static host), sharing one React entry that renders the page named in
// <body data-page="...">. `npm run build` then prerenders each page (prerender.js).
const pages = ["index", "how-it-works/index", "security/index", "pricing/index", "get-started/index", "talent/index", "privacy/index", "404"];

export default defineConfig(({ isSsrBuild }) => ({
  plugins: [react()],
  build: isSsrBuild
    ? {}
    : {
        manifest: true, // read by prerender.js to preload each page's chunks
        rollupOptions: { input: Object.fromEntries(pages.map((p) => [p.replace("/index", "") || "index", resolve(import.meta.dirname, `${p}.html`)])) },
      },
}));
