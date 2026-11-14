// On the multi-tenant platform every company's pages live under
// /t/<company>/ (jobs board, interview links), and its API is under the
// same prefix. A single-company deployment has no prefix.
const ROOT = import.meta.env.VITE_AXOM_API_URL || "http://localhost:8080";
const m = window.location.pathname.match(/^\/t\/([a-z0-9-]{3,40})(\/.*)?$/);

export const COMPANY = m ? m[1] : null;
/** Path inside the company's site, e.g. "/jobs/123". */
export const PATH = m ? m[2] || "/" : window.location.pathname;
/** Prefix for links within the site. */
export const PREFIX = COMPANY ? `/t/${COMPANY}` : "";
/** The company's API base URL. */
export const API = `${ROOT.replace(/\/$/, "")}${PREFIX}`;
