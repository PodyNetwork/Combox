# Milestone 9: End-to-end demo and acceptance

Read 00_shared_context.md first. Its rules apply throughout this milestone.

## Seed data

Load the seed data into the self-hosted Supabase development instance.

- A sample master plan, "launch a SaaS product in 90 days", split into departments and first tasks.
- One seeded low-performing AI worker with enough task history to trigger firing under the milestone 3 rules.
- Seeded payouts that use part of the monthly cap, so the cap meter shows partial usage.

## Documentation

- README: installing Coolify on the VPS, deploying each self-hosted service (Supabase, LiveKit, OpenBao, DocuSeal, Postiz, and ForeMan), and backups; dry-run mode; the NEAR testnet flow; the monthly cap; and how to connect an AI client to the MCP server.
- .env.example complete, including MONTHLY_PAYOUT_CAP_USD=500 and PAYOUT_APPROVAL_THRESHOLD_USD=50.
- DECISIONS.md complete, sorted by milestone.
- A note stating that live mode requires the legal review listed in 00.
- A tested backup and restore for Postgres and OpenBao, since the owner runs both.

## Judge mode

Judges must be able to run the project without our VPS. Build a judge mode:

- One command starts the whole app locally: Postgres in a local container, the admin backend, the signer, and the meeting agent.
- External services are replaced by mocks: DocuSeal, Postiz, Warmbly, the CRM, Upwork, SMTP, and LiveKit. LiveKit may use a local dev server instead of a mock if that needs no extra cost.
- The NEAR integration stays real on testnet. The signer uses a local OpenBao dev instance, and a payout produces a testnet transaction hash. OpenBao dev mode is allowed only in judge mode and never on the VPS.
- A seeded run shows one interview, one firing, one payout under USD 50, and one payout held for approval.
- README steps a judge can follow in under 10 minutes, from a clean clone to a visible result.

## Integration test

Run the seed plan end to end against the mocks.

## Acceptance

In dry-run mode with mocks and NEAR testnet, after the Coolify services are running, the Supabase schema is migrated, and the seed data is loaded, confirm:

- The org chart is built and tasks are routed.
- A job posts to Upwork (sandbox or mock).
- A candidate interview runs in a LiveKit room, with disclosure and consent recorded.
- The seeded low performer is fired, and the owner notification is generated.
- A payout under USD 50 executes automatically on testnet.
- A payout of USD 50 or more waits in pending_approval and proceeds only after dashboard approval.
- A payout that would push the month past USD 500 is blocked as cap_exceeded, and the owner is notified.
- The injection tests pass, including the candidate message that tries to approve a payout or raise the cap, which must be blocked.
- Judge mode runs from a clean clone on a machine without the VPS.

End with a final report covering what ran, what was blocked, the cap usage for the month, and what is waiting for approval.
