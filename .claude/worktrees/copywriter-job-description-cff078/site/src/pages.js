// <body data-page="..."> → the page component in src/pages/. Shared by the
// browser entry (main.jsx) and the build-time prerender (entry-server.jsx).
export const PAGES = {
  home: "Home",
  how: "HowItWorks",
  security: "Security",
  pricing: "Pricing",
  start: "GetStarted",
  talent: "Talent",
  privacy: "Privacy",
  notfound: "NotFound",
};

/** Each page is its own chunk, so a visitor only downloads the page they open. */
export const loadPage = (key) => import(`./pages/${PAGES[key] ?? PAGES.notfound}.jsx`).then((m) => m.default);
