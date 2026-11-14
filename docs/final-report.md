# Final report: judge-mode acceptance run

Run on 2026-11-08 in the build environment with `./judge/run.sh` (mocks for DocuSeal, Postiz, Warmbly, CRM, Upwork, SMTP, LiveKit; real NEAR testnet; real OpenBao 2.7.1 in dev mode). Regenerate with a fresh run; the script writes `judge/out/report.json`.

## What ran

- Org chart built from the master plan (5 departments, 5 AI workers); 5 tasks routed, 1 hiring request opened for a human designer; a job posted to Upwork (mock) after the policy check.
- 1 AI interview: disclosure and consent recorded before any question; the agent refused to discuss pay; transcript, summary, and rubric scores stored.
- 1 worker fired: Ops Agent (seeded low performer) (Operations Agent); owner notification with reason, metrics, and audit link.
- Payout sent on NEAR testnet, signed through OpenBao: [`GuzqvdPRAZzzW3xPQcSo1VXViY2o79KzvrXSB5Kmy7FT`](https://testnet.nearblocks.io/txns/GuzqvdPRAZzzW3xPQcSo1VXViY2o79KzvrXSB5Kmy7FT).
- The held payout from this run is left waiting in the dashboard for the human to approve. In an earlier run on the same day, approving it with `./judge/approve.sh` (MFA admin session) made the signer send it on testnet: [`zms62YYVXLqGuajLYCqbjXsL1jb3D8skm8FC8jXnEKS`](https://testnet.nearblocks.io/txns/zms62YYVXLqGuajLYCqbjXsL1jb3D8skm8FC8jXnEKS).

## Monthly cap (UTC month 2026-11-01)

| Cap | Sent (committed) | Reserved | Remaining |
|---|---|---|---|
| 500.00 | 186.00 | 61.00 | 253.00 |

Committed includes 180 USD of seeded earlier payouts (history only, no on-chain transaction) so the meter shows partial use. Network fees count toward the cap.

## Waiting for approval

- 60.00 USD to `144f864122f3ca245c9acb03de8f23e99a9c29edccf7b3c964de1109967e13aa`: recipient total 60.00 USD over 24h (this payout 60.00) is at or above the 50 USD threshold

## Blocked

- 400.00 USD: **cap_exceeded**. monthly cap exceeded: 247.000000 used + 401.000 requested > 500.000000 cap for 2026-11-01 (owner notified)
- 6 payout(s) **blocked by the first-payout gates** (no verified NEAR address and/or no tax form on file): the plan's AI workers and the fired worker were never given addresses, which shows the gates working.

## Acceptance checklist (milestone 9)

| Check | Result |
|---|---|
| Org chart built and tasks routed | passed |
| Job posts to Upwork (sandbox or mock) | passed (mock) |
| Candidate interview with disclosure and consent recorded | passed (real interview logic; LiveKit mocked) |
| Seeded low performer fired, owner notification generated | passed |
| Payout under USD 50 executes automatically on testnet | passed |
| Payout of USD 50 or more waits, proceeds only after dashboard approval | passed |
| Payout past USD 500 blocked as cap_exceeded, owner notified | passed |
| Injection tests (approve my payout, raise the cap, exfiltrate via transcript, tool output) | passed (test suite) |
| Judge mode runs from a clean clone without the VPS | passed with a local database; the Docker path for Postgres is untested here |
| Coolify services running, Supabase migrated, seed loaded on the VPS | not run: needs the VPS |
