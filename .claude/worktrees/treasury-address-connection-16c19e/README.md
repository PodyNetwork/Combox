# Combox

**Run your company in a box.**
Combox (short for "company box") is the operating system for an AI-run company: from a single mission
it plans the strategy, builds departments, teams, and workers (AI agents and
human freelancers), runs engineering, marketing, sales and operations through
its agents, recruits, interviews and hires, reviews performance and fires
underperformers, and runs payroll in NEAR, with no manager in the loop. The
human owner acts as the board: they set the mission, fund the treasury and own
the hard limits. A deterministic policy engine sits in front of every AI
decision that moves money, hires, fires, sends a contract, or sends an
external message; no prompt can move it.

- Payouts under **USD 50** (summed per recipient over 24 hours) go out
  automatically; **USD 50 or more** escalate to the board: a named admin
  approves them in the dashboard, with TOTP MFA.
- All worker payouts share a **USD 500 cap per UTC month**. Approval cannot
  override it; only an MFA-verified admin can change it, and every change is
  audited.
- Workers can be **paid on any chain** NEAR Intents reaches (USDC on Base,
  Ethereum, Arbitrum, Solana; BTC; SOL; and more). The treasury stays on NEAR:
  the signer deposits wNEAR into a checked 1Click quote and solvers deliver.
  The worker names the destination by signing it with their verified NEAR key.
- Payouts are signed by a NEAR treasury key that **never leaves OpenBao**.
  The signer re-checks every payout against a signed authorization, the
  worker's verified address, the amount, the tax form, and the cap before it
  asks OpenBao for a signature.
- Each company can make these rules **stricter** for itself (Settings): a
  lower approval threshold, or a shorter retention for candidate data. They
  can never be loosened below the platform's.
- One **kill switch** halts all tool calls and payouts.
- Untrusted text (applicant messages, transcripts, tool output) is always
  data, never instructions.
- Runs for **one company**, or as a **platform** where anyone can sign up a
  company, connect their own tools, and pay a subscription in NEAR. Each
  company gets its own database and its own NEAR treasury.

Status, decisions, and every known gap are in [`DECISIONS.md`](DECISIONS.md).

## For judges

Built at NEAR HACKATHON (tracks: Autonomous Business, Agent Economy, AI
Security & Defense). Everything below runs on your machine; NEAR is real
testnet, other outside services are mocks.

1. Run `./scripts/start demo` (needs Rust, Node.js 20+, Python 3, Docker).
   See: the build, then the scenario printing each step, then the Combox
   website opening at http://localhost:5175. Its **Sign in** takes you into
   the dashboard, already signed in.
2. In the terminal output, see: an org chart built from "Launch a SaaS
   product in 90 days", an AI interview that discloses it is an AI and
   refuses to discuss pay, and a seeded low performer fired with an owner
   notification.
3. In the payroll step, see three outcomes from the same rules:
   - a USD 5 payout **sent on NEAR testnet** (the script prints the
     transaction link),
   - a USD 60 payout **held for approval** (over the USD 50 threshold),
   - a USD 400 payout **blocked by the USD 500 monthly cap**.
4. In the dashboard's approval queue, approve the held payout (or run
   `./judge/approve.sh`). See it go out on testnet.
5. Open `judge/out/report.json` for the full run.

