# Setting up ForeMan on your computer

This gets the full ForeMan stack running in Docker and signs you in to the
dashboard. You do one thing by hand (a free Supabase project for sign-in);
`./scripts/setup` does the rest. Payouts and AI interviews are optional extras at
the end.

(Just want to look around first? `./scripts/start demo` runs a demo with no accounts at
all: see "Quick start" in the README.)

## 1. Install

- **Docker Desktop** (docker.com). `./scripts/setup` starts it if it isn't running.
- **Git**.

```bash
git clone https://github.com/PodyNetwork/foreman.git
cd foreman
```

## 2. Create a Supabase project (free) for sign-in

ForeMan has no built-in passwords. You sign in with a Supabase account plus a
6-digit code from an authenticator app. This takes about 5 minutes.

Supabase updates its dashboard from time to time, so a menu name may be
slightly different from what's written here. Look for the closest match.
Everything below is in the **left sidebar** of your project.

**a. Create the project**

1. Go to [supabase.com](https://supabase.com) and sign up or sign in.
2. Click **New project**. Pick any name, set a database password (ForeMan
   doesn't use it, but keep it), choose the region closest to you, and click
   **Create new project**. It takes about a minute to start.
3. Open the project from your **Projects** page.

**b. Turn on email sign-in**

1. In the sidebar, click **Authentication**.
2. Under **Configuration**, click **Sign In / Providers**.
3. Make sure **Email** is **enabled**. It is by default.

**c. Turn on two-factor sign-in**

1. Still in **Authentication**, under **Configuration**, click
   **Multi-Factor**.
2. Set **TOTP (App Authenticator)** to **Enabled**, and click **Save** if
   the button appears.

The dashboard won't let anyone in without this.

**d. Create your user**

1. In **Authentication**, click **Users**.
2. Click **Add user** (top right), then **Create new user**.
3. Enter your **email** and a **password**, and tick **Auto Confirm User**.
4. Click **Create user**.

Forgot to tick Auto Confirm? Open the user from the list and confirm their
email there. Use this same email in step 3: it's the one that becomes the
admin.

**e. Find the two values `./scripts/setup` asks for**

1. In the sidebar, click **Project Settings** (at the bottom).
2. **Project URL:** click **Data API** and copy the **Project URL**, which
   looks like `https://abcdefgh.supabase.co`.
3. **Key:** click **API Keys** and copy either the **Publishable key**
   (starts with `sb_publishable_`) or, on the **Legacy API Keys** tab, the
   **anon public** key (a long string starting with `eyJ`). **Never** use the
   **secret** or **service_role** key: it gives full access to your project.

Shortcut: the **Connect** button at the top of the project page also shows
the Project URL and key together.

Only if `./scripts/setup` asks for a **Legacy JWT secret** (older projects): go to
**Project Settings → JWT Keys** and copy the **Legacy JWT secret**. Most
projects don't need this.

## 3. Run the setup

```bash
./scripts/setup
```

It asks for three things: the Project URL, the anon key, and your email
(the user from step 2). It checks each one against Supabase as you go. Then
it generates ForeMan's signing and encryption keys, writes everything to
`.env`, builds and starts the stack, and opens the ForeMan website
(http://localhost:5175). The first build takes several minutes.

Keep a copy of `.env` somewhere safe (it holds the keys). It stays on your
machine and is never committed.

## 4. Sign in

1. The ForeMan website opens at **http://localhost:5175**. Click **Sign in**
   to go to the dashboard (**http://localhost:5174**). Once you're signed in,
   the website's home page sends you straight to the dashboard.
2. Enter the email and password from step 2.
3. The first time, a QR code appears: scan it with an authenticator app
   (Google Authenticator, Microsoft Authenticator, 1Password, Authy), then
   type the 6-digit code. Next time you only type the code.

You're the admin from your first sign-in: ForeMan makes the owner's email
(`AXOM_OWNER_EMAIL`) an admin the first time that person signs in with the
two-factor code, as long as there are no admins yet. You now see the
dashboard: org chart, tasks, the monthly cap, payouts waiting for approval,
notifications, meetings, and the kill switch.

The public jobs board is at **http://localhost:5173/jobs**.

## 5. Everyday use

```bash
./scripts/start              # start (also rebuilds after pulling new code)
./scripts/stop               # stop; your data is kept
docker compose logs -f axom  # watch ForeMan's log
docker compose --profile payouts down -v   # DELETE the local database and the treasury key
```

Your company plan is `config/master_plan.json`; the approval threshold
(50 USD) and monthly cap (500 USD) are in `config/policy.toml`. Restart after
changing them.

<details>
<summary>Setting it up by hand instead of <code>./scripts/setup</code></summary>

1. `cp .env.example .env`
2. Generate two key pairs (the first for attestations, the second for
   approvals) and an encryption key:
   ```bash
   docker compose run --rm --no-deps axom axom keygen
   docker compose run --rm --no-deps axom axom keygen
   docker compose run --rm --no-deps axom axom secrets-key
   ```
3. In `.env` set `AXOM_OWNER_EMAIL`, `SUPABASE_URL` and `VITE_SUPABASE_URL`
   (the Project URL), `SUPABASE_JWKS_URL`
   (`<project-url>/auth/v1/.well-known/jwks.json`), `VITE_SUPABASE_ANON_KEY`,
   the four `AXOM_ATTESTATION_*` / `AXOM_APPROVAL_*` keys, and
   `AXOM_SECRETS_KEY`. Keep `AXOM_LIVE=false` and `NEAR_NETWORK=testnet`.
   An older Supabase project without JWKS keys uses **Project Settings → JWT
   Keys → Legacy JWT secret** in `SUPABASE_JWT_SECRET` instead.
4. `docker compose up --build`, then sign in as in step 4.

To add another admin later (or if the owner email changed), insert them
with their Supabase user UID:

```bash
docker compose exec db psql -U axom -d axom -c "INSERT INTO axom.admins (user_id, email, name) VALUES ('<UID>', 'them@example.com', 'Their Name');"
```

</details>

## Optional: NEAR testnet payouts

`./scripts/setup` sets these up for you: OpenBao (which holds the treasury
key) and the signer (the only part of ForeMan that can use it) run in
Docker next to the rest. If you set up ForeMan before payouts were part of
setup, `./scripts/start` sets them up the next time it runs (or run
`./scripts/setup payouts`).

1. When payouts are set up, the treasury's NEAR address is printed (the
   dashboard shows it too). Fund it with `./scripts/fund-testnet` (2 testnet
   NEAR; `./scripts/fund-testnet 5` for more), or send testnet NEAR from a
   testnet wallet. It sends from one funding account, made by NEAR's testnet
   helper and kept in `secrets/testnet_funder`, until that runs low. Faucets such as near-faucet.io say "invalid account":
   they only send to accounts that already exist, and the treasury only
   exists after its first deposit.
2. Put a free CoinGecko key in `NEAR_PRICE_FEED_API_KEY` in `.env`, then
   `./scripts/start`.
3. **Back up the `secrets/` folder.** It holds the signer's token and
   OpenBao's unseal key; without them the treasury key can't be used.

OpenBao starts sealed after every restart, and `./scripts/start` unseals it
with the key in `secrets/`. That's fine for testnet on your own machine. For
mainnet, use a separate OpenBao that you unseal by hand
(`infra/coolify/openbao.yml`) and point the signer at it with
`AXOM_DOCKER_OPENBAO_ADDR` in `.env`.

## AI interviews

LiveKit (video) and the AI interviewer are part of the stack: `docker compose
up` starts them, with local-only keys. For interviews to speak and listen,
add Deepgram, Cartesia and Anthropic keys on the dashboard (Integrations,
AI models) or in `.env` (`DEEPGRAM_API_KEY`, `CARTESIA_API_KEY`,
`ANTHROPIC_API_KEY`). On a server, `infra/coolify/axom.yml` runs LiveKit with
TURN; set `LIVEKIT_URL` (its public wss:// address), `LIVEKIT_API_KEY` and
`LIVEKIT_API_SECRET`.

## Platform mode: let anyone sign up companies

This turns your ForeMan into a service: people sign up, create their own
company, connect their tools, and pay a subscription in NEAR. Do the steps
above first (Supabase, keys); then:

1. **Supabase:** in Authentication, turn on **Confirm email** (invitations
   are matched by email, so addresses must be verified) and allow sign-ups.
2. **A master key for stored integration keys:**
   ```bash
   docker compose run --rm --no-deps axom axom secrets-key
   ```
   Put the printed `AXOM_SECRETS_KEY=...` in `.env` and keep a copy offline:
   without it, stored integration keys can't be decrypted.
3. **Billing:** set `AXOM_BILLING_ACCOUNT` to the NEAR account that receives
   subscription payments (testnet while testing). Optional:
   `AXOM_PLAN_PRICE_USD` (default 49) and `AXOM_TRIAL_DAYS` (default 14).
4. **Treasuries (for payouts):** in OpenBao, as the owner:
   ```bash
   bao policy write axom-provisioner infra/openbao/axom-provisioner.hcl
   bao policy write axom-signer infra/openbao/axom-signer.hcl
   bao token create -policy=axom-provisioner -no-default-policy -orphan -period=768h -field=token
   bao token create -policy=axom-signer -no-default-policy -orphan -period=768h -field=token
   ```
   and put the printed tokens straight into `.env` (never into token files):
   `OPENBAO_ADDR`, `OPENBAO_PROVISIONER_TOKEN=<first token>`, and
   `OPENBAO_TOKEN=<second token>`. Docker gives each token only to the part
   that needs it (the API and the signer). The provisioner token can only
   create and lock company keys; the signer token can only sign with them.
5. **Switch modes** in `.env`:
   ```bash
   AXOM_MULTI_TENANT=true
   AXOM_DOCKER_DATABASE_URL=postgres://axom:axom@db:5432/axom_platform
   ```
   The `axom_platform` database is created automatically the first time the
   Docker database starts. If your database volume is older than this, create
   it once:
   ```bash
   docker compose exec db psql -U axom -d axom -c "CREATE DATABASE axom_platform;"
   ```
6. **Start:**
   ```bash
   docker compose --profile payouts up --build
   ```
7. Open http://localhost:5174, **create an account**, set up two-factor, and
   follow the setup guide to create your first company. You don't need to
   insert anything into the database: the creator of a company is its owner.

**Already running ForeMan for one company?** Move it onto the platform without
copying anything (its database must be on the same Postgres server):

```bash
docker compose run --rm axom axom import-company --database axom --slug acme --name "Acme Labs" --owner-email you@example.com --owner-user-id <your Supabase user id>
```

Its data, workers, payouts, and admins carry over (admins become members;
the same Supabase project is assumed). It gets the free trial. Set up its NEAR
treasury again from Settings: the platform uses one key per company.

Each company's database is named `axom_t_<id>` on the same server. Back up
the whole server (all databases), plus `AXOM_SECRETS_KEY` and the OpenBao
snapshot.

## Going live

`AXOM_LIVE=true` makes ForeMan really send emails and contracts and post jobs.
Do not turn it on until the owner's legal review is complete (crypto
payouts, worker classification, tax reporting, AI hiring rules). Mainnet
also needs `NEAR_NETWORK=mainnet` and an explicit `NEAR_RPC_URL`. For a
server, use `infra/coolify/` (README, "Deploying on your VPS with Coolify").

## Troubleshooting

| You see | Fix |
|---|---|
| "not an active admin" | Sign in with the exact email in `AXOM_OWNER_EMAIL` (it only applies while there are no admins yet). Otherwise add yourself as in "Setting it up by hand" above. |
| "MFA-verified session required" | Finish the authenticator step; sign out and in again. |
| Sign-in fails with a network error ("Failed to fetch") | `VITE_SUPABASE_URL`/`VITE_SUPABASE_ANON_KEY` were empty when the dashboard was built: set them, then `./scripts/start`. |
| "approval endpoints disabled" in the log | Set `AXOM_APPROVAL_SIGNING_KEY` and `SUPABASE_JWKS_URL` (or `SUPABASE_JWT_SECRET`), then restart. |
| Dashboard can't reach the API (network/CORS error) | Keep `VITE_AXOM_API_URL=http://localhost:8080` and `AXOM_CORS_ORIGINS=http://localhost:5173,http://localhost:5174`. |
| Port already in use | Something else uses 8080, 5173, 5174, 5175 or 55432: stop it, or change the left-hand port in `docker-compose.yml`. |
