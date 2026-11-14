// Runs after the client and server builds (see "build" in package.json).
// Writes each page's HTML into dist/ so it paints before any JavaScript runs,
// and preloads the page's own chunks so React hydrates without a waterfall.
import { readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { render } from "./dist-ssr/entry-server.js";
import { PAGES } from "./src/pages.js";

const manifest = JSON.parse(readFileSync("dist/.vite/manifest.json", "utf8"));
const chunks = (key, out = new Set()) => {
  const c = manifest[key];
  if (c && !out.has(c.file)) {
    out.add(c.file);
    for (const k of c.imports ?? []) chunks(k, out);
  }
  return out;
};

for (const file of readdirSync("dist", { recursive: true }).filter((f) => f.endsWith(".html"))) {
  const path = `dist/${file}`;
  const html = readFileSync(path, "utf8");
  const page = html.match(/data-page="(\w+)"/)?.[1];
  if (!PAGES[page]) throw new Error(`${path}: unknown data-page "${page}"`);
  const preload = [...chunks(`src/pages/${PAGES[page]}.jsx`)].map((f) => `<link rel="modulepreload" crossorigin href="/${f}">`).join("");
  writeFileSync(
    path,
    html
      .replace("</head>", `${preload}</head>`)
      .replace('<div id="root"></div>', `<div id="root">${await render(page)}</div>`),
  );
  console.log(`prerendered ${path}`);
}

rmSync("dist/.vite", { recursive: true });
rmSync("dist-ssr", { recursive: true });