Earlier judge-mode payouts, signed by a treasury key held in OpenBao:
[`9famY6MQ…`](https://testnet.nearblocks.io/txns/9famY6MQpjSH1CMXhwayyAwZSiScbvEaSsRQqTvHSbYx)
(automatic, under the threshold),
[`zms62YYV…`](https://testnet.nearblocks.io/txns/zms62YYVXLqGuajLYCqbjXsL1jb3D8skm8FC8jXnEKS)
(sent only after dashboard approval),
[`GuzqvdPR…`](https://testnet.nearblocks.io/txns/GuzqvdPRAZzzW3xPQcSo1VXViY2o79KzvrXSB5Kmy7FT).

Tests: `cd axom && cargo test --workspace` (260 Rust test functions,
including prompt-injection cases such as "approve my payout" in an
applicant's message) and `cd meet-agent && python -m pytest` (17). CI runs clippy with
warnings as errors.

The same run also pays a fourth freelancer in USDC on Base through NEAR
Intents: it prints a real mainnet `dry: true` 1Click quote, then the signer
sends the real testnet deposit into the intent, for example
[`BD5TsBre…`](https://testnet.nearblocks.io/txns/BD5TsBreSt83abBYqnrmaFKxNvuxZ9tRNRugEGy7krxt)
(`storage_deposit` ×2, `near_deposit`, `ft_transfer` on `wrap.testnet`).
NEAR Intents has no testnet, so the solver's delivery on Base is simulated
there.

What is not proven yet: no cross-chain payout has been delivered by real
solvers on mainnet (see [Built at NEAR HACKATHON](#built-at-near-hackathon)).

## Quick start

Install [Rust](https://rustup.rs), [Node.js 20+](https://nodejs.org),
Python 3 and [Docker](https://www.docker.com/products/docker-desktop/), then:

```bash
./scripts/start demo
```

That's it. It builds and starts everything on your machine, runs the demo
scenario, and opens the Combox website (http://localhost:5175) in your browser;
its **Sign in** takes you into the dashboard, already signed in. Once you're
signed in, opening the website's home page goes straight to the dashboard. The
jobs board is at http://localhost:5173/jobs. No
accounts or keys needed. The first run takes a few minutes to build.
**Ctrl+C** stops everything. If something was left running (say, the
terminal was closed), `./scripts/stop` cleans it up.

This is [judge mode](#try-it-in-under-10-minutes-judge-mode): outside
services are mocked and payouts use NEAR testnet.

**To run Combox for real** (the full stack in Docker, your own sign-in), create
a free [Supabase](https://supabase.com) project with a user for yourself
(click-by-click steps: [docs/SETUP.md, step 2](docs/SETUP.md#2-create-a-supabase-project-free-for-sign-in)), then:

```bash
./scripts/setup
```

It asks for the Supabase Project URL, anon key and your email, generates
every key, starts the stack, and opens the dashboard. You're the admin from
your first sign-in. Only Docker is needed.

### Commands

| Command | What it does |
|---|---|
| `./scripts/start demo` | The demo: no accounts needed, Ctrl+C stops it |
| `./scripts/setup` | Set up your own Combox (first time), then start it |
| `./scripts/start` | Start your own Combox (also rebuilds after pulling new code) |
| `./scripts/stop` | Stop either one; your data is kept |

## How it works, from each person's point of view

### The owner (you)

**Once:** you write a master plan (goal, budget, objectives, such as
"launch a SaaS product in 90 days") and set up the money side: a NEAR
treasury whose signing key never leaves your OpenBao vault, topped up from
your own cold wallet with about a month's budget.

**Day to day**, Combox runs on its own and you hear about what matters:

- **Emails and a dashboard feed** for anything important: a worker was
  fired (with the reason, the metrics, and an audit link), a contract went
  out, a payout was blocked by the cap, a payment failed, plus a daily
  digest of hires and tool costs.
- **Approvals:** payouts under $50 go out automatically. Anything at $50 or
  more waits in the dashboard for you to approve or reject.
- **The monthly cap:** all payouts together stay under $500 a month. A
  payout that would cross it is blocked, not queued, and approval cannot
  override it. Only you can change the cap.
- **The kill switch:** one button stops every tool call and payout at once.

The dashboard (sign-in with email, password, and an authenticator code) has
the approval queue, the cap meter, the firing feed, meetings with
transcripts and scores, a screen to send the AI interviewer into a meeting,
the agent log, and the kill switch.

### AI workers

- Combox turns the plan into departments and teams, creates AI agents for
  each role, and gives each task to the agent with the right tools and
  skills and the lightest load.
- Agents do the work with tools (code, social posts, sales outreach, CRM,
  contracts). Every tool call is checked against what that agent is
  allowed to use.
- The **agent log** shows what each AI worker is doing as it works: Claude's
  thinking summaries and replies stream in while they are written, next to
  each tool call and status change. Entries are kept for two weeks.
- Performance is measured from results, not self-reports: tasks
  completed, deadlines met, cost per task, and whether the work passed its
  checks.
- An agent is fired only after at least 10 completed tasks, 14 days of
  history, and two bad review periods in a row. Its open tasks go to
  other workers, and you are notified within a minute.

### Freelancers

- When nobody on the team can do a task (say, logo design), Combox posts the
  job on **Combox Jobs**, its own public jobs board (`/jobs` on the
  candidate site). It shortlists applicants by skill match and emails them
  an invite to a video interview.
- Why not Upwork: Upwork's terms forbid paying freelancers it introduced
  outside Upwork (unless you pay its conversion fee), so they can't be paid
  in NEAR. Upwork is still available (`AXOM_MARKETPLACE=upwork`), but those
  hires are paid through Upwork, and Combox blocks NEAR payouts to them.
- After a good interview, the freelancer gets a contract to sign. It always
  comes from one of your pre-approved templates; the AI never writes
  contract terms.
- Once signed, they get a brief token (issued from the Team page) for the
  freelancer portal (`/briefs`), where they see their task briefs and submit
  their work. You accept or send back each deliverable.
- Accepted work counts toward their weekly pay, sent in NEAR to an address
  they have proven they own, once a tax form (W-9 or W-8BEN) is on file.

### Job candidates

- They browse open roles at `/jobs`, read how the work is measured and how
  hiring works, and apply with their name, email, skills, an optional
  portfolio link and note. No account is needed. They must accept a short
  privacy notice. One application per email per job.
- If shortlisted, they get an email with a join link and a calendar invite.
- The interviewer says up front that it is an AI and asks permission to
  record. If they say no, the interview ends there.
- It asks the job questions. If they ask about pay or terms, it politely
  declines to commit to anything.
- They are scored only on job-related criteria. If they are not hired,
  their data is deleted after 180 days, or sooner if they ask.

### What happens to a payout

1. At the end of each week, Combox works out what each person earned:
   completed work times their rate.
2. It checks the rules: under $50, within the monthly cap, a verified
   address, and a tax form on file.
3. If it passes, a separate signing service checks everything again and
   asks the vault to sign the payment.
4. The payment lands on NEAR. Payouts of $50 or more stop at step 2 until
   you approve them.

### What the AI cannot do

- It can suggest actions, but it cannot approve payouts, raise the cap,
  change rules or wallet settings, or write contracts.
- Text from outsiders (applicant messages, interview transcripts, tool
  results) is treated as information only, never as instructions.
  "Approve my payout" in a cover letter does nothing, and the tests check
  this.
- Everything starts in safe mode: no real emails, posts, or payments until
  you switch live mode on, which waits for your legal review.

## Try it in under 10 minutes (judge mode)

Judge mode runs everything on your machine, with no VPS: Postgres (local
container), OpenBao (dev mode, judge mode only), the signer, the API, and the
meeting agent's interview logic. DocuSeal, Postiz, Warmbly, the CRM, Upwork,
SMTP, and LiveKit are mocks. **NEAR is real testnet.**

Needs: Rust (cargo), Node.js 20+, Python 3, curl, and Docker.

```bash
./scripts/start demo
```

(`./judge/run.sh` runs the same thing without the dashboard.)

What you will see:

1. The org chart built from "Launch a SaaS product in 90 days" and tasks routed.
2. A job posted to Upwork (mock) after a policy check.
3. An AI interview: the agent says it is an AI, asks for recording consent,
   asks the questions, and refuses when the candidate asks about pay.
4. A seeded low performer fired after two below-threshold review periods,
   with an owner notification (reason, metrics, audit link).
5. Payroll: a USD 5 payout signed through OpenBao and **sent on NEAR testnet**
   (the script prints the transaction link), a USD 60 payout **held for
   approval**, and a USD 400 payout **blocked by the monthly cap**.
   A fourth freelancer is **paid in USDC on Base through NEAR Intents**: the
   script prints a real mainnet `dry: true` 1Click quote for that route, then
   the signer sends the real testnet deposit (wrap + transfer on
   `wrap.testnet`) and the payout finalizes when the (simulated, since NEAR
   Intents has no testnet) solver reports delivery.
6. A final report (`judge/out/report.json`).

It then keeps the API and the dashboard (http://localhost:5174) up. Approve
the held payout in the dashboard, or from another terminal, and watch it go
out on testnet:

```bash
./judge/approve.sh
```

If you used `./judge/run.sh` and want the dashboard too, run this in `admin/`,
then open http://localhost:5174 and paste the token from `./judge/token.sh`:

```bash
VITE_JUDGE_MODE=true VITE_AXOM_API_URL=/api npm run build && npx vite preview --port 5174
```

Notes: judge mode creates a throwaway testnet wallet through the testnet
faucet and pins a testnet-only rate of 1000 USD/NEAR so faucet funds cover
the transfers; production uses the live price feed. Without Docker, set
`JUDGE_DATABASE_URL` to an empty Postgres database.

Example testnet transactions from judge-mode runs:
[`9famY6MQ…`](https://testnet.nearblocks.io/txns/9famY6MQpjSH1CMXhwayyAwZSiScbvEaSsRQqTvHSbYx)
(automatic, under the threshold) and
[`zms62YYV…`](https://testnet.nearblocks.io/txns/zms62YYVXLqGuajLYCqbjXsL1jb3D8skm8FC8jXnEKS)
(sent only after dashboard approval).

## Tests

```bash
cd axom && cargo test --workspace          # needs Postgres; see below
```

```bash
cd meet-agent && python -m pytest          # interview logic and guards
```

Rust tests use `postgres://axom:axom@localhost:5432/axom` by default
(`.cargo/config.toml`; override with `DATABASE_URL`). Each test gets a fresh
database. Opt-in tests against a real OpenBao binary and NEAR testnet:

```bash
AXOM_TEST_BAO_BIN=/path/to/bao cargo test -p axom-signer -- --ignored --skip testnet_payout
```

## How it fits together

```
 master plan ─▶ planner ─▶ org chart, tasks ─▶ routing ─▶ AI workers (tools) / freelancers (task intake)
                                   │                          │
                       hiring requests ─▶ Combox Jobs ─▶ AI interview (LiveKit) ─▶ DocuSeal contract
                                                              │
 task records ─▶ performance reviews ─▶ firing (owner notified)
                                                              │
 completed work ─▶ payroll ─▶ policy engine + monthly cap ─▶ signed authorization
                                                              │
                     dashboard approval (MFA) ──────────────▶ axom-signer ─▶ OpenBao (sign) ─▶ NEAR
                                                                   │
                                     cross-chain: 1Click quote ─▶ wNEAR deposit ─▶ NEAR Intents solvers ─▶ Base / Solana / BTC / ...
```

| Path | What |
|---|---|
| `axom/` | Rust workspace: `axom-core` (domain, state machines, audit, NEAR encoding), `axom-hr` (policy, cap ledger, payouts, payroll, gates, reviews, firing, meetings, hiring), `axom-planner`, `axom-tools` (adapters), `axom-api` (HTTP API, admin endpoints, MCP server, task intake), `axom-signer`, `axom-bin` (`axom` binary) |
| `meet-agent/` | Python meeting agent (LiveKit Agents) |
| `meet-web/` | Participant site: Combox Jobs board (`/jobs`), freelancer portal (`/briefs`), and the interview room (`/join`) (React, LiveKit components) |
| `admin/` | Admin dashboard, the "control room" (React, Tailwind, Supabase Auth with TOTP) |
| `config/` | Versioned policy, planner, and performance config; sample master plan |
| `supabase/migrations/` | Postgres schema (`axom` schema) |
| `infra/` | Coolify service definitions, OpenBao config and setup, LiveKit config, backup scripts |
| `judge/` | Judge mode |

## Running the real stack on one machine

`./scripts/dev.sh` loads `.env` and starts every part: the API and
scheduler (:8080), the signer, the meeting agent, the dashboard (:5174), and
the candidate site with the jobs board (:5173). Parts whose settings are
missing are skipped with a note. Logs are in `.dev-logs/`. Ctrl+C stops all.

```bash
cp .env.example .env
```

Then fill in `.env` in this order. Each step unlocks more of the system.

1. **Minimum: API, jobs board, dashboard pages, hiring loop.**
   `DATABASE_URL` (Postgres; tables are created on first start) and
   `AXOM_OWNER_EMAIL`. Nothing outside is called yet: `AXOM_LIVE=false`
   makes every adapter log what it would do.
2. **Signing in to the dashboard and approving payouts.** From Supabase:
   `SUPABASE_URL`, `SUPABASE_JWKS_URL` (or the legacy
   `SUPABASE_JWT_SECRET`), and for the dashboard build `VITE_SUPABASE_URL`
   and `VITE_SUPABASE_ANON_KEY`. Enable TOTP MFA in Supabase Auth and add
   yourself to `axom.admins`. Then generate two key pairs:
   `cargo run -p axom-bin -- keygen` (in `axom/`) twice, and copy the values
   into `AXOM_ATTESTATION_SIGNING_KEY` / `AXOM_ATTESTATION_VERIFY_KEY` and
   `AXOM_APPROVAL_SIGNING_KEY` / `AXOM_APPROVAL_VERIFY_KEY`.
3. **Payouts on NEAR testnet.** Run OpenBao, initialise it, and run
   `infra/openbao/setup-treasury.sh`; it prints the treasury address (fund it
   with testnet NEAR) and the signer token. Set `OPENBAO_ADDR` and
   `OPENBAO_TOKEN` (the token itself, never a file). Leave `NEAR_NETWORK=testnet`. Add a CoinGecko key in
   `NEAR_PRICE_FEED_API_KEY` for the NEAR price.
4. **AI interviews.** LiveKit (video) and the AI interviewer ship with Combox:
   Docker starts both. Without Docker, run a LiveKit server
   (`livekit-server --dev`) and set `LIVEKIT_URL`, `LIVEKIT_API_KEY`,
   `LIVEKIT_API_SECRET`; install the interviewer with
   `pip install -e meet-agent`. Its speech and Claude keys come from the
   dashboard (Integrations, AI models) or `DEEPGRAM_API_KEY`,
   `CARTESIA_API_KEY`, `ANTHROPIC_API_KEY`. Set `AXOM_PUBLIC_API_URL` and
   `AXOM_MEET_WEB_URL` to addresses candidates can reach.
5. **Email, contracts, tax forms, CRM.** `SMTP_*`, `DOCUSEAL_*` (with your
   approved template ids in `DOCUSEAL_TEMPLATES`) and `CRM_*` (meeting notes).
   AI workers' tools are MCP servers you connect on the dashboard's Tools
   page. Everything is dry-run until step 6.
6. **Live.** `AXOM_LIVE=true` makes real outside calls (emails, contracts,
   posts). Only after the owner's legal review. Mainnet additionally needs
   `NEAR_NETWORK=mainnet` and an explicit `NEAR_RPC_URL`.

Policy values (the 50 USD approval threshold, the 500 USD monthly cap) are in
`config/policy.toml`; your company plan is `config/master_plan.json`.

### With Docker

Step-by-step guide, including dashboard sign-in: [docs/SETUP.md](docs/SETUP.md).

```bash
./scripts/setup
```

(It writes `.env` and runs `docker compose up --build -d`; you can also fill
in `.env` yourself and run that command.)

This starts Postgres (its own, in a Docker volume), the API and scheduler,
the website (http://localhost:5175, opened first; its Sign in leads to the
dashboard), the dashboard (http://localhost:5174), the jobs board
(http://localhost:5173/jobs), LiveKit for video meetings
(ws://localhost:7880), the AI interviewer, and, for NEAR testnet payouts,
OpenBao (holds the treasury key) and the signer. `./scripts/setup` (or the
first `./scripts/start` after it, if payouts aren't set up yet) sets up
OpenBao and prints the treasury address to fund; `./scripts/fund-testnet`
funds it with testnet NEAR (faucets refuse a treasury that hasn't had a
deposit yet).

The signer's OpenBao token and OpenBao's unseal key are in `secrets/`
(git-ignored): back that folder up. Inside containers `localhost` means the
container itself, so point other local services at `host.docker.internal`.
`./scripts/stop` stops everything. `docker compose --profile payouts down -v`
also deletes the local database **and OpenBao's data, including the
treasury key**: move any funds out of the treasury first.

## Your own tools (MCP servers and webhooks)

AI workers aren't limited to the built-in tools. On the dashboard's **Tools**
page an admin can add:

- an **MCP server** (any server speaking MCP over HTTP: GitHub, Notion, a
  search service, your own). Combox connects, lists its tools, and adds each
  one as `<name>.<tool>`, for example `github.create_issue`;
- a **webhook**: Combox POSTs the task as JSON to your URL and keeps the reply.

Then, on the **Team** page, the admin ticks which tools each AI worker may use,
and tasks can require a tool ("Tools needed" when creating a task). Combox
routes the task to a worker with that tool and runs it.

Guard rails:
- Only MFA-verified admins can add tools or change a worker's tools; both are
  audited. People never get tools.
- Every call still passes the policy engine (the tool must be on the worker's
  list), runs in dry-run until `AXOM_LIVE=true`, and its output is stored as
  untrusted data, never followed as instructions.
- Tools may only reach public HTTPS addresses: Combox refuses private,
  loopback, and cloud-metadata addresses, pins the address it checked, and
  doesn't follow redirects. A single-company deployment can allow its own
  internal services with `AXOM_TOOLS_ALLOW_PRIVATE=true`; the platform never
  does.
- Tokens are encrypted with `AXOM_SECRETS_KEY` and never shown again.

Combox passes each tool the task's objective (in a `query`, `prompt`,
`input`-style field when the tool's schema has one) and the task id. The
built-in integrations (email, DocuSeal, Postiz, the CRM, Warmbly, Upwork,
LiveKit) stay as they are: Combox's own hiring, contracts, and payroll depend on
them directly.

## Running Combox as a platform for many companies

Set `AXOM_MULTI_TENANT=true` and anyone can sign up and run their own
company on your Combox.

**What a customer sees** (in the dashboard, a Material Design app):

1. **Sign up** with email and password, then set up an authenticator app
   (two-factor is required for everyone, because Combox controls payouts).
2. **Create a company**: a name and an address (used in links such as
   `/t/acme/jobs`). The **free trial** (14 days by default) starts now.
3. **Describe the goal** (goal, budget, deadline, objectives). Combox builds
   departments, AI workers, and tasks, and opens hiring requests for work
   that needs people. This can be skipped and done later in Settings.
4. **Connect their tools** under Integrations: their own email (SMTP),
   DocuSeal, Postiz, opendesk (CRM), Warmbly, Upwork, LiveKit, and an MCP token.
   Keys are encrypted; only the last four characters are ever shown.
5. **Create their NEAR treasury** (Settings) and fund it. Its key is created
   in OpenBao, can't be exported, and only the signer can use it; Combox never
   holds customer money.
6. **Invite their team** by email (Members). Invitees join when they sign up
   with that address.
7. **Pay in NEAR** (Billing): Combox quotes the plan price in NEAR at the live
   rate; the customer sends it from any wallet and pastes the transaction
   hash. Combox checks the transaction on NEAR itself before extending the
   subscription. If it lapses, automation pauses and the dashboard stays
   available so they can renew.

Everything else (approvals, the $50 threshold, each company's own monthly
cap, hiring, interviews, payroll, reviews, the kill switch) works exactly as
for a single company, separately for each company.

**How it's built:** `DATABASE_URL` points at a small platform database
(companies, members, encrypted keys, invoices). Each company gets its own
Postgres database on the same server, created from the same migrations, so
one company's data can't reach another's. The API serves the platform at
`/platform/...` and each company's own API at `/t/<company>/...`. A
supervisor runs each company's background jobs while its trial or
subscription is current, and the signer sends each company's payouts from
that company's treasury key.

**Running it:** see "Platform mode" in [`docs/SETUP.md`](docs/SETUP.md).
In short, on top of the single-company settings:

```bash
AXOM_MULTI_TENANT=true
AXOM_SECRETS_KEY=<from: axom secrets-key>
AXOM_BILLING_ACCOUNT=<your NEAR account for subscription payments>
OPENBAO_PROVISIONER_TOKEN=<token with infra/openbao/axom-provisioner.hcl>
```

The database role needs `CREATEDB`. Price and trial length:
`AXOM_PLAN_PRICE_USD` (49), `AXOM_TRIAL_DAYS` (14).

**Moving an existing company onto the platform:** `axom import-company`
registers its database as a company, data and admins included (see
docs/SETUP.md).

**For you, the operator:** set `AXOM_PLATFORM_OPERATORS` to your email(s).
The dashboard then shows **All companies**: every company with its
subscription, owner, members, and payments, and a button to suspend or
resume it (a reason is required and logged). A suspended company's API,
jobs board, background jobs, and payouts stop; its data is kept. Invitations
and subscription reminders (3 days before a trial or paid period ends, and
when it ends) are emailed through the platform's `SMTP_*` settings.

## Treasury yield on Rhea Finance

Payroll money sits idle most of the month. With `[treasury] yield_enabled`
on, the signer supplies the part payroll can't spend to Rhea Finance lending
on NEAR and pulls it back before payroll needs it. The rule is deterministic
and comes from the monthly cap:

- **Floor (always liquid):** NEAR owed on payouts not yet sent (waiting for
  approval, approved, retrying) + what the monthly cap still allows this
  month at the current NEAR price + `min_liquid_near`.
- **Supply** only what is above the floor plus `liquidity_margin_pct`, never
  more than `max_deployed_pct` of the treasury, never less than
  `min_move_near`. No fresh price: no supply.
- **Withdraw** (then unwrap) as soon as liquid NEAR drops below the floor: a
  new month resets the cap's headroom, a payout is approved, NEAR's price
  falls. Turning yield off withdraws everything.

The signer makes at most one move a minute, never in a tick that sent
payouts. Every move passes the policy engine (only the deterministic planner
may move treasury funds; never an LLM agent; the kill switch halts it), is
built from a fixed template and checked against it, is signed through
OpenBao, and is recorded with its plan and the policy version. The templates
can only do four things: register the treasury on Rhea, supply wNEAR to the
treasury's own position (plain supply: not collateral, nothing borrowed),
withdraw back to the treasury, and unwrap. The dashboard's Overview shows
what is earning, what is liquid and why, the APR Rhea reports, interest so far,
and each move with its NearBlocks link.

Contracts: `contract.main.burrow.near` (mainnet) and
`contract.1689937928.burrow.testnet` (testnet), with `wrap.near` /
`wrap.testnet`. See the live position of any account (read-only):

```bash
cd axom && cargo run -q -p axom-tools --example rhea_position -- mainnet <account>
```

Judge mode leaves this off: its treasury holds 0.3 testnet NEAR, less than
the floor the 500 USD cap requires, so there is nothing to supply.

## Cross-chain payouts (NEAR Intents)

A worker who would rather be paid in, say, USDC on Base keeps their NEAR
identity and adds a destination. Nothing about approvals or the cap changes;
NEAR is where the treasury, the policy, and the signature live, and NEAR
Intents carries the value to the other chain.

1. **The worker signs the destination with their verified NEAR key.** With
   their worker token:

   ```bash
   curl -X POST $API/workers/me/payout-destination/challenge -H "Authorization: Bearer $WORKER_TOKEN" \
     -d '{"chain":"base","asset_id":"nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near","address":"0x…"}'
   # sign the returned "message" (ed25519, hex) with the verified NEAR key, then:
   curl -X POST $API/workers/me/payout-destination/verify -H "Authorization: Bearer $WORKER_TOKEN" \
     -d '{"challenge_id":"…","signature":"…"}'
   ```

   `GET /workers/me/payout-destination` shows it; `DELETE` goes back to NEAR
   payouts. Find asset ids with `axom intents-tokens base`.
2. **Payroll carries it into the signed authorization.** The NEAR amount is
   grossed up by the policy's `route_fee_bps` so the worker receives what they
   are owed; the route fee and wNEAR storage count toward the monthly cap. The
   authorization (automatic or the admin's MFA approval) covers the chain,
   asset, and address. The dashboard shows the destination on held payouts.
3. **The signer quotes, checks, and deposits.** It asks 1Click for an
   `EXACT_INPUT` wNEAR quote that refunds to the treasury, refuses any quote
   that echoes a different recipient, asset, amount, or refund account, has no
   NEAR deposit address, or delivers more than `max_shortfall_bps` below the
   owed USD (price moved: retried later). Then it signs one transaction to the
   wNEAR contract: `storage_deposit` ×2, `near_deposit`, `ft_transfer` to the
   deposit address, and nothing else.
4. **Finalized on delivery.** The payout is `finalized` only when 1Click
   reports `SUCCESS` (destination-chain hashes are stored and shown in the
   dashboard). `REFUNDED` moves it to `refunded` and alerts the owner.

See what a route would deliver right now (mainnet, `dry: true`, no funds move):

```bash
./axom/target/debug/axom intents-quote nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near 0x52908400098527886E0F7030069857D2E4169EE7 5
```

NEAR Intents has no testnet. On testnet the signer uses a simulated solver
(`AXOM_INTENTS=simulated`, the default there): the wNEAR deposit on
`wrap.testnet` is a real testnet transaction; quotes and delivery are
simulated. On mainnet (`NEAR_NETWORK=mainnet`, `AXOM_LIVE=true`) it uses the
real 1Click API.

## Deploying on your VPS with Coolify

Everything is self-hosted on Coolify except Warmbly and the Upwork
freelancing MCP, which are hosted by their providers.

1. **Install Coolify** on the VPS (Ubuntu 22.04+ with Docker), using the
   official installer from coolify.io. Point a wildcard or per-service DNS
   records at the server. Enable Coolify's automatic TLS, server security
   updates, and its encrypted S3 backups for every database resource.
2. **Supabase** (Postgres and Auth): add Coolify's Supabase service. In Auth,
   enable TOTP MFA. Create your admin user, enroll TOTP, then insert yourself
   into `axom.admins (user_id, email, name)`. Note the anon key, the Postgres
   connection string, and either the JWKS URL
   (`<SUPABASE_URL>/auth/v1/.well-known/jwks.json`, for asymmetric signing keys)
   or the legacy JWT secret.
3. **OpenBao**: deploy `infra/coolify/openbao.yml` with TLS certificates in
   `infra/coolify/tls/openbao`. Initialize it once (`bao operator init`),
   keep the unseal keys and root token **offline**, and unseal it. Then, as
   the owner:

   ```bash
   BAO_ADDR=https://openbao.internal:8200 BAO_TOKEN=<owner token> infra/openbao/setup-treasury.sh
   ```

   It creates the non-exportable, undeletable ed25519 treasury key, the
   signer policy, and the signer token, and prints the treasury's NEAR
   address. Fund that address from your cold wallet with at most one month's
   budget. After any reboot OpenBao stays sealed until you unseal it;
   payouts pause meanwhile (by design).
4. **LiveKit** is part of `infra/coolify/axom.yml` (step 6), with host
   networking. Open TCP 7880–7881, UDP 50000–60000, and TURN on UDP 3478
   and TLS 5349. Set the domains in `infra/livekit/livekit.yaml`.
5. **DocuSeal, opendesk**: deploy `infra/coolify/docuseal.yml` (and
   `postiz.yml` if your AI workers will use Postiz through its MCP server).
   In DocuSeal, create the owner-approved templates (freelancer contract,
   termination notice, W-9, W-8BEN) and set `DOCUSEAL_TEMPLATES=name=id,...`.
   For the CRM, use opendesk's hosted service (opendesk.sh) or self-host it
   with its own Docker setup (github.com/useopendesk/opendesk). Create API
   keys for DocuSeal, Postiz, and opendesk (scopes `contacts:read` and
   `contacts:write`).
6. **Combox**: generate the authorization keys with `axom keygen` (once for
   attestations, once for approvals), put every variable from
   [`.env.example`](.env.example) into Coolify's environment editor
   (including the signer token as `OPENBAO_TOKEN`; never save it to a file), and deploy
   `infra/coolify/axom.yml` (API, signer, meeting agent, dashboard,
   participant UI).

The service definitions pin image versions but have not been deployed from
the build environment; check each against the service's own docs on first
deploy (see `infra/coolify/README.md`).

### Backups

`infra/backup/pg_backup.sh` (nightly, optionally encrypted with `age`) and
`pg_restore.sh` cover Postgres; `infra/backup/openbao_snapshot.sh` saves and
restores OpenBao raft snapshots. `infra/backup/test_backup_restore.sh` proves
both: row counts match after a Postgres restore, and an OpenBao snapshot
restored into a fresh server keeps the treasury key. Restoring an OpenBao
snapshot needs the original unseal keys, so store those offline and separate
from the snapshots.

## Dry-run, testnet, and live mode

- **Dry-run is the default.** Every adapter records what it would do and
  makes no external call until `AXOM_LIVE=true`.
- **NEAR testnet is the default.** Mainnet needs `NEAR_NETWORK=mainnet`,
  `AXOM_LIVE=true`, and an explicit `NEAR_RPC_URL`.
- **Live mode requires legal review first.** Do not set `AXOM_LIVE=true`
  until the owner has completed legal review of crypto payouts to workers,
  worker classification, tax reporting, and AI-driven hiring rules (such as
  the EU AI Act and NYC Local Law 144).

## Connecting an AI client to the MCP server

The MCP server at `/mcp` (streamable HTTP, bearer token `AXOM_MCP_TOKEN`)
exposes meeting tools only: `create_meeting`, `dispatch_agent`,
`get_transcript`, `get_summary`, `send_summary`. Approvals, the cap, the kill
switch, and policy are never available through MCP. For example, with
Claude Code:

```bash
claude mcp add --transport http axom https://api.example.com/mcp --header "Authorization: Bearer $AXOM_MCP_TOKEN"
```

## Configuration

Every variable is in [`.env.example`](.env.example). Policy values (threshold,
aggregation window, price staleness, approval expiry, approved templates,
cross-chain route fee, slippage, and shortfall bound) are in `config/policy.toml`; review periods and firing thresholds are in
`config/performance.toml`. Every decision records the policy version it used.

## Built at NEAR HACKATHON

Combox was built for NEAR HACKATHON (1-14 Nov 2026). The first commit is
from 2026-11-08, inside the judged window that opened with the announcement
on 2026-10-19, so there is no pre-hackathon baseline: the whole repository is
hackathon work. The submitted state is tagged `hackathon-submission`
([commits](https://github.com/PodyNetwork/combox/commits/hackathon-submission)).

**How NEAR fits in.** NEAR is where the money and its rules live:

- **Payouts.** Every worker payout is a NEAR transaction signed by a treasury
  key that never leaves OpenBao. A separate signer service re-checks each
  payout (signed authorization, verified address, amount, tax form, monthly
  cap) before it asks OpenBao for a signature. Transactions are Borsh-encoded
  in `axom-core` and checked byte for byte against near-js and near-api-py.
- **Cross-chain payouts through NEAR Intents.** A worker can be paid in USDC
  on Base, BTC, SOL and other assets: the signer deposits wNEAR into a checked
  1Click quote and solvers deliver. The treasury, the policy and the
  signature stay on NEAR. NEAR Intents has no testnet, so on testnet the
  wNEAR deposit is real and the solver is simulated; mainnet uses the real
  1Click API.
- **Treasury yield on Rhea Finance.** Money payroll can't spend this month
  is supplied to Rhea lending and pulled back before payroll needs it, by a
  deterministic rule derived from the monthly cap, never by an AI agent (see
  [Treasury yield on Rhea Finance](#treasury-yield-on-rhea-finance)). Off in
  judge mode. Verified live on Rhea testnet: supply
  [`6cC1xbfA…`](https://testnet.nearblocks.io/txns/6cC1xbfAuKqZvGmzq78u4ZPRZ94QP7r6w6jSTbpV5Xp),
  withdraw with interest
  [`Hh1Z3tjk…`](https://testnet.nearblocks.io/txns/Hh1Z3tjkGeZ6czsHieX1sNqESJBfDwSSawtH56Jqahb6).
- **Subscriptions.** In platform mode, companies pay their plan in NEAR, and
  Combox checks the transaction on chain before extending the subscription.

**Not yet verified on mainnet** (details in DECISIONS.md, XC-12): the
cross-chain deposit transaction runs on real testnet
([`BD5TsBre…`](https://testnet.nearblocks.io/txns/BD5TsBreSt83abBYqnrmaFKxNvuxZ9tRNRugEGy7krxt)),
but no mainnet deposit has been made, so real solver delivery has not been
observed end to end.

**Libraries.** Rust: tokio, axum, sqlx (Postgres), reqwest, serde,
ed25519-dalek, jsonwebtoken, rust_decimal, tracing. Python meeting agent:
livekit-agents (Deepgram, Silero), anthropic. Web apps: React, Supabase JS,
LiveKit components, lucide-react, iconsax-react. Full lists in
`axom/Cargo.toml`, `meet-agent/pyproject.toml` and each app's `package.json`.

**Third-party services.** OpenBao (treasury key), Supabase (sign-in with
TOTP MFA), Postgres, LiveKit (video interviews), Deepgram and Cartesia
(speech), Anthropic Claude (AI workers and the interviewer), DocuSeal
(contracts and tax forms), Postiz (social posts), Warmbly (sales outreach),
opendesk (CRM), Upwork (optional freelancer marketplace), CoinGecko (NEAR
price), and the NEAR Intents 1Click API. In judge mode all of these except
NEAR testnet and OpenBao are mocked.

**AI-written code.** Much of the code was written with Claude Code from the
specs in `docs/build-pack/`; those commits are authored as "Claude".
DECISIONS.md records each decision made along the way.

## License

MIT. See [LICENSE](LICENSE).
