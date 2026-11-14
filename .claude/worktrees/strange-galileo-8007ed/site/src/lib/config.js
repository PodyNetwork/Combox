// Where the site's buttons lead. Set these at build time for a real
// deployment, e.g. VITE_DASHBOARD_URL=https://app.example.com npm run build.
const strip = (u) => u.replace(/\/$/, "");

/** The admin dashboard: sign-up, sign-in, approvals (platform sign-up lives here too). */
export const DASHBOARD_URL = strip(import.meta.env.VITE_DASHBOARD_URL || "http://localhost:5174");
/** Every dashboard link on the site. In judge mode Layout adds the sign-in token on click. */
export const DASHBOARD_HOME = `${DASHBOARD_URL}/`;
/** The public jobs board (Combox Jobs) on the participant site. */
export const JOBS_URL = strip(import.meta.env.VITE_JOBS_URL || "http://localhost:5173/jobs");
/** The freelancer portal on the participant site. */
export const BRIEFS_URL = strip(import.meta.env.VITE_BRIEFS_URL || "http://localhost:5173/briefs");
/** Source code and docs. */
export const REPO_URL = strip(import.meta.env.VITE_REPO_URL || "https://github.com/PodyNetwork/Combox");

/** Built for ./scripts/start demo (judge mode): the dashboard signs in with a local token. */
export const JUDGE_MODE = import.meta.env.VITE_JUDGE_MODE === "true";

/** The dashboard sets this cookie while someone is signed in (no secret in it). False while prerendering. */
export const SIGNED_IN = typeof document !== "undefined" && document.cookie.split("; ").includes("axom_signed_in=1");

const JUDGE_KEY = "axom-judge-token";

/**
 * ./scripts/start demo opens the website with the judge session token in the URL fragment
 * (never sent to a server). Keep it for this tab so "Sign in" lands in the
 * dashboard already signed in, and drop it from the address bar.
 */
export function takeJudgeToken() {
  if (!JUDGE_MODE) return;
  const m = window.location.hash.match(/^#judge-token=([\w.-]+)$/);
  if (!m) return;
  try {
    sessionStorage.setItem(JUDGE_KEY, m[1]);
  } catch {
    /* storage blocked: the dashboard asks for the token instead */
  }
  history.replaceState(null, "", location.pathname + location.search);
}

/** Link into the dashboard (signed in automatically in judge mode). */
export function dashboardHref() {
  if (JUDGE_MODE) {
    try {
      const t = sessionStorage.getItem(JUDGE_KEY);
      if (t) return `${DASHBOARD_URL}/#judge-token=${t}`;
    } catch {
      /* fall through */
    }
  }
  return DASHBOARD_HOME;
}

/** Platform plan defaults (AXOM_PLAN_PRICE_USD, AXOM_TRIAL_DAYS). */
export const PLAN_PRICE_USD = Number(import.meta.env.VITE_PLAN_PRICE_USD || 49);
export const TRIAL_DAYS = Number(import.meta.env.VITE_TRIAL_DAYS || 14);

/** Platform policy defaults (config/policy.toml, config/performance.toml). */
export const POLICY = {
  version: "2026-11-08.1",
  thresholdUsd: 50,
  windowHours: 24,
  capUsd: 500,
  priceMaxAgeSecs: 900,
  approvalExpiryDays: 7,
  retentionDays: 180,
};

export const FIRING = {
  minCompletedTasks: 10,
  minDataDays: 14,
  consecutiveBelowPeriods: 2,
  reviewEveryTasks: 10,
  periodDays: 7,
  minCompletionRate: 0.7,
  minOnTimeRate: 0.6,
  maxCostPerCompletedUsd: 5,
  minObjectiveQuality: 0.6,
};

export const NAV = [
  { href: "/how-it-works/", label: "How it works", page: "how" },
  { href: "/security/", label: "Security", page: "security" },
  { href: "/talent/", label: "For talent", page: "talent" },
  { href: "/pricing/", label: "Pricing", page: "pricing" },
  { href: "/get-started/", label: "Get started", page: "start" },
];
