// Build-time only: renders a page to HTML for prerender.js.
import { renderToString } from "react-dom/server";
import Layout from "./Layout.jsx";
import { loadPage } from "./pages.js";

export async function render(page) {
  const Page = await loadPage(page);
  return renderToString(
    <Layout page={page}>
      <Page />
    </Layout>,
  );
}
