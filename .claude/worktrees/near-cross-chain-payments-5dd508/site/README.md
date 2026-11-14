# Combox marketing site

The public website for Combox, the operating system for fully autonomous,
AI-run companies: a landing page and the pages around it, in the same
control-room design as the dashboard and the participant site. All copy follows
one message: AI runs every area of the company, inside hard limits enforced by
code; the human owner acts as the board.

| Page | Path |
|---|---|
| Home (live control room, every department run by AI, planner, payout policy, AI interview, prompt-injection and firing demos) | `/` |
| How it works (the board, the AI workforce, recruiting, candidates, payroll, architecture, safe mode) | `/how-it-works/` |
| Security and hard limits (what the AI runs and can never touch, kill switch demo, key custody, isolation, compliance) | `/security/` |
| Pricing (plans, NEAR billing, estimate calculator) | `/pricing/` |
| Get started (demo, self-host checklist, platform sign-up, Coolify) | `/get-started/` |
| For freelancers and candidates | `/talent/` |
| Privacy and data handling | `/privacy/` |
| Not found | `/404.html` |

The demos run in the browser on small copies of Combox's real rules
(`src/lib/axom.js`): the payout checks mirror `axom-hr`'s policy engine and
cap, the org chart uses the keyword rules in `config/planner.toml`, and the
firing rules come from `config/performance.toml`. If those change, update
`src/lib/axom.js` and `src/lib/config.js` too.

## Signed-in visitors go to the dashboard

Like most products, the home page sends signed-in visitors straight to the
dashboard, and the menu shows "Open dashboard" instead of "Sign in". The
dashboard sets an `axom_signed_in=1` cookie while you're signed in (it holds
no secret) and clears it when you sign out. On localhost the website and the
dashboard share it automatically (cookies ignore ports). In production, build
the dashboard with `VITE_SESSION_COOKIE_DOMAIN=example.com` so
`app.example.com` and `example.com` share it.

`./scripts/setup` (Docker) serves it as the `site` service on :5175 and opens it
first (`infra/site.Dockerfile`). `./scripts/start demo` builds this site with `VITE_JUDGE_MODE=true`, serves it on :5175 and
opens it first. It passes the local sign-in token in the link's `#fragment`,
so "Sign in" lands in the dashboard already signed in.

## Run

```bash
npm install
npm run dev        # http://localhost:5175
```

## Build

```bash
VITE_DASHBOARD_URL=https://app.example.com \
VITE_JOBS_URL=https://jobs.example.com/jobs \
VITE_BRIEFS_URL=https://jobs.example.com/briefs \
npm run build      # static files in dist/, one HTML file per page
```

Other build settings: `VITE_REPO_URL`, `VITE_PLAN_PRICE_USD` (49),
`VITE_TRIAL_DAYS` (14). Without them, links point at the local dev ports.
`dist/` works on any static host; serve `404.html` for unknown paths.

The build prerenders every page (`prerender.js`): each HTML file already holds
the page's content, so it paints before any JavaScript runs, and React then
hydrates it. Code that reads the browser (cookies, `localStorage`, the URL
hash, the clock) must run in an effect, not while rendering, or the build
fails or the page hydrates with a mismatch. Fonts are self-hosted in
`public/fonts/` (latin subset). `infra/site.Dockerfile` serves `dist/`
gzipped, with the hashed `/assets/` files cached for a year.
