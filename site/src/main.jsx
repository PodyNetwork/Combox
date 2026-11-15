import { createRoot, hydrateRoot } from "react-dom/client";
import Layout from "./Layout.jsx";
import { takeJudgeToken } from "./lib/config.js";
import { PAGES, loadPage } from "./pages.js";
import "./styles.css";

// One entry for every page: <body data-page="..."> names the page to render.
const page = PAGES[document.body.dataset.page] ? document.body.dataset.page : "notfound";

takeJudgeToken();

// Every page, the home page included, stays readable when signed in; the
// header then offers "Open dashboard" instead of "Sign in".
loadPage(page).then((Page) => {
  const app = (
    <Layout page={page}>
      <Page />
    </Layout>
  );
  // `npm run build` prerenders every page (prerender.js); the dev server doesn't.
  const root = document.getElementById("root");
  if (root.firstElementChild) hydrateRoot(root, app);
  else createRoot(root).render(app);
});
